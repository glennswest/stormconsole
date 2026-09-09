//! The VM plugin: virtual machines as first-class objects in the console.
//!
//! **Where a VM lives.** Not in a daemon of its own. stormvm's
//! `docs/kube.md` is explicit — "stormvm is libraries, the kubelet is the
//! loop": a VM is a KubeVirt `VirtualMachine`/`VirtualMachineInstance` in
//! the apiserver, rustkube-node's kubelet reconciles the instances
//! assigned to its node, and stormvm's own REST API "is not done, and may
//! not be wanted". So this plugin watches the CRDs exactly the way the
//! Cilium view watches Cilium's, with the same client and the same
//! list+watch loop, and there is no second source of truth to reconcile.
//!
//! **What it adds over the kubernetes plugin**, and why it is its own
//! plugin rather than two more kinds there: a VM is a domain with its own
//! navigation, its own creation forms, its own lifecycle verbs, and two
//! console doors that are websockets rather than component actions. None
//! of that belongs in a plugin whose subject is workloads.
//!
//! **What is honest about today.** The CRDs are optional — a cluster
//! without them shows an idle plugin that says so rather than an error.
//! Nothing turns a `VirtualMachine` into an instance yet (stormvm's own
//! "Left" list), so a definition that wants to run and has no instance
//! says exactly that instead of being rendered as broken. The console
//! doors are built and probed; stormvm serves neither yet.

pub mod components;
pub mod console;
mod create;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Creator, Health, NavSection};
use plugin_kubernetes::{Client, KubeStore, ResourceSpec};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// KubeVirt's vocabulary, because it is the one people already know and
/// the one `oc get vmi` and `virtctl` speak (stormvm DESIGN.md). Both are
/// optional: a cluster with no VM CRD is a cluster with no VMs, not a
/// broken one.
const RESOURCES: &[ResourceSpec] = &[
    ResourceSpec {
        kind: "vm",
        title: "Virtual machines",
        list_path: "/apis/kubevirt.io/v1/virtualmachines",
        namespaced: true,
        optional: true,
        inventory: false,
    },
    ResourceSpec {
        kind: "vmi",
        title: "Virtual machine instances",
        list_path: "/apis/kubevirt.io/v1/virtualmachineinstances",
        namespaced: true,
        optional: true,
        inventory: false,
    },
];

const VM_API: &str = "/apis/kubevirt.io/v1";

struct Inner {
    client: Option<Client>,
    store: Arc<KubeStore>,
    /// This node's stormvm, for the console doors only — the objects come
    /// from the apiserver.
    stormvm: Option<String>,
    stormvm_up: RwLock<bool>,
    http: reqwest::Client,
}

pub struct VmPlugin {
    inner: Arc<Inner>,
}

impl VmPlugin {
    pub fn new(server: Option<String>, token: Option<String>, insecure: bool, stormvm: Option<String>) -> Self {
        let client = server.as_ref().map(|s| Client::new(s, token.as_deref(), insecure));
        Self {
            inner: Arc::new(Inner {
                client,
                store: Arc::new(KubeStore::with_kinds(RESOURCES.len())),
                stormvm,
                stormvm_up: RwLock::new(false),
                http: reqwest::Client::new(),
            }),
        }
    }
}

