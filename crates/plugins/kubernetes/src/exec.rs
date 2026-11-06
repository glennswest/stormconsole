//! A shell in a container (#12): `kubectl exec -it`, from the pod page.
//!
//! `browser ⇄ /api/plugins/k8s/pods/{ns}/{name}/exec ⇄ apiserver pods/exec ⇄ kubelet`, a WebSocket on
//! each side in `v4.channel.k8s.io`: binary messages whose first byte is the channel (0 stdin, 1 stdout,
//! 2 stderr, 3 the result as a `metav1.Status`, 4 a resize). The console passes them on and reads none.
//!
//! **It goes as the person, or not at all.** A shell is the most anyone can do to a pod, so three things
//! must all say yes: the pod is one this viewer may see, their console role may change things (a GET is
//! past the write gate, so it is asked here), and the apiserver allows them `create pods/exec`. The
//! console's own credential is never used: someone signed in with no kubernetes identity is refused,
//! rather than given whatever the console's ServiceAccount may do.
//!
//! The upgrade goes through reqwest and not tungstenite's dialer, for the apiserver's own TLS
//! (`[kubernetes] ca_file`, followed when it moves) and because a refusal then has a body: a person is
//! told "pods/exec is forbidden", not "HTTP error: 403".

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message as Browser, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message as Upstream;
use tokio_tungstenite::WebSocketStream;

use crate::Inner;

pub const PROTOCOL: &str = "v4.channel.k8s.io";
const STDIN: u8 = 0;
const ERROR: u8 = 3;
const RESIZE: u8 = 4;

/// Why this viewer may not have a shell, in a sentence, or `None` when it is the apiserver's to say.
pub fn refusal(may_write: bool, identity: bool) -> Option<&'static str> {
    if !may_write {
        return Some("A terminal changes things: it needs the operator role on this console.");
    }
    if !identity {
        return Some("A shell goes as you, and this sign-in has no kubernetes identity. Sign in as a user with one.");
    }
    None
}

/// The shell asked for by default: bash where there is one, and a terminal type it can use.
fn default_command() -> Vec<String> {
    let start = "TERM=xterm; export TERM; command -v bash >/dev/null 2>&1 && exec bash; exec sh";
    ["/bin/sh", "-c", start].map(String::from).to_vec()
}

/// What runs: words split on spaces, or the default. A person with `pods/exec` can run anything, so
/// this is not a limit. It is how a form's one line becomes an argv.
pub fn command_of(line: &str) -> Vec<String> {
    let words: Vec<String> = line.split_whitespace().map(String::from).collect();
    if words.is_empty() {
        default_command()
    } else {
        words
    }
}

/// The apiserver's query, in kubectl's spelling; it translates it for the kubelet. With a terminal
/// stderr is on stdout, so it is not asked for.
pub fn query(container: &str, command: &[String]) -> Vec<(&'static str, String)> {
    let mut q = vec![
        ("container", container.to_string()),
        ("stdin", "true".to_string()),
        ("stdout", "true".to_string()),
        ("tty", "true".to_string()),
    ];
    q.extend(command.iter().map(|c| ("command", c.clone())));
    q
}

/// May this frame from the browser go on? Only what a terminal sends: its typing and its size.
pub fn from_browser(frame: &[u8]) -> bool {
    matches!(frame.first(), Some(&STDIN) | Some(&RESIZE))
}

/// A refusal as the browser's end of the protocol reads any result.
fn status_frame(message: &str) -> Bytes {
    let mut out = vec![ERROR];
    out.extend_from_slice(json!({"status": "Failure", "message": message}).to_string().as_bytes());
    out.into()
}

/// What the apiserver or kubelet said, from a body that may be a `Status` or plain words.
pub fn said(code: u16, body: &str) -> String {
    let words = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.trim().chars().take(400).collect());
    if words.is_empty() {
        format!("the apiserver answered {code}")
    } else {
        words
    }
}

#[derive(Deserialize, Default)]
pub struct ExecQuery {
    #[serde(default)]
    container: String,
    #[serde(default)]
    command: String,
}

/// `WS /pods/{ns}/{name}/exec?container=&command=`
pub async fn exec(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    Query(q): Query<ExecQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let pod = match crate::podpage::the_pod(&inner, &viewer, &ns, &name).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(why) = refusal(viewer.may_write(), viewer.token.is_some()) {
        return (StatusCode::FORBIDDEN, Json(json!({"error": why}))).into_response();
    }
    let known = ["containers", "initContainers", "ephemeralContainers"].iter().any(|k| {
        pod.pointer(&format!("/spec/{k}"))
            .and_then(Value::as_array)
            .is_some_and(|l| l.iter().any(|c| c.get("name").and_then(Value::as_str) == Some(q.container.as_str())))
    });
    if !known {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("no container {:?} in this pod", q.container)}))).into_response();
    }
    let Some(token) = viewer.token.clone() else { return StatusCode::FORBIDDEN.into_response() };
    let path = format!("/api/v1/namespaces/{ns}/pods/{name}/exec");
    let params = query(&q.container, &command_of(&q.command));
    ws.protocols([PROTOCOL]).on_upgrade(move |socket| session(inner, socket, path, params, token))
}

