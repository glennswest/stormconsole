//! RDP to a machine, from its page (#54): stormrdp's gateway, through the console.
//!
//! The gateway on this node serves `/rdcleanpath`, the browser's way in, and `POST /api/v1/login`, which trades a
//! rustkube bearer for a short-lived RDP login. A node's golden ships no client page (no `web_root`), and its TLS
//! pair is self-signed, so a browser would refuse to open a socket to it. So the client is in the console's page, and
//! the console relays the socket and makes the exchange.
//!
//! **Loopback only.** The gateway's certificate cannot be checked, so the viewer's bearer goes nowhere an unchecked
//! certificate could take it off this machine. A gateway serves its own node's machines anyway, and
//! `GET :9101/api/v1/vms` says which, so a machine on another node is said to be that node's console's.
//!
//! The password goes to the page, which needs it for NLA inside the tunnel to the gateway. It is in no URL, log or
//! audit record, and is never stored. A new login by the same user replaces their old one, as the gateway does.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as Browser, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message as Upstream;
use tokio_tungstenite::WebSocketStream;

use crate::{refuse_hidden, Inner};

/// As stormcentral's registered config has it, always this machine's.
const GATEWAY: &str = "https://127.0.0.1:8443";
const ADMIN: &str = "http://127.0.0.1:9101";
/// stormrdp's stormd (the port layout: :9101 + 100).
const STORMD: u16 = 9201;

/// The client for the gateway. Its pair is minted on first start with nothing to check it against, and this only
/// ever dials the constant above.
fn gateway() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .http1_only()
        .build()
        .map_err(|e| e.to_string())
}

/// A person's own bearer, or for root the console's. Nothing else: the gateway's check is on the person.
fn bearer(inner: &Inner, viewer: &Viewer) -> Option<String> {
    viewer.token.clone().or_else(|| if viewer.root { inner.client.as_ref().and_then(|c| c.token()) } else { None })
}

/// Why this viewer cannot, or `None` when it is the gateway's to say.
pub fn refusal(may_write: bool, has_bearer: bool) -> Option<&'static str> {
    if !may_write {
        return Some("Remote desktop gives you the machine's keyboard: it needs the operator role on this console.");
    }
    if !has_bearer {
        return Some("Remote desktop logs in as you, with your kubernetes identity, and this console has none for you. The Graphical console does not need one.");
    }
    None
}

/// The gateway's row for this machine, out of `GET :9101/api/v1/vms`.
pub fn target_of(vms: &Value, ns: &str, name: &str) -> Option<String> {
    let row = vms["items"].as_array()?.iter().find(|v| v["namespace"] == ns && v["name"] == name)?;
    row["target"].as_str().map(str::to_string).or_else(|| Some(format!("{name}.{ns}")))
}

/// `GET /vms/{ns}/{name}/rdp`: may it be offered, and why not.
pub async fn status(State(inner): State<Arc<Inner>>, viewer: Viewer, Path((ns, name)): Path<(String, String)>) -> Response {
    if let Some(r) = refuse_hidden(&inner, &viewer, &ns).await {
        return r;
    }
    let why = refusal(viewer.may_write(), bearer(&inner, &viewer).is_some());
    let asked = inner.http.get(format!("{ADMIN}/api/v1/vms")).timeout(Duration::from_secs(3)).send().await;
    let (target, reason) = match asked {
        Ok(r) if r.status().is_success() => {
            let vms: Value = r.json().await.unwrap_or(Value::Null);
            match target_of(&vms, &ns, &name) {
                Some(t) => (Some(t), None),
                None => (None, Some("The gateway on this node has no such machine. It serves the machines running here: a stopped machine has none, and one on another node is on that node's console.".to_string())),
            }
        }
        Ok(r) => (None, Some(format!("stormrdp answered {}.", r.status()))),
        Err(_) if console_core::upstream::not_started(ADMIN, STORMD).await => (None, Some("stormrdp is not started on this node.".to_string())),
        Err(e) => (None, Some(format!("stormrdp did not answer: {}.", console_core::dial::chain(&e)))),
    };
    Json(json!({
        "target": target,
        "available": target.is_some() && why.is_none(),
        "reason": reason.as_deref().or(why),
    }))
    .into_response()
}

