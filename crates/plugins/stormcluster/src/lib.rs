//! The stormcluster plugin: the Cluster page (#63).
//!
//! stormcluster (stormcluster#1) runs on every node at :9102 and serves the
//! cluster as a stormview feed: `system` (the cluster, or this node as a
//! single-node cluster), `member:<node>`, `peer:<node>` (discovered, not a
//! member) and `op:<id>` (the last five operations). Every card carries its
//! actions as body-less POSTs, so the feed is folded in like stormdrive's.
//! This plugin adds what a feed cannot:
//!
//! - **Who may act.** Reads are open; every write — form, join, promote,
//!   demote, drain, uncordon, split, resume — is `admin` only. Splitting a
//!   node out of the cluster or demoting a master is not an operator's
//!   everyday mistake to be allowed to make.
//! - **stormcluster's write token** (`token_file`), held here and added
//!   server-side; the browser never sees it or the node's address.
//! - **Answers a page can read.** A request sent to a node that does not
//!   coordinate it comes back as `{"coordinator", "response"}`; a refusal is
//!   `409 {"refused": [...]}`; a dry run's plan is bare step objects. The
//!   proxy unwraps the first (naming the coordinator), carries the reasons
//!   of the second as `error` too (so any row button says them, not
//!   "409 Conflict"), and gives each planned step the sentence stormcluster
//!   itself would print for it.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, NavSection, Viewer};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

pub const NAME: &str = "cluster";

struct Inner {
    base: String,
    token: Option<String>,
    feed: Arc<Feed>,
    client: reqwest::Client,
}

pub struct StormclusterPlugin {
    inner: Arc<Inner>,
}

impl StormclusterPlugin {
    /// `token` is stormcluster's write token, when it has one configured.
    pub fn new(url: &str, token: Option<String>) -> Self {
        let base = url.trim_end_matches('/').to_string();
        Self {
            inner: Arc::new(Inner {
                feed: Arc::new(Feed::new(&base, NAME, &format!("/api/plugins/{NAME}/proxy"))),
                base,
                token: token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                client: reqwest::Client::new(),
            }),
        }
    }
}

/// May this viewer do this to the cluster? Reads, yes; anything else only
/// as an administrator.
pub fn allowed(method: &Method, viewer: &Viewer) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) || viewer.has_role("admin")
}

/// The only upstream paths the proxy forwards: the operator's API.
/// `/api/v1/record` is between stormclusters — publishing or forgetting the
/// cluster record by hand would make a node believe it is somewhere it is
/// not — so the browser does not get to name it.
pub fn forwardable(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    if p.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    const SERVED: [&str; 8] = [
        "api/v1/health",
        "api/v1/self",
        "api/v1/peers",
        "api/v1/cluster",
        "api/v1/etcd",
        "api/v1/components",
        "api/v1/operations",
        "api/v1/members",
    ];
    SERVED.iter().any(|s| p == *s || p.starts_with(&format!("{s}/")))
}

#[async_trait]
impl ConsolePlugin for StormclusterPlugin {
    fn name(&self) -> &'static str {
        NAME
    }

    fn nav(&self) -> Vec<NavSection> {
        // First in Cluster: which machines the cluster is made of comes
        // before what the apiserver says about them.
        vec![NavSection::new("Cluster", 60).admin().item_at("Membership", "#/cluster", -1)]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/me", get(me))
            .route("/proxy/{*path}", any(proxy))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        self.inner.feed.components().await
    }

    async fn health(&self) -> Health {
        self.inner.feed.state().await.health
    }

    async fn detail(&self) -> String {
        let s = self.inner.feed.state().await;
        console_core::upstream::detail("stormcluster", &self.inner.base, &s.detail)
    }

    async fn run(&self, shutdown: CancellationToken) {
        self.inner.feed.run(self.inner.client.clone(), Duration::from_secs(3), shutdown).await;
    }
}

/// Whether to offer the buttons at all. The proxy enforces it either way.
async fn me(viewer: Viewer) -> Response {
    let admin = viewer.has_role("admin");
    Json(json!({
        "admin": admin,
        "why": if admin { "" } else {
            "forming, joining, promoting, demoting, draining and splitting are for administrators"
        },
    }))
    .into_response()
}

async fn proxy(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(path): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !forwardable(&path) {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("{path} is not served here")}))).into_response();
    }
    if !allowed(&method, &viewer) {
        let who = viewer.user.clone().unwrap_or_else(|| "this viewer".into());
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!(
                "{who} is not an administrator: changing what the cluster is made of needs the admin role"
            )})),
        )
            .into_response();
    }
    let write = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
    if write {
        let who = viewer.user.clone().unwrap_or_else(|| "admin".into());
        tracing::info!(user = %who, %method, path = %path, query = uri.query().unwrap_or(""), "cluster: acting through stormcluster");
    }
    let resp = console_core::proxy::forward_as(
        &inner.client,
        &inner.base,
        &method,
        &path,
        uri.query(),
        &headers,
        body,
        inner.token.as_deref(),
    )
    .await;
    if !write {
        return resp;
    }
    // A write's answer is small JSON; make it one a page can read.
    let status = resp.status();
    let bytes = match axum::body::to_bytes(resp.into_body(), 4 << 20).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("stormcluster's answer: {e}")}))).into_response(),
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v) => (status, Json(normalise(status, v))).into_response(),
        Err(_) if status == StatusCode::UNAUTHORIZED => (
            status,
            Json(json!({"error": "stormcluster wants a bearer token: set [stormcluster] token_file to its token_file"})),
        )
            .into_response(),
        Err(_) => (status, [(header::CONTENT_TYPE, "text/plain")], bytes).into_response(),
    }
}

