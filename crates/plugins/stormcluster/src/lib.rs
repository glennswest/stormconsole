//! The stormcluster plugin: the Cluster page (#63, #88).
//!
//! stormcluster (stormcluster#1) runs on every node at :9102 and serves the
//! cluster as a stormview feed: `system` (the cluster, or this node as a
//! single-node cluster), `member:<node>`, `peer:<node>` (discovered, not a
//! member) and `op:<id>` (the last five operations). That feed is the
//! read-only view, folded in like stormdrive's.
//!
//! **Changes are objects** (#88, stormcluster#12): `cluster.storm.io`
//! `Cluster` and `ClusterMember`, written through the apiserver as the
//! viewer and reconciled by stormcluster — see [`objects`]. This plugin
//! watches both kinds and adds what neither the feed nor the objects give:
//!
//! - **A plan before anything is written.** stormcluster's dry run (`POST
//!   /api/v1/plan`) answers with the steps and a sentence for each, or the
//!   reasons it is refused; the page shows it and writes only on confirm. A
//!   release (deleting a member) erases the node and cannot be taken back.
//! - **Who may act is the apiserver's answer.** Every write carries the
//!   viewer's bearer, so RBAC on `cluster.storm.io` decides.
//! - **The proxy is read-only**, plus the dry run: stormcluster has no other
//!   writes, and `/api/v1/record` is between stormclusters.
//! - **:9102 over TLS** (#89) with the node CA and the console's client
//!   pair, and stormcluster's bearer added server-side for reads.

pub mod objects;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, patch, post};
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Feed, Health, NavSection, Viewer};
use plugin_kubernetes::{Client, KubeStore};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

pub const NAME: &str = "cluster";

struct Inner {
    base: String,
    token: Option<String>,
    feed: Arc<Feed>,
    /// :9102 is TLS only (stormcluster#5): the node CA and the console's
    /// client pair, followed as stormcert renews them (#89).
    tls: console_core::tls::Client,
    /// The apiserver the objects live on — this node's, which on an SNO is
    /// the one stormcluster reconciles and in a cluster is the cluster's.
    kube: Option<Client>,
    store: Arc<KubeStore>,
}

pub struct StormclusterPlugin {
    inner: Arc<Inner>,
}

impl StormclusterPlugin {
    /// `token` is stormcluster's bearer, when it has one configured.
    pub fn new(url: &str, token: Option<String>) -> Self {
        Self::with_tls(url, token, console_core::tls::TlsFiles::default())
    }

    /// The same, speaking TLS: trust only `files.ca`, present the pair
    /// (#89). No files is the plain client, for a stormcluster from before
    /// stormcluster#5.
    pub fn with_tls(url: &str, token: Option<String>, files: console_core::tls::TlsFiles) -> Self {
        let base = url.trim_end_matches('/').to_string();
        Self {
            inner: Arc::new(Inner {
                feed: Arc::new(Feed::new(&base, NAME, &format!("/api/plugins/{NAME}/proxy"))),
                base,
                token: token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
                // HTTP/1.1: stormcluster offers h2 in ALPN and drops an h2
                // client after the handshake (stormcluster#30).
                tls: console_core::tls::Client::http1("stormcluster", files),
                kube: None,
                store: Arc::new(KubeStore::with_kinds(objects::RESOURCES.len())),
            }),
        }
    }

    /// The apiserver connection the objects are written and watched through
    /// (#88). Called right after construction, before anything shares it.
    pub fn with_kube(mut self, conn: Option<Arc<console_core::apiserver::Conn>>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.kube = conn.map(Client::new);
        }
        self
    }
}

