//! The two console doors: serial and framebuffer.
//!
//! Both are streams rather than component actions, and both are addressed
//! by **VM, not node** — stormvm resolves which node runs it, and the same
//! URL keeps working across a live migration (stormvm `docs/DESIGN.md`,
//! "Console — serial and framebuffer, one door"). The console's job is to
//! put them on its own origin, so the browser never learns a node address
//! and never has a second thing to authenticate to:
//!
//! ```text
//! browser ⇄ /api/plugins/vm/console/{ns}/{name}/serial ⇄ stormvm :9095 /api/v1/vms/{id}/console/serial
//! browser ⇄ /api/plugins/vm/console/{ns}/{name}/vnc    ⇄ stormvm :9095 /api/v1/vms/{id}/console/vnc
//! ```
//!
//! Frames pass through untouched in both directions: the serial door
//! carries bytes, the framebuffer door carries RFB, and neither is
//! anything this process should be interpreting.
//!
//! **What actually works today.** stormvm serves neither endpoint yet
//! (its phase 1 has "Console service" unticked), and the other route to a
//! guest's serial — the pod log the kubelet already writes — needs
//! rustkube#55 and rustkube-node#34. So the doors are built, probed, and
//! honest: with no stormvm on the node the capability answer says which
//! upstream is missing, and the UI says that instead of showing a
//! terminal that will never print.

use std::time::Duration;

use axum::extract::ws::{Message as AxumMessage, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as UpMessage;

/// Which doors are open for one VM, and — when they are not — which
/// upstream has to land first. A closed door that says nothing is worse
/// than no door.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Capabilities {
    pub serial: bool,
    pub vnc: bool,
    /// The stormvm this console would dial, described for a reader rather
    /// than addressed (a node-local URL means nothing in a browser).
    pub upstream: String,
    pub reason: String,
}

/// stormvm is optional and usually absent; say which thing is missing
/// rather than "unavailable".
pub fn capabilities(upstream: Option<&str>, reachable: bool) -> Capabilities {
    match (upstream, reachable) {
        (None, _) => Capabilities {
            serial: false,
            vnc: false,
            upstream: String::new(),
            reason: "no stormvm configured — set [vm] url to the node's stormvm".into(),
        },
        (Some(url), false) => Capabilities {
            serial: false,
            vnc: false,
            upstream: console_core::upstream::describe(url),
            reason: "stormvm is not answering — its console service is unbuilt (stormvm phase 1). \
                     The guest's serial is still in the pod log, which needs rustkube#55 and \
                     rustkube-node#34 to read from here."
                .into(),
        },
        (Some(url), true) => Capabilities {
            serial: true,
            vnc: true,
            upstream: console_core::upstream::describe(url),
            reason: String::new(),
        },
    }
}

/// The stormvm path for one door. Addressed by VM id, never by node.
pub fn upstream_path(kind: Door, ns: &str, name: &str) -> String {
    format!("/api/v1/vms/{ns}/{name}/console/{}", kind.as_str())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Serial,
    Vnc,
}

impl Door {
    pub fn as_str(self) -> &'static str {
        match self {
            Door::Serial => "serial",
            Door::Vnc => "vnc",
        }
    }
}

/// `http://host:port` + a path → the `ws://` URL to dial.
pub fn ws_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let dialled = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("ws://{base}")
    };
    format!("{dialled}{path}")
}

/// Relay one browser socket to one stormvm socket until either end goes.
/// Nothing here reads the payload: a terminal's bytes and RFB's framing
/// are the endpoints' business.
pub async fn relay(browser: WebSocket, url: String) {
    let upstream = match tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(&url),
    )
    .await
    {
        Ok(Ok((s, _))) => s,
        Ok(Err(e)) => {
            close_with(browser, &format!("stormvm refused the console: {e}")).await;
            return;
        }
        Err(_) => {
            close_with(browser, "stormvm did not answer the console in 10s").await;
            return;
        }
    };

    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut br_tx, mut br_rx) = browser.split();

    let to_upstream = async move {
        while let Some(Ok(msg)) = br_rx.next().await {
            let out = match msg {
                AxumMessage::Text(t) => UpMessage::Text(t.as_str().into()),
                AxumMessage::Binary(b) => UpMessage::Binary(b),
                AxumMessage::Close(_) => break,
                AxumMessage::Ping(p) => UpMessage::Ping(p),
                AxumMessage::Pong(p) => UpMessage::Pong(p),
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = up_tx.close().await;
    };

    let to_browser = async move {
        while let Some(Ok(msg)) = up_rx.next().await {
            let out = match msg {
                UpMessage::Text(t) => AxumMessage::Text(t.as_str().into()),
                UpMessage::Binary(b) => AxumMessage::Binary(b),
                UpMessage::Close(_) => break,
                UpMessage::Ping(p) => AxumMessage::Ping(p),
                UpMessage::Pong(p) => AxumMessage::Pong(p),
                UpMessage::Frame(_) => continue,
            };
            if br_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = br_tx.close().await;
    };

    // Either direction ending ends the session: a half-open console is a
    // terminal that accepts typing nothing will ever see.
    tokio::select! {
        _ = to_upstream => {}
        _ = to_browser => {}
    }
}

/// Say why the door did not open, in the socket itself — the browser has
/// already upgraded by this point, so an HTTP status is no longer available
/// to explain with.
async fn close_with(mut socket: WebSocket, why: &str) {
    let _ = socket.send(AxumMessage::Text(format!("\r\n[console] {why}\r\n").into())).await;
    let _ = socket.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_door_is_addressed_by_vm_never_by_node() {
        assert_eq!(
            upstream_path(Door::Serial, "default", "web-1"),
            "/api/v1/vms/default/web-1/console/serial"
        );
        assert_eq!(
            upstream_path(Door::Vnc, "default", "web-1"),
            "/api/v1/vms/default/web-1/console/vnc"
        );
    }

    #[test]
    fn the_scheme_follows_the_upstream() {
        assert_eq!(ws_url("http://127.0.0.1:9095", "/x"), "ws://127.0.0.1:9095/x");
        assert_eq!(ws_url("https://n:9095/", "/x"), "wss://n:9095/x");
        assert_eq!(ws_url("127.0.0.1:9095", "/x"), "ws://127.0.0.1:9095/x");
    }

    #[test]
    fn a_closed_door_names_what_is_missing() {
        let none = capabilities(None, false);
        assert!(!none.serial && !none.vnc);
        assert!(none.reason.contains("no stormvm configured"), "{}", none.reason);

        let down = capabilities(Some("http://127.0.0.1:9095"), false);
        assert!(!down.serial);
        assert_eq!(down.upstream, "on this node :9095", "loopback is never offered as an address");
        assert!(down.reason.contains("rustkube#55"), "{}", down.reason);

        let up = capabilities(Some("http://127.0.0.1:9095"), true);
        assert!(up.serial && up.vnc);
        assert!(up.reason.is_empty());
    }
}