#[async_trait]
impl ConsolePlugin for VmPlugin {
    fn name(&self) -> &'static str {
        "vm"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Virtualization", 15).item("Virtual machines", "#/vms")]
    }

    fn creators(&self) -> Vec<Creator> {
        create::creators()
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/machines/{ns}/{name}/start", post(start))
            .route("/machines/{ns}/{name}/stop", post(stop))
            .route("/machines/{ns}/{name}", delete(delete_machine))
            .route("/instances/{ns}/{name}/stop", post(delete_instance))
            .route("/vms/{ns}/{name}", get(detail))
            .route("/console/{ns}/{name}", get(console_caps))
            .route("/console/{ns}/{name}/serial", get(serial))
            .route("/console/{ns}/{name}/vnc", get(vnc))
            .route("/create", post(create::create))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        components::map(&self.inner.store.snapshot().await)
    }

    async fn health(&self) -> Health {
        if self.inner.client.is_none() {
            return Health::Idle;
        }
        let snap = self.inner.store.snapshot().await;
        let comps = components::map(&snap);
        if comps.is_empty() {
            return Health::Idle;
        }
        comps
            .iter()
            .map(|c| c.health)
            .min_by_key(|h| match h {
                Health::Error => 0,
                Health::Warn => 1,
                Health::Ok => 2,
                Health::Idle => 3,
                Health::Unknown => 4,
            })
            .unwrap_or(Health::Idle)
    }

    async fn detail(&self) -> String {
        if self.inner.client.is_none() {
            return "no rustkube endpoint configured".into();
        }
        let snap = self.inner.store.snapshot().await;
        let running = snap
            .get("vmi")
            .map(|m| {
                m.values()
                    .filter(|v| v.pointer("/status/phase").and_then(Value::as_str) == Some("Running"))
                    .count()
            })
            .unwrap_or(0);
        let instances = snap.get("vmi").map(|m| m.len()).unwrap_or(0);
        let defined = snap.get("vm").map(|m| m.len()).unwrap_or(0);
        let (synced, total) = self.inner.store.synced_kinds().await;
        if synced < total {
            return "waiting for the apiserver".into();
        }
        if self.inner.store.is_absent("vmi").await && self.inner.store.is_absent("vm").await {
            // Nothing this console can do about it, and worth saying
            // exactly: the cluster carries no VM resource at all.
            return "the kubevirt.io resources are not installed on this cluster".into();
        }
        if instances == 0 && defined == 0 {
            return "no virtual machines yet — the kubevirt.io resources are served and empty".into();
        }
        let doors = match (&self.inner.stormvm, *self.inner.stormvm_up.read().await) {
            (Some(_), true) => " · consoles available",
            (Some(_), false) => " · no console (stormvm not answering)",
            (None, _) => "",
        };
        format!("{running}/{instances} running · {defined} defined{doors}")
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
                plugin_kubernetes::watch(client, spec, store, token).await;
            });
        }
        // stormvm is probed rather than assumed: the console doors say
        // which upstream is missing, and that answer has to be current.
        loop {
            if let Some(url) = &self.inner.stormvm {
                let up = self
                    .inner
                    .http
                    .get(format!("{}/healthz", url.trim_end_matches('/')))
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await
                    .map(|r| r.status().is_success())
                    .unwrap_or(false);
                *self.inner.stormvm_up.write().await = up;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

fn no_apiserver() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"}))).into_response()
}

fn from_apiserver(status: reqwest::StatusCode, body: Value, done: &str) -> Response {
    if status.is_success() {
        return Json(json!({"message": done})).into_response();
    }
    let msg = body
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("apiserver returned {}", status.as_u16()));
    (StatusCode::BAD_GATEWAY, Json(json!({"error": msg}))).into_response()
}

