//! The kubernetes plugin: rustkube apiserver views through a watch-backed
//! cache. rustkube only — this console has no other orchestrator.
//!
//! One list+watch loop per resource kind feeds a shared store; the
//! components mapping renders a consistent snapshot with health derived
//! from the same conditions kubectl reads. Actions surface as POST routes
//! under /api/plugins/k8s so any stormview renderer can wire them.

pub mod apply;
mod authz;
pub mod cache;
pub mod client;
mod components;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use console_core::{
    Access, ComponentSummary, ConsolePlugin, Creator, Health, NavSection, Probe, Viewer,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use cache::{watch_resource, Store, RESOURCES};
use client::RkClient;

// A VM on this platform is a kube object — a KubeVirt
// `VirtualMachineInstance` the kubelet reconciles (stormvm docs/kube.md) —
// so the VM plugin watches the apiserver with the same client and the
// same list+watch loop rather than growing a second one.
pub use cache::{watch_resource as watch, ResourceSpec, Store as KubeStore};
pub use client::{RkClient as Client, RkError};

struct Inner {
    server: Option<String>,
    client: Option<RkClient>,
    probe: Option<Probe>,
    store: Arc<Store>,
    http: reqwest::Client,
    authz: authz::Authorizer,
}

pub struct KubernetesPlugin {
    inner: Arc<Inner>,
}

impl KubernetesPlugin {
    pub fn new(server: Option<String>, token: Option<String>, insecure: bool) -> Self {
        let client = server.as_ref().map(|s| RkClient::new(s, token.as_deref(), insecure));
        let probe = client.as_ref().map(|c| Probe::new(format!("{}/version", c.base())));
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure)
            .build()
            .expect("reqwest client");
        Self {
            inner: Arc::new(Inner {
                server,
                client,
                probe,
                store: Arc::new(Store::default()),
                http,
                authz: authz::Authorizer::default(),
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
            NavSection::new("Home", 0).item("Overview", "#/"),
            NavSection::new("Workloads", 10)
                .item("Pods", "#/k8s/pod")
                .item("Deployments", "#/k8s/deploy")
                .item("StatefulSets", "#/k8s/sts")
                .item("DaemonSets", "#/k8s/ds")
                .item("Jobs", "#/k8s/job")
                .item("CronJobs", "#/k8s/cronjob"),
            NavSection::new("Networking", 25)
                .item("Services", "#/k8s/svc")
                .item("Network policies", "#/k8s/netpol")
                .item("Cilium policies", "#/k8s/cnp")
                .item("Clusterwide policies", "#/k8s/ccnp")
                .item("Cilium endpoints", "#/k8s/cep")
                .item("Cilium nodes", "#/k8s/cn")
                .item("Identities", "#/k8s/cid"),
            NavSection::new("Compute", 20).item("Cluster nodes", "#/k8s/node"),
            NavSection::new("Storage", 40).item("PVCs", "#/k8s/pvc"),
            NavSection::new("Observe", 30).item("Events", "#/k8s/events"),
            NavSection::new("Administration", 60).item("Namespaces", "#/k8s/ns"),
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
            .route("/object/{kind}/{*key}", get(object))
            .route("/apply", post(apply_yaml))
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
        out.extend(components::map(&inner.store.snapshot().await));
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
    async fn access(&self, viewer: &Viewer) -> Access {
        let (Some(token), Some(server)) = (&viewer.token, &self.inner.server) else {
            return Access::Unrestricted;
        };
        let known = self.inner.store.namespaces().await;
        let allowed =
            self.inner.authz.allowed(server, &self.inner.http, token, &known).await;
        if allowed.source == authz::Source::Unavailable {
            // An authorizer that cannot be reached must not quietly become
            // a permissive one, and must not blank the console either.
            // Nothing is hidden and the note says why.
            return Access::limited(
                |_| true,
                0,
                authz::note(0, authz::Source::Unavailable),
            );
        }
        let hidden: Vec<String> =
            known.iter().filter(|n| !allowed.namespaces.contains(*n)).cloned().collect();
        if hidden.is_empty() {
            return Access::Unrestricted;
        }
        let hidden_set: std::collections::HashSet<String> = hidden.into_iter().collect();
        let count = hidden_set.len();
        let note = authz::note(count, allowed.source);
        Access::limited(move |id| visible_to(id, &hidden_set), count, note)
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
        if let Some(probe) = &self.inner.probe {
            probe.run(self.inner.http.clone(), Duration::from_secs(10), shutdown).await;
        } else {
            shutdown.cancelled().await;
        }
    }
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
async fn namespace_detail(State(inner): State<Arc<Inner>>, Path(ns): Path<String>) -> Response {
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
            .get(&format!("/api/v1/namespaces/{ns}/events"))
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
async fn object(State(inner): State<Arc<Inner>>, Path((kind, key)): Path<(String, String)>) -> Response {
    match inner.store.object(&kind, &key).await {
        Some(v) => Json(json!({"kind": kind, "key": key, "object": v, "yaml": to_yaml(&v)}))
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("no {kind} {key} in the cache")})),
        )
            .into_response(),
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
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    match client.delete(&format!("/api/v1/namespaces/{ns}/pods/{name}")).await {
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
async fn raw_delete(State(inner): State<Arc<Inner>>, Path(path): Path<String>) -> Response {
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"})))
            .into_response();
    };
    let path = format!("/{}", path.trim_start_matches('/'));
    if !(path.starts_with("/api/") || path.starts_with("/apis/")) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "not an apiserver path"}))).into_response();
    }
    match client.delete(&path).await {
        Ok(status) if status.is_success() => Json(json!({"deleted": path})).into_response(),
        Ok(status) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("apiserver returned {}", status.as_u16())})))
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Import YAML, OpenShift-style: one or more documents, each created in
/// its collection. Every document gets a line in the result; a failure on
/// one does not stop the rest.
async fn apply_yaml(State(inner): State<Arc<Inner>>, body: String) -> Response {
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
        let (kind, name, path) = match apply::target(&doc) {
            Ok(t) => t,
            Err(e) => {
                failed = true;
                results.push(json!({"error": e}));
                continue;
            }
        };
        match client.post_json(&path, &doc).await {
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

#[derive(serde::Deserialize)]
struct EventsQuery {
    namespace: Option<String>,
}

async fn events(State(inner): State<Arc<Inner>>, Query(q): Query<EventsQuery>) -> Response {
    let Some(client) = &inner.client else {
        return Json(json!([])).into_response();
    };
    let path = match &q.namespace {
        Some(ns) if !ns.is_empty() => format!("/api/v1/namespaces/{ns}/events"),
        _ => "/api/v1/events".to_string(),
    };
    match client.get(&path).await {
        Ok(list) => {
            let rows: Vec<Value> = list
                .get("items")
                .and_then(Value::as_array)
                .map(|items| items.iter().map(event_row).collect())
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
