//! The kubernetes plugin: rustkube apiserver views through a watch-backed
//! cache. rustkube only — this console has no other orchestrator.
//!
//! One list+watch loop per resource kind feeds a shared store; the
//! components mapping renders a consistent snapshot with health derived
//! from the same conditions kubectl reads. Actions surface as POST routes
//! under /api/plugins/k8s so any stormview renderer can wire them.

pub mod apply;
pub mod authz;
pub mod cache;
pub mod client;
mod components;
pub mod network;
pub mod objevents;
pub mod projects;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use console_core::{
    Access, ComponentSummary, ConsolePlugin, Creator, Events, Health, NavSection, Probe, Viewer,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use cache::{watch_resource, Store, RESOURCES};
use client::RkClient;

/// How many events one object's box holds.
///
/// A box is for the last thing that happened, not an archive — the Events
/// page is the archive. Twenty is more than fits on a screen and fewer
/// than makes a table row unscrollable.
const EVENT_CAP: usize = 20;

/// How many the bottom dock holds. Larger, because it is a ticker across
/// the whole cluster and the thing you are looking for scrolled past
/// while you were reading the last one.
const RECENT_CAP: usize = 60;

/// The Cilium agent's health server (`cilium-health-api`), bound to
/// loopback on every node that runs the agent.
const CILIUM_AGENT_HEALTH: &str = "http://127.0.0.1:9879/healthz";

// A VM on this platform is a kube object — a KubeVirt
// `VirtualMachineInstance` the kubelet reconciles (stormvm docs/kube.md) —
// so the VM plugin watches the apiserver with the same client and the
// same list+watch loop rather than growing a second one.
pub use authz::NamespaceAccess;
pub use cache::{watch_resource as watch, ResourceSpec, Store as KubeStore};
pub use client::{RkClient as Client, RkError};

struct Inner {
    server: Option<String>,
    client: Option<RkClient>,
    probe: Option<Probe>,
    /// The Cilium agent's own health server. It is localhost-bound by
    /// design, and the console golden shares the host network, so this
    /// works exactly when the console runs on the node — which is the
    /// only place the answer means anything (#4).
    agent: Probe,
    store: Arc<Store>,
    http: reqwest::Client,
    access: Arc<authz::NamespaceAccess>,
}

pub struct KubernetesPlugin {
    inner: Arc<Inner>,
}

impl KubernetesPlugin {
    /// The shared namespace-authorization answer, so the VM plugin asks
    /// the same question once rather than a second time.
    pub fn namespace_access(&self) -> Arc<authz::NamespaceAccess> {
        self.inner.access.clone()
    }

    pub fn new(server: Option<String>, token: Option<String>, insecure: bool) -> Self {
        let client = server.as_ref().map(|s| RkClient::new(s, token.as_deref(), insecure));
        let probe = client.as_ref().map(|c| Probe::new(format!("{}/version", c.base())));
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure)
            .build()
            .expect("reqwest client");
        let store = Arc::new(Store::default());
        let server_for_access = server.clone();
        Self {
            inner: Arc::new(Inner {
                server,
                client,
                probe,
                agent: Probe::new(CILIUM_AGENT_HEALTH),
                store: store.clone(),
                http: http.clone(),
                access: authz::NamespaceAccess::new(server_for_access, http, store),
            }),
        }
    }
}