/// Start and stop are one field. KubeVirt's `spec.running` is the switch;
/// writing it is the whole of a VM's lifecycle on the apiserver side, and
/// what acts on it is the cluster's business, not the console's.
async fn set_running(inner: &Inner, ns: &str, name: &str, running: bool) -> Response {
    let Some(client) = &inner.client else { return no_apiserver() };
    let path = format!("{VM_API}/namespaces/{ns}/virtualmachines/{name}");
    match client.patch_merge(&path, &json!({"spec": {"running": running}})).await {
        Ok((status, body)) => from_apiserver(
            status,
            body,
            if running { "start requested" } else { "stop requested" },
        ),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn start(State(inner): State<Arc<Inner>>, Path((ns, name)): Path<(String, String)>) -> Response {
    set_running(&inner, &ns, &name, true).await
}

async fn stop(State(inner): State<Arc<Inner>>, Path((ns, name)): Path<(String, String)>) -> Response {
    set_running(&inner, &ns, &name, false).await
}

async fn delete_machine(
    State(inner): State<Arc<Inner>>,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    let Some(client) = &inner.client else { return no_apiserver() };
    match client.delete(&format!("{VM_API}/namespaces/{ns}/virtualmachines/{name}")).await {
        Ok(s) if s.is_success() => Json(json!({"message": format!("{ns}/{name} deleted")})).into_response(),
        Ok(s) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("apiserver returned {}", s.as_u16())})))
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Stopping an instance is deleting it. There is no other verb: a VMI is
/// the running machine, and nothing about it survives being stopped
/// except its definition, if it has one.
async fn delete_instance(
    State(inner): State<Arc<Inner>>,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    let Some(client) = &inner.client else { return no_apiserver() };
    match client
        .delete(&format!("{VM_API}/namespaces/{ns}/virtualmachineinstances/{name}"))
        .await
    {
        Ok(s) if s.is_success() => Json(json!({"message": format!("{ns}/{name} stopped")})).into_response(),
        Ok(s) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("apiserver returned {}", s.as_u16())})))
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Everything a VM page shows, in one answer: the definition, the running
/// instance, the disks with what backs each, the interfaces, and the two
/// console doors' state.
async fn detail(State(inner): State<Arc<Inner>>, Path((ns, name)): Path<(String, String)>) -> Response {
    let key = format!("{ns}/{name}");
    let machine = inner.store.object("vm", &key).await;
    let instance = inner.store.object("vmi", &key).await;
    if machine.is_none() && instance.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("no virtual machine {key}")})),
        )
            .into_response();
    }
    // The instance is the truth about a running machine; the definition
    // is the truth about one that is not.
    let spec = instance
        .as_ref()
        .and_then(|v| v.get("spec").cloned())
        .or_else(|| machine.as_ref().and_then(|v| v.pointer("/spec/template/spec").cloned()))
        .unwrap_or(Value::Null);
    let domain = spec.get("domain").cloned().unwrap_or(Value::Null);
    let disks: Vec<Value> = components::disks(&spec)
        .into_iter()
        .map(|(name, backing)| json!({"name": name, "backing": backing}))
        .collect();
    let interfaces = spec
        .pointer("/domain/devices/interfaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let networks = spec.pointer("/networks").and_then(Value::as_array).cloned().unwrap_or_default();
    let caps = console::capabilities(inner.stormvm.as_deref(), *inner.stormvm_up.read().await);
    let yaml = plugin_kubernetes::to_yaml(
        machine.as_ref().or(instance.as_ref()).unwrap_or(&Value::Null),
    );
    Json(json!({
        "namespace": ns,
        "name": name,
        "phase": instance.as_ref().and_then(|v| v.pointer("/status/phase")).cloned(),
        "node": instance.as_ref().and_then(|v| v.pointer("/status/nodeName")).cloned(),
        "reason": instance.as_ref().and_then(|v| v.pointer("/status/reason").or_else(|| v.pointer("/status/message"))).cloned(),
        "vcpu": components::vcpus(&domain),
        "memory": components::memory(&domain),
        "disks": disks,
        "interfaces": interfaces,
        "networks": networks,
        "hasDefinition": machine.is_some(),
        "running": instance.is_some(),
        "console": caps,
        "yaml": yaml,
    }))
    .into_response()
}

async fn console_caps(State(inner): State<Arc<Inner>>, Path(_p): Path<(String, String)>) -> Response {
    Json(console::capabilities(inner.stormvm.as_deref(), *inner.stormvm_up.read().await))
        .into_response()
}

async fn door(
    inner: Arc<Inner>,
    ws: WebSocketUpgrade,
    ns: String,
    name: String,
    kind: console::Door,
) -> Response {
    let Some(base) = inner.stormvm.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "no stormvm configured — set [vm] url"})),
        )
            .into_response();
    };
    let url = console::ws_url(&base, &console::upstream_path(kind, &ns, &name));
    ws.on_upgrade(move |socket| console::relay(socket, url))
}

async fn serial(
    State(inner): State<Arc<Inner>>,
    Path((ns, name)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    door(inner, ws, ns, name, console::Door::Serial).await
}

async fn vnc(
    State(inner): State<Arc<Inner>>,
    Path((ns, name)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    door(inner, ws, ns, name, console::Door::Vnc).await
}