/// One answer from stormcluster, shaped for the page (see the module doc).
pub fn normalise(status: StatusCode, v: Value) -> Value {
    // Forwarded to the coordinator: the coordinator's answer is the answer.
    let mut v = match v {
        Value::Object(mut o) if o.contains_key("coordinator") && o.contains_key("response") => {
            let coordinator = o.remove("coordinator").unwrap_or(Value::Null);
            match o.remove("response").unwrap_or(Value::Null) {
                Value::Object(mut inner) => {
                    inner.insert("coordinator".into(), coordinator);
                    Value::Object(inner)
                }
                Value::Null => json!({"coordinator": coordinator}),
                other => json!({"coordinator": coordinator, "response": other}),
            }
        }
        other => other,
    };
    let Some(o) = v.as_object_mut() else { return v };
    if let Some(reasons) = o.get("refused").and_then(Value::as_array) {
        if !o.contains_key("error") {
            let said: Vec<&str> = reasons.iter().filter_map(Value::as_str).collect();
            let mut e = format!("refused: {}", said.join("; "));
            if let Some(c) = o.get("coordinator").and_then(Value::as_str) {
                e.push_str(&format!(" (by {c}, which coordinates this)"));
            }
            o.insert("error".into(), Value::String(e));
        }
    }
    if let Some(Value::Array(steps)) = o.get_mut("plan").and_then(|p| p.get_mut("steps")) {
        for s in steps.iter_mut() {
            if let Value::Object(so) = s {
                if !so.contains_key("description") {
                    let d = describe(&Value::Object(so.clone()));
                    so.insert("description".into(), Value::String(d));
                }
            }
        }
    }
    // A failure stormcluster put in no words of its own.
    if !status.is_success() && !o.contains_key("error") {
        o.insert("error".into(), Value::String(format!("stormcluster answered {status}")));
    }
    v
}

/// What a planned step does, in stormcluster's own words (its
/// `Step::describe`, stormcluster 61777dd). A step this console does not
/// know is named with its fields, never dropped.
pub fn describe(step: &Value) -> String {
    let f = |k: &str| step.get(k).and_then(Value::as_str).unwrap_or("?").to_string();
    let node = f("node");
    match step.get("step").and_then(Value::as_str).unwrap_or("") {
        "checkJoinable" => format!("check {node} is a joinable SNO"),
        "seed" => format!("seed cluster {} on {node}", f("name")),
        "ensureEndpoint" => "ensure the API endpoint fronts the masters".into(),
        "enroll" => format!("issue a join token for {node}"),
        "etcdAddLearner" => format!("add {node} to fastetcd as a learner"),
        "nodeJoin" => format!("join {node} as {}", f("role")),
        "nodePromote" => format!("start the control plane on {node}"),
        "waitEtcdStarted" => format!("wait for {node}'s fastetcd to catch up"),
        "etcdPromote" => format!("promote {node} to a fastetcd voter"),
        "waitNodeReady" => format!("wait for Node {node} to be Ready"),
        "cordon" => format!("cordon {node}"),
        "uncordon" => format!("uncordon {node}"),
        "evict" => format!("evict the pods on {node}"),
        "etcdMoveLeaderOff" => format!("move fastetcd leadership off {node}"),
        "etcdRemove" => format!("remove {node} from fastetcd"),
        "nodeDemote" => format!("stop the control plane on {node}"),
        "revokeCerts" => format!("revoke {node}'s certificates"),
        "deleteNode" => format!("delete Node {node}"),
        "nodeLeave" => {
            let keep = step.get("keep_data").or_else(|| step.get("keepData")).and_then(Value::as_bool).unwrap_or(true);
            format!("revert {node} to SNO ({})", if keep { "keeping its data" } else { "wiping its data" })
        }
        "recordMember" => format!("record {node} as {}, {}", f("role"), f("state")),
        "removeMember" => format!("remove {node} from the record"),
        "forget" => format!("tell {node} it left the cluster"),
        "publish" => "publish the cluster record to every member".into(),
        other => {
            let mut parts: Vec<String> = step
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter(|(k, _)| k.as_str() != "step")
                        .map(|(k, v)| format!("{k} {}", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            parts.insert(0, if other.is_empty() { "a step".to_string() } else { other.to_string() });
            parts.join(" · ")
        }
    }
}

#[cfg(test)]
mod tests;