#[async_trait]
impl ConsolePlugin for KubernetesPlugin {
    fn name(&self) -> &'static str {
        "k8s"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![
            // Projects are the top of the console (#28): where somebody's
            // own work lives, and the first thing they choose.
            NavSection::new("Home", 0).item("Overview", "#/").item("Projects", "#/projects"),
            // Numbered with gaps, because the vm plugin puts virtual
            // machines in this section too and there is no integer
            // between 0 and 1.
            NavSection::new("Workloads", 10)
                .item_at("Pods", "#/k8s/pod", 0)
                .item_at("Deployments", "#/k8s/deploy", 20)
                .item_at("StatefulSets", "#/k8s/sts", 30)
                .item_at("DaemonSets", "#/k8s/ds", 40)
                .item_at("Jobs", "#/k8s/job", 50)
                .item_at("CronJobs", "#/k8s/cronjob", 60),
            // Diagnosis, all of it. A person reaching for Cilium
            // identities, endpoints and clusterwide policies is asking why
            // something cannot be reached, not shipping a workload (#16).
            NavSection::new("Networking", 25)
                .admin()
                .item("Services", "#/k8s/svc")
                .item("Network policies", "#/k8s/netpol")
                .item("Cilium policies", "#/k8s/cnp")
                .item("Clusterwide policies", "#/k8s/ccnp")
                .item("Cilium endpoints", "#/k8s/cep")
                .item("Cilium nodes", "#/k8s/cn")
                .item("Identities", "#/k8s/cid"),
            NavSection::new("Storage", 40).item("PVCs", "#/k8s/pvc"),
            NavSection::new("Observe", 30).admin().item("Events", "#/k8s/events"),
            // What no project owns (#28): the cluster's own objects, and
            // every namespace including the system's, kept apart from
            // anybody's work.
            NavSection::new("Cluster", 60)
                .admin()
                .item("Nodes", "#/k8s/node")
                .item("Namespaces", "#/k8s/ns")
                .item("Persistent volumes", "#/k8s/pv")
                .item("Storage classes", "#/k8s/sc")
                .item("Custom resources", "#/k8s/crd")
                .item("Cluster roles", "#/k8s/crole"),
        ]
    }

    fn creators(&self) -> Vec<Creator> {
        apply::creators()
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/kinds", get(kinds))
            .route("/pods/{ns}/{name}/delete", post(delete_pod))
            .route("/events", get(events))
            .route("/namespaces/{ns}", get(namespace_detail))
            .route("/object/{kind}/{*key}", get(object).put(edit_object))
            .route("/apply", post(apply_yaml))
            .route("/projects", get(projects::list).post(projects::create))
            .route("/projects/{name}", get(projects::detail).delete(projects::remove))
            .route("/projects/{name}/members", post(projects::add_member))
            .route("/projects/{name}/members/{binding}", delete(projects::remove_member))
            .route("/projects/{name}/isolate", post(projects::isolate).delete(projects::unisolate))
            .route("/raw/{*path}", delete(raw_delete))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let inner = &self.inner;
        let (health, detail) = apiserver_state(inner).await;
        let mut out = vec![ComponentSummary {
            id: "k8s:apiserver".into(),
            kind: "apiserver".into(),
            label: "rustkube".into(),
            health,
            detail,
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        }];
        let agent = inner.agent.state().await;
        out.extend(components::map(&inner.store.snapshot().await, Some((agent.health, agent.detail))));
        out
    }

    async fn health(&self) -> Health {
        apiserver_state(&self.inner).await.0
    }

    async fn detail(&self) -> String {
        match &self.inner.server {
            Some(s) => console_core::upstream::detail("rustkube", s, ""),
            None => "no rustkube endpoint configured".to_string(),
        }
    }

    /// What this viewer may see, asked of the apiserver as them.
    ///
    /// With no credential to carry there is no identity to authorize
    /// against, so the honest answer is `Unrestricted` — and the console
    /// says *that* (`/api/v1/console/access` reports `identified: false`)
    /// rather than implying a check it is not doing.
    /// What happened to one of this plugin's objects.
    ///
    /// Read as the console and filtered to this viewer's namespaces, the
    /// same way the Events page is — the cache's own credential is what
    /// can see events at all, and the filter is what stops a hidden
    /// namespace's activity leaking through a per-object question.
    async fn events(&self, viewer: &Viewer, id: &str) -> Option<Events> {
        let inner = &self.inner;
        // A container first: its events belong to its pod and are
        // narrowed by field path, so the id shape has to be checked
        // before the general one, which would not match it anyway.
        let (kind, ns, name, container) = match objevents::container_of(id) {
            Some((ns, pod, c)) => ("Pod", ns, pod, Some(c)),
            None => {
                let (k, ns, n) = objevents::object_of(id)?;
                (k, ns, n, None)
            }
        };
        let client = inner.client.as_ref()?;
        if !ns.is_empty() {
            if let Some((hidden, _)) = inner.access.hidden(viewer).await {
                if hidden.contains(&ns) {
                    // The same answer an absent object gets: a per-object
                    // route is not a way around the filtered feed.
                    return Some(Events::none(format!("no {kind} {ns}/{name}")));
                }
            }
        }
        let list = match client.get(&objevents::list_path(&ns)).await {
            Ok(l) => l,
            Err(e) => {
                return Some(Events::none(format!(
                    "the apiserver did not answer for events: {e}"
                )))
            }
        };
        let items: Vec<_> = list
            .get("items")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|e| objevents::about(e, kind, &ns, &name))
                    .filter(|e| match &container {
                        Some(c) => objevents::about_container(e, c),
                        None => true,
                    })
                    .map(objevents::event)
                    .collect()
            })
            .unwrap_or_default();
        Some(Events::of(items).newest(EVENT_CAP))
    }

    /// Recent activity across the cluster, for the dock.
    ///
    /// The same read as the Events page and filtered the same way — this
    /// is a more convenient window onto it, not a second source that can
    /// disagree with it.
    async fn recent_events(&self, viewer: &Viewer) -> Option<Events> {
        let client = self.inner.client.as_ref()?;
        let hidden = self.inner.access.hidden(viewer).await.map(|(h, _)| h);
        let list = match client.get("/api/v1/events").await {
            Ok(l) => l,
            Err(e) => return Some(Events::none(format!("the apiserver did not answer: {e}"))),
        };
        let items: Vec<_> = list
            .get("items")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|e| match (&hidden, e.pointer("/involvedObject/namespace").and_then(Value::as_str)) {
                        (Some(h), Some(ns)) if !ns.is_empty() => !h.contains(ns),
                        _ => true,
                    })
                    .map(|e| {
                        let mut ev = objevents::event(e);
                        // The dock is a ticker: a line has to say what it
                        // is about, because there is no page around it to
                        // supply that.
                        ev.source = format!(
                            "{}/{}",
                            e.pointer("/involvedObject/kind").and_then(Value::as_str).unwrap_or(""),
                            e.pointer("/involvedObject/name").and_then(Value::as_str).unwrap_or("")
                        );
                        ev
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Events::of(items).newest(RECENT_CAP))
    }

    async fn access(&self, viewer: &Viewer) -> Access {
        let Some((hidden, note)) = self.inner.access.hidden(viewer).await else {
            return Access::Unrestricted;
        };
        let count = hidden.len();
        Access::limited(move |id| visible_to(id, &hidden), count, note)
    }

    async fn run(&self, shutdown: CancellationToken) {
        let Some(client) = self.inner.client.clone() else {
            shutdown.cancelled().await;
            return;
        };
        for spec in RESOURCES {
            let store = self.inner.store.clone();
            let client = client.clone();
            let token = shutdown.clone();
            tokio::spawn(async move {
                watch_resource(client, spec, store, token).await;
            });
        }
        // The agent's health server answers on loopback whether or not
        // the apiserver does, so it is probed either way.
        {
            let inner = self.inner.clone();
            let token = shutdown.clone();
            tokio::spawn(async move {
                inner.agent.run(inner.http.clone(), Duration::from_secs(15), token).await;
            });
        }
        if let Some(probe) = &self.inner.probe {
            probe.run(self.inner.http.clone(), Duration::from_secs(10), shutdown).await;
        } else {
            shutdown.cancelled().await;
        }
    }
}