/// The only upstream paths the proxy forwards: stormcluster's reads.
/// `/api/v1/record` is between stormclusters, and there are no other
/// writes on :9102 any more (stormcluster#12) — the dry run has its own
/// route, [`plan`].
pub fn forwardable(path: &str) -> bool {
    let p = path.trim_start_matches('/');
    if p.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    const SERVED: [&str; 7] = [
        "api/v1/health",
        "api/v1/self",
        "api/v1/peers",
        "api/v1/cluster",
        "api/v1/etcd",
        "api/v1/components",
        "api/v1/operations",
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
            .route("/objects", get(objects_list))
            .route("/plan", post(plan))
            .route("/form", post(form))
            .route("/members", post(join))
            .route("/members/{node}", patch(change).delete(release))
            .route("/clusters/{name}", delete(dissolve))
            .route("/proxy/{*path}", any(proxy))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        self.inner.feed.components().await
    }

    async fn health(&self) -> Health {
        // A certificate file that cannot be used is the reason, whatever
        // the last poll saw.
        if self.inner.tls.error().is_some() {
            return Health::Error;
        }
        self.inner.feed.state().await.health
    }

    async fn detail(&self) -> String {
        let s = self.inner.feed.state().await;
        let d = match self.inner.tls.error() {
            Some(e) => format!("{e} · {}", s.detail),
            None => s.detail,
        };
        console_core::upstream::detail("stormcluster", &self.inner.base, &d)
    }

    async fn run(&self, shutdown: CancellationToken) {
        if let Some(client) = self.inner.kube.clone() {
            for spec in objects::RESOURCES {
                let (store, client, token) = (self.inner.store.clone(), client.clone(), shutdown.clone());
                tokio::spawn(async move { plugin_kubernetes::watch(client, spec, store, token).await });
            }
        }
        // The feed's own loop holds one client for good; this one picks up
        // a renewed pair (or one minted after start) before every poll.
        loop {
            self.inner.tls.refresh();
            self.inner.feed.poll(&self.inner.tls.get()).await;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

fn err(code: StatusCode, e: impl Into<String>) -> Response {
    (code, Json(json!({"error": e.into()}))).into_response()
}

/// Whether to offer the buttons. The apiserver decides either way; this
/// only spares a reader the sight of buttons that will be refused.
async fn me(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let write = viewer.may_write() && inner.kube.is_some();
    Json(json!({
        "write": write,
        // Kept for a page from before #88.
        "admin": write,
        "why": if inner.kube.is_none() {
            "the console has no apiserver to write cluster.storm.io objects to ([kubernetes])"
        } else if write {
            ""
        } else {
            "changing what the cluster is made of needs the operator role here, and the apiserver's leave on cluster.storm.io"
        },
    }))
    .into_response()
}

/// Both kinds as the page reads them, or why there are none.
async fn objects_list(State(inner): State<Arc<Inner>>) -> Response {
    if inner.kube.is_none() {
        return Json(json!({"installed": false, "reason": "no apiserver is configured ([kubernetes])", "clusters": [], "members": []}))
            .into_response();
    }
    let absent = inner.store.is_absent("scluster").await || inner.store.is_absent("smember").await;
    let mut clusters: Vec<Value> = inner.store.kind("scluster").await.values().map(objects::summary).collect();
    let mut members: Vec<Value> = inner.store.kind("smember").await.values().map(objects::summary).collect();
    let by_name = |a: &Value, b: &Value| a["name"].as_str().cmp(&b["name"].as_str());
    clusters.sort_by(by_name);
    members.sort_by(by_name);
    Json(json!({
        "installed": !absent,
        "reason": if absent {
            "the apiserver does not serve cluster.storm.io yet: stormcluster installs its CRDs on the apiserver it reconciles (kube.install_crds), or they ship in stormcluster's deploy/crd.yaml"
        } else { "" },
        "clusters": clusters,
        "members": members,
    }))
    .into_response()
}

/// stormcluster's dry run: what a request would do, or why it is refused.
/// Never starts anything, so any reader may ask.
async fn plan(State(inner): State<Arc<Inner>>, Json(req): Json<Value>) -> Response {
    let (status, v) = ask_plan(&inner, &req).await;
    (status, Json(v)).into_response()
}

async fn ask_plan(inner: &Inner, req: &Value) -> (StatusCode, Value) {
    inner.tls.refresh();
    let mut r = inner.tls.get().post(format!("{}/api/v1/plan", inner.base)).json(req).timeout(Duration::from_secs(30));
    if let Some(t) = &inner.token {
        r = r.bearer_auth(t);
    }
    match r.send().await {
        Ok(resp) => {
            let code = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let ok = resp.status().is_success();
            let v: Value = resp.json().await.unwrap_or_else(|_| json!({}));
            let v = objects::plan_answer(ok, v);
            (if ok || code == StatusCode::CONFLICT { code } else { StatusCode::BAD_GATEWAY }, v)
        }
        Err(e) => (StatusCode::BAD_GATEWAY, json!({"error": format!("stormcluster could not be asked for a plan: {e}")})),
    }
}

/// The apiserver said no: its status and its words.
fn from_apiserver(status: reqwest::StatusCode, body: &Value, what: &str) -> Response {
    let code = match status.as_u16() {
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        404 => StatusCode::NOT_FOUND,
        409 => StatusCode::CONFLICT,
        400 | 422 => StatusCode::BAD_REQUEST,
        _ => StatusCode::BAD_GATEWAY,
    };
    let said = body.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()));
    err(code, format!("{what}: {said}"))
}

/// Create objects in order, as the viewer. The first refusal stops it and
/// names what was already written, since a half-written form is a state
/// somebody has to see.
async fn create_all(inner: &Inner, viewer: &Viewer, objs: &[Value]) -> Result<Vec<String>, Response> {
    let Some(kube) = &inner.kube else { return Err(err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver")) };
    let mut made = Vec::new();
    for o in objs {
        let kind = o["kind"].as_str().unwrap_or("");
        let name = o.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
        let plural = if kind == "Cluster" { "clusters" } else { "clustermembers" };
        match kube.post_json_as(&format!("{}/{plural}", objects::API), o, viewer.token.as_deref()).await {
            Ok((s, _)) if s.is_success() => made.push(format!("{kind} {name}")),
            Ok((s, b)) => {
                let mut what = format!("{kind} {name}");
                if !made.is_empty() {
                    what = format!("{what} (already written: {})", made.join(", "));
                }
                return Err(from_apiserver(s, &b, &what));
            }
            Err(e) => return Err(err(StatusCode::BAD_GATEWAY, format!("{kind} {name}: {e}"))),
        }
    }
    Ok(made)
}

fn audit(viewer: &Viewer, what: &str) {
    let who = viewer.user.clone().unwrap_or_else(|| "anonymous".into());
    tracing::info!(user = %who, "cluster: {what}");
}

/// The node whose apiserver this is: stormcluster's `self`.
async fn self_node(inner: &Inner) -> Option<String> {
    inner.tls.refresh();
    let mut r = inner.tls.get().get(format!("{}/api/v1/self", inner.base)).timeout(Duration::from_secs(5));
    if let Some(t) = &inner.token {
        r = r.bearer_auth(t);
    }
    let v: Value = r.send().await.ok()?.json().await.ok()?;
    v.get("node").and_then(Value::as_str).map(str::to_string)
}

/// Form a cluster seeded on this node: the members, then the `Cluster`.
async fn form(State(inner): State<Arc<Inner>>, viewer: Viewer, Json(f): Json<objects::Form>) -> Response {
    let Some(seed) = self_node(&inner).await else {
        return err(StatusCode::BAD_GATEWAY, "stormcluster did not say which node this is (GET /api/v1/self)");
    };
    let objs = match objects::form_objects(&f, &seed) {
        Ok(o) => o,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match create_all(&inner, &viewer, &objs).await {
        Ok(made) => {
            audit(&viewer, &format!("form {} seeded on {seed}: {}", f.name.trim(), made.join(", ")));
            Json(json!({"message": format!(
                "wrote {}: stormcluster forms {} seeded on {seed} — its progress is the Cluster's status",
                made.join(", "), f.name.trim()
            )}))
            .into_response()
        }
        Err(r) => r,
    }
}

/// Join nodes: one `ClusterMember` each.
async fn join(State(inner): State<Arc<Inner>>, viewer: Viewer, Json(j): Json<objects::Join>) -> Response {
    let objs = match objects::join_objects(&j) {
        Ok(o) => o,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    match create_all(&inner, &viewer, &objs).await {
        Ok(made) => {
            audit(&viewer, &format!("join as {}: {}", j.role, made.join(", ")));
            Json(json!({"message": format!("wrote {}: stormcluster joins them as {}s", made.join(", "), j.role)})).into_response()
        }
        Err(r) => r,
    }
}

/// Promote, demote, drain, uncordon, storage: a merge patch of the spec.
async fn change(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(node): Path<String>,
    Json(c): Json<objects::Change>,
) -> Response {
    let Some(kube) = &inner.kube else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    let body = match objects::change_patch(&c) {
        Ok(b) => b,
        Err(e) => return err(StatusCode::BAD_REQUEST, e),
    };
    let path = format!("{}/clustermembers/{node}", objects::API);
    match kube.patch_merge(&path, &body, viewer.token.as_deref()).await {
        Ok((s, o)) if s.is_success() => {
            inner.store.observe("smember", o).await;
            audit(&viewer, &format!("ClusterMember {node} spec {}", body["spec"]));
            Json(json!({"message": format!("ClusterMember {node}: spec {} written; stormcluster acts on it", body["spec"])}))
                .into_response()
        }
        Ok((s, b)) => from_apiserver(s, &b, &format!("ClusterMember {node}")),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// Release a node: delete its `ClusterMember`. stormcluster drains it,
/// erases its data and makes it a new SNO; the finalizer keeps the object
/// until that is done.
async fn release(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(node): Path<String>) -> Response {
    delete_object(&inner, &viewer, "clustermembers", "ClusterMember", &node, &format!(
        "ClusterMember {node} deleted: stormcluster releases {node} — drained, its data erased, a new SNO"
    ))
    .await
}

/// Dissolve the cluster: delete its `Cluster`. Every member goes back to
/// SNO, workers first, the seed last.
async fn dissolve(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    delete_object(&inner, &viewer, "clusters", "Cluster", &name, &format!(
        "Cluster {name} deleted: stormcluster releases every member, workers first and the seed last"
    ))
    .await
}

async fn delete_object(inner: &Inner, viewer: &Viewer, plural: &str, kind: &str, name: &str, done: &str) -> Response {
    let Some(kube) = &inner.kube else { return err(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    match kube.delete(&format!("{}/{plural}/{name}", objects::API), viewer.token.as_deref()).await {
        Ok(s) if s.is_success() => {
            audit(viewer, &format!("{kind} {name} deleted"));
            Json(json!({"message": done})).into_response()
        }
        Ok(s) => from_apiserver(s, &Value::Null, &format!("{kind} {name}")),
        Err(e) => err(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// stormcluster's reads, with its bearer and over its TLS.
async fn proxy(
    State(inner): State<Arc<Inner>>,
    Path(path): Path<String>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !forwardable(&path) {
        return err(StatusCode::NOT_FOUND, format!("{path} is not served here"));
    }
    if !matches!(method, Method::GET | Method::HEAD) {
        return err(
            StatusCode::METHOD_NOT_ALLOWED,
            "stormcluster takes no writes over :9102: a change is a cluster.storm.io object (stormcluster#12)",
        );
    }
    inner.tls.refresh();
    console_core::proxy::forward_as(
        &inner.tls.get(),
        &inner.base,
        &method,
        &path,
        uri.query(),
        &headers,
        body,
        inner.token.as_deref(),
    )
    .await
}

#[cfg(test)]
mod tests;
