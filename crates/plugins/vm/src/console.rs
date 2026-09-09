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
//! **Reaching them.** stormvm binds loopback by default and admits a
//! connection from the node without a credential — everything that
//! legitimately opens a console is on the node, and stormconsole is one of
//! those things, relaying from its own authenticated origin. A console
//! pointed at a *remote* node (the dev workflow in this repo's README) is
//! refused, and cannot help itself: minting is loopback-only by design, so
//! there is no token for it to fetch. That is stormvm's call and the right
//! one; the capability answer says so rather than returning a bare 401.

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

/// What *this VM's* doors are, asked of stormvm.
///
/// Reachability is not the whole answer: stormvm registers a serial socket
/// only if the spec asked for a console and a VNC socket only if it asked
/// for a framebuffer, and it reports both per VM. Offering a graphical
/// console on a machine whose spec never asked for one is a tab that opens
/// onto a refusal — the CH path has no framebuffer at all, and stormvm's
/// design says so at define time rather than at first look.
pub async fn for_vm(
    client: &reqwest::Client,
    upstream: Option<&str>,
    reachable: bool,
    ns: &str,
    name: &str,
) -> Capabilities {
    let base = match (upstream, reachable) {
        (Some(u), true) => u,
        _ => return capabilities(upstream, reachable),
    };
    let url = format!("{}/api/v1/vms/{ns}/{name}", base.trim_end_matches('/'));
    let resp = client.get(&url).timeout(Duration::from_secs(5)).send().await;
    let described = console_core::upstream::describe(base);
    match resp {
        Ok(r) if r.status().is_success() => {
            let v: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            let door = |k: &str| v.pointer(&format!("/console/{k}")).and_then(serde_json::Value::as_bool).unwrap_or(false);
            let (serial, vnc) = (door("serial"), door("vnc"));
            Capabilities {
                serial,
                vnc,
                upstream: described,
                reason: match (serial, vnc) {
                    (true, true) => String::new(),
                    (true, false) => "this machine has no framebuffer — its spec asked for a \
                                      serial console only, so there is nothing to draw"
                        .into(),
                    (false, true) => "this machine has no serial console — its spec asked for a \
                                      framebuffer only"
                        .into(),
                    (false, false) => "this machine asked for neither console".into(),
                },
            }
        }
        Ok(r) if r.status().as_u16() == 404 => Capabilities {
            serial: false,
            vnc: false,
            upstream: described,
            reason: "stormvm on this node is not running this machine — it may have stopped, \
                     or it may be running somewhere else"
                .into(),
        },
        _ => capabilities(upstream, false),
    }
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
            reason: "stormvm is not answering on this node, so there is no console to open. \
                     It serves the doors (`stormvm serve`), and it binds loopback by default — \
                     a console running off the node cannot reach them, because a door opened \
                     from elsewhere needs a token minted on the node."
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
            close_with(browser, &refusal(&e)).await;
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

/// stormvm's refusals in words.
///
/// The upgrade carries only a status code — the JSON body stormvm writes
/// never reaches a websocket client — so "HTTP error: 409 Conflict" is all
/// tungstenite can say, and it is exactly the wrong amount of information
/// to put in front of somebody waiting to see a screen.
fn refusal(e: &tokio_tungstenite::tungstenite::Error) -> String {
    use tokio_tungstenite::tungstenite::Error;
    let status = match e {
        Error::Http(r) => Some(r.status().as_u16()),
        _ => None,
    };
    match status {
        Some(401) => "stormvm refused this console: a door opened from off the node needs a \
                      token minted on it, and this console is not on that node"
            .into(),
        Some(404) => "stormvm is not running this machine — it may have stopped".into(),
        Some(409) => "this machine has no such console: its spec did not ask for one".into(),
        Some(s) => format!("stormvm refused this console ({s})"),
        None => format!("stormvm could not be reached: {e}"),
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
    fn a_refusal_is_translated_out_of_a_status_code() {
        use tokio_tungstenite::tungstenite::Error;
        let http = |code: u16| {
            let r = axum::http::Response::builder()
                .status(code)
                .body(None::<Vec<u8>>)
                .unwrap();
            Error::Http(Box::new(r))
        };
        assert!(refusal(&http(409)).contains("did not ask for one"), "{}", refusal(&http(409)));
        assert!(refusal(&http(401)).contains("token minted"), "{}", refusal(&http(401)));
        assert!(refusal(&http(404)).contains("may have stopped"), "{}", refusal(&http(404)));
        // Whatever it is, it never reaches a viewer as a bare status line.
        for c in [409u16, 401, 404, 500] {
            assert!(!refusal(&http(c)).starts_with("HTTP error"), "{c}");
        }
    }

    #[test]
    fn a_closed_door_names_what_is_missing() {
        let none = capabilities(None, false);
        assert!(!none.serial && !none.vnc);
        assert!(none.reason.contains("no stormvm configured"), "{}", none.reason);

        let down = capabilities(Some("http://127.0.0.1:9095"), false);
        assert!(!down.serial);
        assert_eq!(down.upstream, "on this node :9095", "loopback is never offered as an address");
        assert!(down.reason.contains("stormvm serve"), "{}", down.reason);
        // The remote case is the one a reader will hit and be puzzled by.
        assert!(down.reason.contains("off the node"), "{}", down.reason);

        let up = capabilities(Some("http://127.0.0.1:9095"), true);
        assert!(up.serial && up.vnc);
        assert!(up.reason.is_empty());
    }
}