/// A namespace this viewer may not see does not exist as far as they are
/// concerned. Returning 403 would confirm it is there, so a hidden
/// namespace answers exactly as an absent one does — which is also what
/// keeps a plugin route from being a way around the filtered feed.
async fn refuse_hidden(inner: &Inner, viewer: &Viewer, ns: &str) -> Option<Response> {
    let (hidden, _) = inner.access.hidden(viewer).await?;
    hidden.contains(ns).then(|| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("no namespace {ns}")})),
        )
            .into_response()
    })
}

/// Is this component id outside every hidden namespace?
///
/// Ids are `k8s:<kind>:<key>`, and the key is `ns/name` for a namespaced
/// kind and a bare name otherwise — so the namespace is read off the id
/// and no object has to be consulted. A kind this console does not know
/// is treated as cluster-scoped and stays visible; guessing it namespaced
/// would hide it entirely on no evidence.
fn visible_to(id: &str, hidden: &std::collections::HashSet<String>) -> bool {
    let Some(rest) = id.strip_prefix("k8s:") else { return true };
    let Some((kind, key)) = rest.split_once(':') else { return true };
    if kind == "ns" {
        return !hidden.contains(key);
    }
    if !cache::is_namespaced(kind) {
        return true;
    }
    match key.split_once('/') {
        Some((ns, _)) => !hidden.contains(ns),
        None => true,
    }
}