/// `POST /vms/{ns}/{name}/rdp/login`: the viewer's bearer for an RDP login.
pub async fn login(State(inner): State<Arc<Inner>>, viewer: Viewer, Path((ns, name)): Path<(String, String)>) -> Response {
    if let Some(r) = refuse_hidden(&inner, &viewer, &ns).await {
        return r;
    }
    let token = bearer(&inner, &viewer);
    if let Some(why) = refusal(viewer.may_write(), token.is_some()) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": why}))).into_response();
    }
    let http = match gateway() {
        Ok(c) => c,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response(),
    };
    let asked = http
        .post(format!("{GATEWAY}/api/v1/login"))
        .bearer_auth(token.unwrap_or_default())
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    let r = match asked {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("stormrdp did not answer: {}", console_core::dial::chain(&e))}))).into_response(),
    };
    let code = r.status().as_u16();
    let body = r.text().await.unwrap_or_default();
    if code != 200 {
        let said = serde_json::from_str::<Value>(&body).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(body);
        return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("stormrdp refused the login ({code}): {}", said.trim())}))).into_response();
    }
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({"login": v["login"], "password": v["password"], "expires": v["expires"], "target": format!("{name}.{ns}")})),
    )
        .into_response()
}

/// `WS /vms/{ns}/{name}/rdp/session`: the browser's RDCleanPath socket, to the gateway. The bytes are RDP inside TLS
/// to the gateway, so the console sees none of what is typed.
pub async fn session(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, _name)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Some(r) = refuse_hidden(&inner, &viewer, &ns).await {
        return r;
    }
    if let Some(why) = refusal(viewer.may_write(), bearer(&inner, &viewer).is_some()) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": why}))).into_response();
    }
    ws.on_upgrade(relay)
}

async fn relay(mut browser: WebSocket) {
    let up = match dial().await {
        Ok(up) => up,
        Err(why) => {
            tracing::warn!("stormrdp's /rdcleanpath: {why}");
            let _ = browser.close().await;
            return;
        }
    };
    let (mut up_tx, mut up_rx) = up.split();
    let (mut br_tx, mut br_rx) = browser.split();
    let to_gateway = async {
        while let Some(Ok(m)) = br_rx.next().await {
            let out = match m {
                Browser::Binary(b) => Upstream::Binary(b),
                Browser::Text(t) => Upstream::Text(t.as_str().into()),
                Browser::Close(_) => break,
                _ => continue,
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = up_tx.close().await;
    };
    let to_browser = async {
        while let Some(Ok(m)) = up_rx.next().await {
            let out = match m {
                Upstream::Binary(b) => Browser::Binary(b),
                Upstream::Text(t) => Browser::Text(t.as_str().into()),
                Upstream::Close(_) => break,
                _ => continue,
            };
            if br_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = br_tx.close().await;
    };
    tokio::select! {
        _ = to_gateway => {}
        _ = to_browser => {}
    }
}

async fn dial() -> Result<WebSocketStream<reqwest::Upgraded>, String> {
    let asked = gateway()?
        .get(format!("{GATEWAY}/rdcleanpath"))
        .header(header::CONNECTION, "Upgrade")
        .header(header::UPGRADE, "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .send()
        .await
        .map_err(|e| console_core::dial::chain(&e))?;
    if asked.status().as_u16() != 101 {
        return Err(format!("it answered {}", asked.status()));
    }
    let raw = asked.upgrade().await.map_err(|e| e.to_string())?;
    Ok(WebSocketStream::from_raw_socket(raw, Role::Client, None).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_may() {
        assert!(refusal(false, true).unwrap().contains("operator"));
        assert!(refusal(true, false).unwrap().contains("kubernetes identity"));
        assert_eq!(refusal(true, true), None);
    }

    #[test]
    fn the_gateways_row_for_a_machine() {
        let v = json!({"items": [
            {"namespace": "shop", "name": "vm1", "uid": "u", "target": "vm/shop/vm1", "framebuffer": true},
            {"namespace": "other", "name": "vm1", "uid": "w"},
        ]});
        assert_eq!(target_of(&v, "shop", "vm1").as_deref(), Some("vm/shop/vm1"));
        assert_eq!(target_of(&v, "other", "vm1").as_deref(), Some("vm1.other"));
        assert_eq!(target_of(&v, "shop", "nope"), None);
        assert_eq!(target_of(&json!({}), "shop", "vm1"), None);
    }

    #[test]
    fn the_console_only_ever_dials_this_machine() {
        assert!(console_core::upstream::is_loopback(GATEWAY));
        assert!(console_core::upstream::is_loopback(ADMIN));
    }
}