async fn session(inner: Arc<Inner>, mut browser: WebSocket, path: String, params: Vec<(&'static str, String)>, token: String) {
    let upstream = match dial(&inner, &path, &params, &token).await {
        Ok(s) => s,
        Err(why) => {
            let _ = browser.send(Browser::Binary(status_frame(&why))).await;
            let _ = browser.close().await;
            return;
        }
    };
    let (mut up_tx, mut up_rx) = upstream.split();
    let (mut br_tx, mut br_rx) = browser.split();

    let to_upstream = async move {
        while let Some(Ok(msg)) = br_rx.next().await {
            let out = match msg {
                Browser::Binary(b) if from_browser(&b) => Upstream::Binary(b),
                Browser::Close(_) => break,
                Browser::Ping(p) => Upstream::Ping(p),
                Browser::Pong(p) => Upstream::Pong(p),
                _ => continue,
            };
            if up_tx.send(out).await.is_err() {
                break;
            }
        }
        // The kubelet stops the exec when its client goes.
        let _ = up_tx.close().await;
    };
    let to_browser = async move {
        while let Some(Ok(msg)) = up_rx.next().await {
            let out = match msg {
                Upstream::Binary(b) => Browser::Binary(b),
                Upstream::Close(_) => break,
                Upstream::Ping(p) => Browser::Ping(p),
                Upstream::Pong(p) => Browser::Pong(p),
                _ => continue,
            };
            if br_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = br_tx.close().await;
    };
    tokio::select! {
        _ = to_upstream => {}
        _ = to_browser => {}
    }
}

type Stream = WebSocketStream<reqwest::Upgraded>;

async fn dial(inner: &Inner, path: &str, params: &[(&'static str, String)], token: &str) -> Result<Stream, String> {
    let client = inner.client.as_ref().ok_or("no apiserver")?;
    let http = client.conn().http1()?;
    let asked = http
        .get(format!("{}{path}", client.base()))
        .query(params)
        .bearer_auth(token)
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .header("Sec-WebSocket-Protocol", PROTOCOL)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("the apiserver did not answer: {}", console_core::dial::chain(&e)))?;
    let code = asked.status().as_u16();
    if code != 101 {
        return Err(said(code, &asked.text().await.unwrap_or_default()));
    }
    let raw = asked.upgrade().await.map_err(|e| format!("the upgrade failed: {e}"))?;
    Ok(WebSocketStream::from_raw_socket(raw, Role::Client, None).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_may_have_a_shell() {
        assert!(refusal(false, true).unwrap().contains("operator"), "a reader, even with kubernetes rights");
        assert!(refusal(true, false).unwrap().contains("identity"), "never the console's own credential");
        assert_eq!(refusal(true, true), None, "then it is the apiserver's to say");
    }

    #[test]
    fn a_form_line_becomes_an_argv() {
        assert_eq!(command_of("ls  -l /tmp"), ["ls", "-l", "/tmp"]);
        let default = command_of("  ");
        assert_eq!(default[0], "/bin/sh");
        assert!(default[2].contains("TERM=xterm") && default[2].contains("exec sh"));
    }

    #[test]
    fn the_query_is_kubectls_with_a_terminal() {
        let q = query("app", &["ls".to_string(), "-l".to_string()]);
        assert!(q.contains(&("container", "app".to_string())));
        assert!(q.contains(&("tty", "true".to_string())));
        assert!(!q.iter().any(|(k, _)| *k == "stderr"), "a terminal's stderr is on stdout");
        let commands: Vec<&str> = q.iter().filter(|(k, _)| *k == "command").map(|(_, v)| v.as_str()).collect();
        assert_eq!(commands, ["ls", "-l"]);
    }

    #[test]
    fn only_a_terminals_frames_go_on() {
        assert!(from_browser(&[0, b'l']));
        assert!(from_browser(&[4, b'{']));
        for f in [&[1u8, 0][..], &[2], &[3], &[255, 0], &[]] {
            assert!(!from_browser(f), "{f:?}");
        }
    }

    #[test]
    fn a_refusal_is_in_the_words_it_came_in() {
        assert_eq!(said(403, r#"{"kind":"Status","message":"pods \"web\" is forbidden"}"#), "pods \"web\" is forbidden");
        assert_eq!(said(404, "no such container\n"), "no such container");
        assert_eq!(said(500, ""), "the apiserver answered 500");
        let f = status_frame("no");
        assert_eq!(f[0], ERROR);
        assert_eq!(serde_json::from_slice::<Value>(&f[1..]).unwrap()["message"], "no");
    }
}