/// The kind catalogue: what this plugin watches, what to call it, and
/// whether the namespace selector applies. One declaration, in
/// `cache::RESOURCES`, instead of a copy of it in every view (#5).
async fn kinds() -> Response {
    Json(cache::catalogue()).into_response()
}

/// Everything a namespace page needs, in one answer (#6): the object
/// itself, an inventory whose every count is a link, its quotas and limit
/// ranges, and its events.
async fn namespace_detail(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(ns): Path<String>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let Some(object) = inner.store.object("ns", &ns).await else {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no namespace {ns}")})))
            .into_response();
    };
    let mut inventory = Vec::new();
    for spec in RESOURCES.iter().filter(|r| r.inventory) {
        let count = inner.store.count_in(spec.kind, &ns).await;
        inventory.push(json!({
            "kind": spec.kind,
            "title": spec.title,
            "count": count,
            "href": format!("#/k8s/{}?ns={}", spec.kind, ns),
        }));
    }
    let quotas: Vec<Value> = objects_in(&inner, "quota", &ns).await;
    let limits: Vec<Value> = objects_in(&inner, "limits", &ns).await;
    let events = match &inner.client {
        Some(c) => c
            .get_as(&format!("/api/v1/namespaces/{ns}/events"), viewer.token.as_deref())
            .await
            .ok()
            .and_then(|l| l.get("items").and_then(Value::as_array).cloned())
            .map(|items| items.iter().map(event_row).collect::<Vec<_>>())
            .unwrap_or_default(),
        None => vec![],
    };
    Json(json!({
        "name": ns,
        "phase": object.pointer("/status/phase").and_then(Value::as_str).unwrap_or("Active"),
        "created": object.pointer("/metadata/creationTimestamp").and_then(Value::as_str).unwrap_or(""),
        "labels": object.pointer("/metadata/labels").cloned().unwrap_or(json!({})),
        "annotations": annotations_of(&object),
        "inventory": inventory,
        "quotas": quotas.iter().map(quota_row).collect::<Vec<_>>(),
        "limitRanges": limits.iter().map(limit_row).collect::<Vec<_>>(),
        "events": events,
        "yaml": to_yaml(&object),
    }))
    .into_response()
}

async fn objects_in(inner: &Inner, kind: &str, ns: &str) -> Vec<Value> {
    let prefix = format!("{ns}/");
    let mut out: Vec<Value> = inner
        .store
        .kind(kind)
        .await
        .into_iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| v)
        .collect();
    out.sort_by_key(|v| {
        v.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("").to_string()
    });
    out
}

/// A namespace's annotations, minus the one that is not an annotation.
///
/// `kubectl.kubernetes.io/last-applied-configuration` is the entire object
/// as a JSON string — kilobytes of it — and rendering it in a card of
/// key/value pairs buries every annotation that means something. It is
/// dropped here rather than in the view, because every consumer of this
/// payload would otherwise have to know to drop it.
///
/// Annotations are where the descriptive fields live: OpenShift puts a
/// project's requester and description in `openshift.io/requester` and
/// `openshift.io/description`, and a namespace's own labels never carried
/// either. They were fetched with the object and thrown away.
fn annotations_of(object: &Value) -> Value {
    const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";
    match object.pointer("/metadata/annotations").and_then(Value::as_object) {
        Some(map) => {
            let kept: serde_json::Map<String, Value> =
                map.iter().filter(|(k, _)| k.as_str() != LAST_APPLIED).map(|(k, v)| (k.clone(), v.clone())).collect();
            Value::Object(kept)
        }
        None => json!({}),
    }
}

