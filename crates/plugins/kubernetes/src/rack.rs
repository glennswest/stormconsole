//! A node's rack (#77): the Node label `topology.storm.io/rack` the Drives page groups by (#43).
//!
//! stormipmi sets it from a machine's location, but only runs on a single node (`roles=sno`), and a node
//! with no BareMetalHost has no location at all. Here an admin sets it by hand for those. It goes **as the
//! person**: `admin` on this console, `patch nodes` at the apiserver, and never the console's credential.
//! The page offers this only where stormipmi has no machine for the node, so it has nothing to fight. If a
//! location is recorded later it wins: stormipmi sets the label to it (`location::plan`).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::Inner;

pub const LABEL: &str = "topology.storm.io/rack";

/// A Kubernetes label value, in the words stormipmi refuses one with.
pub fn check(rack: &str) -> Result<(), String> {
    let edge = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let ok = rack.len() <= 63
        && rack.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && edge(rack.chars().next())
        && edge(rack.chars().last());
    if ok {
        Ok(())
    } else {
        Err(format!("{rack:?}: up to 63 letters, digits, '-', '_' or '.', starting and ending with a letter or digit"))
    }
}

/// The merge patch. An empty rack takes the label off.
pub fn patch(rack: &str) -> Value {
    let value = if rack.is_empty() { Value::Null } else { json!(rack) };
    json!({"metadata": {"labels": {LABEL: value}}})
}

/// Why this viewer may not set it, or `None` when it is the apiserver's to say.
pub fn refusal(admin: bool, identity: bool) -> Option<&'static str> {
    if !admin {
        return Some("Setting a node's rack needs the admin role on this console.");
    }
    if !identity {
        return Some("This goes as you, and this sign-in has no kubernetes identity. Sign in as a user with one.");
    }
    None
}

/// `GET /nodes/{name}/rack`: the label, and what this viewer may do about it.
pub async fn read(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    let Some(node) = inner.store.object("node", &name).await else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no Node {name}")}))).into_response();
    };
    let rack = node.pointer("/metadata/labels").and_then(|l| l.get(LABEL)).cloned().unwrap_or(Value::Null);
    let why = refusal(viewer.has_role("admin"), viewer.may_act_upstream());
    Json(json!({"rack": rack, "may": why.is_none(), "why": why})).into_response()
}

#[derive(Deserialize)]
pub struct Change {
    #[serde(default)]
    rack: String,
}

/// `PUT /nodes/{name}/rack {"rack": "r1"}`; an empty one clears it.
pub async fn write(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>, Json(change): Json<Change>) -> Response {
    let refuse = |code: StatusCode, why: String| (code, Json(json!({"error": why}))).into_response();
    if let Some(why) = refusal(viewer.has_role("admin"), viewer.may_act_upstream()) {
        return refuse(StatusCode::FORBIDDEN, why.to_string());
    }
    let rack = change.rack.trim();
    if !rack.is_empty() {
        if let Err(why) = check(rack) {
            return refuse(StatusCode::BAD_REQUEST, why);
        }
    }
    let Some(client) = &inner.client else { return refuse(StatusCode::SERVICE_UNAVAILABLE, "no apiserver".to_string()) };
    match client.patch_merge(&format!("/api/v1/nodes/{name}"), &patch(rack), viewer.token.as_deref()).await {
        Ok((status, saved)) if status.is_success() => {
            // Seen at once by the page's re-read, not when the watch catches up.
            inner.store.observe("node", saved).await;
            let message = if rack.is_empty() { format!("{name}: rack cleared") } else { format!("{name}: rack {rack}") };
            Json(json!({"message": message, "rack": rack})).into_response()
        }
        Ok((status, body)) => {
            let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let said = body.get("message").and_then(Value::as_str).map_or_else(|| format!("the apiserver answered {status}"), str::to_string);
            refuse(code, said)
        }
        Err(e) => refuse(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rack_is_a_label_value() {
        for good in ["r1", "rack-3", "a.b_c", "7"] {
            assert!(check(good).is_ok(), "{good}");
        }
        for bad in ["rack 3", "-r", "r-", "", "a/b", &"x".repeat(64)] {
            assert!(check(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_patch_sets_or_takes_off() {
        assert_eq!(patch("r1")["metadata"]["labels"][LABEL], "r1");
        assert!(patch("")["metadata"]["labels"].get(LABEL).unwrap().is_null(), "null removes it in a merge patch");
    }

    #[test]
    fn who_may() {
        assert!(refusal(false, true).unwrap().contains("admin"));
        assert!(refusal(true, false).unwrap().contains("identity"), "never the console's credential");
        assert_eq!(refusal(true, true), None);
    }
}