/// A quota as a page can print it: every resource with its used and hard
/// figures side by side.
fn quota_row(q: &Value) -> Value {
    let name = q.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
    let hard = q.pointer("/status/hard").or_else(|| q.pointer("/spec/hard"));
    let used = q.pointer("/status/used");
    let mut resources = Vec::new();
    if let Some(map) = hard.and_then(Value::as_object) {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for k in keys {
            resources.push(json!({
                "resource": k,
                "hard": map.get(k).and_then(Value::as_str).unwrap_or(""),
                "used": used.and_then(|u| u.get(k)).and_then(Value::as_str).unwrap_or(""),
            }));
        }
    }
    json!({"name": name, "resources": resources})
}

fn limit_row(l: &Value) -> Value {
    let name = l.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
    let limits = l
        .pointer("/spec/limits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    json!({"name": name, "limits": limits})
}

/// One object as YAML — the thing every OpenShift resource page has a tab
/// for. Served from the watch cache, so it is the same object the list
/// row was drawn from.
async fn object(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((kind, key)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_key(&inner, &viewer, &kind, &key).await {
        return refusal;
    }
    let editable = cache::spec(&kind).is_some();
    match inner.store.object(&kind, &key).await {
        Some(v) => Json(json!({
            "kind": kind,
            "key": key,
            "object": v,
            "yaml": to_yaml(&v),
            "editable": editable,
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("no {kind} {key} in the cache")})),
        )
            .into_response(),
    }
}

/// Save an edited object. The body is the YAML from the editor; it goes
/// back to the apiserver as a replace, so the `resourceVersion` it was
/// loaded with is the concurrency guard — an edit of an object somebody
/// else has changed since is refused with a 409 rather than silently
/// overwriting them. Create exists (`/apply`); this is the other half
/// (#4).
async fn edit_object(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((kind, key)): Path<(String, String)>,
    body: String,
) -> Response {
    if let Some(refusal) = refuse_key(&inner, &viewer, &kind, &key).await {
        return refusal;
    }
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    let Some(spec) = cache::spec(&kind) else {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": format!("unknown kind {kind}")})))
            .into_response();
    };
    let docs = match apply::parse_documents(&body) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let [doc] = &docs[..] else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "an edit is one document — use Import YAML for several"})),
        )
            .into_response();
    };
    // Editing must not become a rename: a name change here would create a
    // second object and leave the first, which is not what "save" means.
    let name = doc.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
    let expected = key.rsplit('/').next().unwrap_or(&key);
    if name != expected {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("this edits {expected}; the document names {name}. Rename with Import YAML and delete the old one.")})),
        )
            .into_response();
    }
    let path = spec.object_path(&key);
    match client.put_json(&path, doc, viewer.token.as_deref()).await {
        Ok((status, _)) if status.is_success() => {
            Json(json!({"message": format!("{kind} {key} saved")})).into_response()
        }
        Ok((status, resp)) => {
            let msg = resp
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()));
            let code = if status.as_u16() == 409 { StatusCode::CONFLICT } else { StatusCode::BAD_GATEWAY };
            (code, Json(json!({"error": msg}))).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// The object as YAML, with the fields nobody reads taken out. A page of
/// `managedFields` is not a resource; it is what buries one.
pub fn to_yaml(v: &Value) -> String {
    let mut v = v.clone();
    if let Some(meta) = v.pointer_mut("/metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
    }
    serde_yaml::to_string(&v).unwrap_or_else(|e| format!("# could not render: {e}\n"))
}

async fn apiserver_state(inner: &Inner) -> (Health, String) {
    match &inner.probe {
        Some(p) => {
            let s = p.state().await;
            let (synced, total) = inner.store.synced_kinds().await;
            let health = match s.health {
                Health::Ok if synced == total => Health::Ok,
                Health::Ok => Health::Warn,
                other => other,
            };
            (health, format!("{} · {synced}/{total} kinds synced", s.detail))
        }
        None => (Health::Idle, "no rustkube endpoint configured".to_string()),
    }
}

async fn delete_pod(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    match client
        .delete(&format!("/api/v1/namespaces/{ns}/pods/{name}"), viewer.token.as_deref())
        .await
    {
        Ok(status) if status.is_success() => Json(json!({"deleted": format!("{ns}/{name}")}))
            .into_response(),
        Ok(status) => (StatusCode::BAD_GATEWAY, Json(json!({"error": status.as_u16()})))
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()})))
            .into_response(),
    }
}

/// DELETE any apiserver path — what a component's delete action calls.
/// Only `/api/…` and `/apis/…` are forwarded.
async fn raw_delete(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path(path): Path<String>,
) -> Response {
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    let path = format!("/{}", path.trim_start_matches('/'));
    if !(path.starts_with("/api/") || path.starts_with("/apis/")) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "not an apiserver path"}))).into_response();
    }
    if let Some(ns) = namespace_in_path(&path) {
        if let Some(refusal) = refuse_hidden(&inner, &viewer, ns).await {
            return refusal;
        }
    }
    match client.delete(&path, viewer.token.as_deref()).await {
        Ok(status) if status.is_success() => Json(json!({"deleted": path})).into_response(),
        Ok(status) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("apiserver returned {}", status.as_u16())})))
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Import YAML, OpenShift-style: one or more documents, each created in
/// its collection. Every document gets a line in the result; a failure on
/// one does not stop the rest.
#[derive(serde::Deserialize, Default)]
struct ApplyQuery {
    /// The project the create dialog chose: where a namespaced document
    /// that names no namespace goes (#28). Never `default` by omission.
    #[serde(default)]
    project: Option<String>,
}

async fn apply_yaml(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Query(q): Query<ApplyQuery>,
    body: String,
) -> Response {
    let project = q.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    let docs = match apply::parse_documents(&body) {
        Ok(d) => d,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    if docs.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "no documents"}))).into_response();
    }
    let mut results = Vec::new();
    let mut failed = false;
    for doc in docs {
        let (kind, name, path) = match apply::target(&doc, project) {
            Ok(t) => t,
            Err(e) => {
                failed = true;
                results.push(json!({"error": e}));
                continue;
            }
        };
        if let Some(ns) = namespace_in_path(&path) {
            // Somebody's workload beside the system's own objects is how
            // test1 and test2 ended up in `default` (#28). An administrator
            // importing into kube-system on purpose names it in the YAML,
            // and may. First, because it is the more useful answer — and a
            // system namespace's existence is no secret.
            if inner.access.is_system(ns) && !viewer.has_role("admin") {
                failed = true;
                results.push(json!({"kind": kind, "name": name, "error": format!(
                    "{ns} is a system namespace: workloads go in a project. Choose one, or create one"
                )}));
                continue;
            }
            if refuse_hidden(&inner, &viewer, ns).await.is_some() {
                failed = true;
                results.push(json!({"kind": kind, "name": name, "error": format!("no namespace {ns}")}));
                continue;
            }
        }
        match client.post_json_as(&path, &doc, viewer.token.as_deref()).await {
            Ok((status, _)) if status.is_success() => {
                results.push(json!({"kind": kind, "name": name, "status": status.as_u16(), "created": true}))
            }
            Ok((status, resp)) => {
                failed = true;
                let msg = resp.get("message").and_then(Value::as_str).unwrap_or("").to_string();
                results.push(json!({"kind": kind, "name": name, "status": status.as_u16(), "error": msg}))
            }
            Err(e) => {
                failed = true;
                results.push(json!({"kind": kind, "name": name, "error": e.to_string()}))
            }
        }
    }
    let status = if failed { StatusCode::MULTI_STATUS } else { StatusCode::CREATED };
    let summary = results
        .iter()
        .map(|r| match (r.get("kind"), r.get("name"), r.get("error")) {
            (Some(k), Some(n), None) => format!("{} {} created", k.as_str().unwrap_or(""), n.as_str().unwrap_or("")),
            (Some(k), Some(n), Some(e)) => format!("{} {}: {}", k.as_str().unwrap_or(""), n.as_str().unwrap_or(""), e.as_str().unwrap_or("")),
            (_, _, Some(e)) => e.as_str().unwrap_or("").to_string(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("; ");
    (status, Json(json!({"results": results, "error": if failed { Some(summary.clone()) } else { None }, "message": summary})))
        .into_response()
}

/// The namespace an apiserver path acts in, if it has one:
/// `/api/v1/namespaces/<ns>/pods/web` → `kube-system`. A path with no
/// `/namespaces/` segment is cluster-scoped and has none.
fn namespace_in_path(path: &str) -> Option<&str> {
    let (_, rest) = path.split_once("/namespaces/")?;
    let ns = rest.split('/').next()?;
    (!ns.is_empty()).then_some(ns)
}

/// A key from the store (`ns/name`, or a bare name) in a hidden namespace?
async fn refuse_key(inner: &Inner, viewer: &Viewer, kind: &str, key: &str) -> Option<Response> {
    let ns = if kind == "ns" {
        key
    } else if cache::is_namespaced(kind) {
        key.split_once('/')?.0
    } else {
        return None;
    };
    refuse_hidden(inner, viewer, ns).await
}

#[derive(serde::Deserialize)]
struct EventsQuery {
    namespace: Option<String>,
}

async fn events(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Query(q): Query<EventsQuery>,
) -> Response {
    let Some(client) = &inner.client else {
        return Json(json!([])).into_response();
    };
    let path = match &q.namespace {
        Some(ns) if !ns.is_empty() => {
            if let Some(refusal) = refuse_hidden(&inner, &viewer, ns).await {
                return refusal;
            }
            format!("/api/v1/namespaces/{ns}/events")
        }
        _ => "/api/v1/events".to_string(),
    };
    // Cluster-wide events are read as the console (the cache's own
    // credential) but filtered to what this viewer may see, so a hidden
    // namespace's activity does not leak through the Events page.
    let hidden = inner.access.hidden(&viewer).await.map(|(h, _)| h);
    match client.get(&path).await {
        Ok(list) => {
            let rows: Vec<Value> = list
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(event_row)
                        .filter(|r| match (&hidden, r.get("namespace").and_then(Value::as_str)) {
                            (Some(h), Some(ns)) if !ns.is_empty() => !h.contains(ns),
                            _ => true,
                        })
                        .collect()
                })
                .unwrap_or_default();
            Json(rows).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()})))
            .into_response(),
    }
}

fn event_row(e: &Value) -> Value {
    let g = |p: &str| e.pointer(p).and_then(Value::as_str).unwrap_or("");
    json!({
        "time": e.pointer("/lastTimestamp").and_then(Value::as_str)
            .or_else(|| e.pointer("/eventTime").and_then(Value::as_str))
            .or_else(|| e.pointer("/metadata/creationTimestamp").and_then(Value::as_str))
            .unwrap_or(""),
        "type": g("/type"),
        "reason": g("/reason"),
        "object": format!("{}/{}", g("/involvedObject/kind"), g("/involvedObject/name")),
        "namespace": g("/involvedObject/namespace"),
        "message": g("/message"),
        "count": e.pointer("/count").and_then(Value::as_i64).unwrap_or(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn hidden(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_hidden_namespace_hides_its_namespaced_objects_and_itself() {
        let h = hidden(&["kube-system"]);
        assert!(!visible_to("k8s:ns:kube-system", &h));
        assert!(!visible_to("k8s:pod:kube-system/coredns", &h));
        assert!(!visible_to("k8s:cnp:kube-system/allow-dns", &h));
        assert!(visible_to("k8s:ns:default", &h));
        assert!(visible_to("k8s:pod:default/web", &h));
    }

    #[test]
    fn cluster_scoped_objects_are_never_hidden_by_a_namespace() {
        let h = hidden(&["kube-system"]);
        // A node, a Cilium identity and the apiserver card belong to no
        // namespace; hiding them with one would be a lie about the cluster.
        assert!(visible_to("k8s:node:storm-1", &h));
        assert!(visible_to("k8s:cid:42", &h));
        assert!(visible_to("k8s:apiserver", &h));
        assert!(visible_to("k8s:cilium", &h));
        // And a kind this build does not know stays visible rather than
        // being guessed into a namespace it may not have.
        assert!(visible_to("k8s:whatever:kube-system/x", &h));
        assert!(visible_to("sb:volume:kube-system", &h));
    }

    #[test]
    fn the_catalogue_says_what_the_selector_applies_to() {
        let c = cache::catalogue();
        let by = |k: &str| c.iter().find(|v| v["kind"] == k).unwrap().clone();
        assert_eq!(by("pod")["namespaced"], true);
        assert_eq!(by("pod")["title"], "Pods");
        assert_eq!(by("pod")["href"], "#/k8s/pod");
        assert_eq!(by("node")["namespaced"], false);
        assert_eq!(by("ns")["namespaced"], false);
        assert_eq!(by("ns")["inventory"], false, "a namespace is not in its own inventory");
        assert_eq!(by("cnp")["namespaced"], true);
        assert_eq!(by("ccnp")["namespaced"], false);
        assert_eq!(by("quota")["inventory"], true);
        assert!(cache::is_namespaced("pod") && !cache::is_namespaced("node"));
        assert!(!cache::is_namespaced("nosuchkind"));
    }

    #[test]
    fn yaml_drops_the_field_that_buries_the_object() {
        let v = json!({
            "apiVersion": "v1",
            "kind": "Namespace",
            "metadata": {"name": "default", "managedFields": [{"manager": "rustkube"}]}
        });
        let y = to_yaml(&v);
        assert!(y.contains("name: default"), "{y}");
        assert!(!y.contains("managedFields"), "{y}");
    }

    #[test]
    fn annotations_drop_the_one_that_is_not_an_annotation() {
        // `last-applied-configuration` is the whole object as a JSON string,
        // kilobytes of it, and in a card of key/value pairs it buries every
        // annotation that means something.
        let obj = json!({"metadata": {"annotations": {
            "openshift.io/requester": "gwest",
            "openshift.io/description": "gwest-dev",
            "kubectl.kubernetes.io/last-applied-configuration": "{\"a\":1}"
        }}});
        let a = annotations_of(&obj);
        assert_eq!(a["openshift.io/requester"], "gwest");
        assert_eq!(a["openshift.io/description"], "gwest-dev");
        assert!(a.get("kubectl.kubernetes.io/last-applied-configuration").is_none());
    }

    #[test]
    fn a_namespace_with_no_annotations_yields_an_object_not_a_null() {
        // The view iterates it; a null would be a crash on a namespace that
        // simply has none, which is most of them.
        assert_eq!(annotations_of(&json!({"metadata": {}})), json!({}));
    }

    #[test]
    fn a_quota_prints_used_against_hard() {
        let q = json!({
            "metadata": {"name": "compute"},
            "spec": {"hard": {"cpu": "4", "memory": "8Gi"}},
            "status": {"hard": {"cpu": "4", "memory": "8Gi"}, "used": {"cpu": "1", "memory": "2Gi"}}
        });
        let row = quota_row(&q);
        assert_eq!(row["name"], "compute");
        assert_eq!(row["resources"][0]["resource"], "cpu");
        assert_eq!(row["resources"][0]["hard"], "4");
        assert_eq!(row["resources"][0]["used"], "1");
    }
}

#[cfg(test)]
mod path_tests {
    use super::namespace_in_path;

    #[test]
    fn the_namespace_a_write_acts_in_comes_off_its_path() {
        assert_eq!(namespace_in_path("/api/v1/namespaces/team-a/pods"), Some("team-a"));
        assert_eq!(
            namespace_in_path("/apis/apps/v1/namespaces/kube-system/deployments/dns"),
            Some("kube-system")
        );
        // Cluster-scoped: no namespace to check, and no namespace to hide.
        assert_eq!(namespace_in_path("/api/v1/nodes/storm-1"), None);
        assert_eq!(namespace_in_path("/api/v1/namespaces"), None);
        assert_eq!(namespace_in_path("/api/v1/namespaces/"), None);
    }
}
