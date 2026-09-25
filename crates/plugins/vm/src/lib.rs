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
pub mod disks;
pub mod images;
pub mod keys;
mod keystore;
pub mod network;
pub mod settings;
pub mod snapshots;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use console_core::{
    Access, ComponentSummary, ConsolePlugin, Creator, Events, Health, NavSection, Viewer,
};
use plugin_kubernetes::{Client, KubeStore, NamespaceAccess, ResourceSpec};
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
        api_kind: "VirtualMachine",
        title: "Virtual machines",
        list_path: "/apis/kubevirt.io/v1/virtualmachines",
        namespaced: true,
        optional: true,
        inventory: false,
    },
    ResourceSpec {
        kind: "vmi",
        api_kind: "VirtualMachineInstance",
        title: "Virtual machine instances",
        list_path: "/apis/kubevirt.io/v1/virtualmachineinstances",
        namespaced: true,
        optional: true,
        inventory: false,
    },
    // A machine's snapshots and restores (#25). Optional like the rest:
    // the CRDs arrive with stormpump#28, and a cluster without them has a
    // Backup tab that says so rather than a failed watch.
    ResourceSpec {
        kind: "vmsnap",
        api_kind: "VirtualMachineSnapshot",
        title: "Virtual machine snapshots",
        list_path: "/apis/snapshot.kubevirt.io/v1beta1/virtualmachinesnapshots",
        namespaced: true,
        optional: true,
        inventory: false,
    },
    ResourceSpec {
        kind: "vmrestore",
        api_kind: "VirtualMachineRestore",
        title: "Virtual machine restores",
        list_path: "/apis/snapshot.kubevirt.io/v1beta1/virtualmachinerestores",
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
    /// What stormvm last said about each machine it is running, keyed
    /// `ns/name` — `console.{serial,vnc,replay}` and
    /// `control.{lifecycle,freeze}` (stormvm `docs/console.md`).
    ///
    /// The probe already fetched the collection to decide whether stormvm
    /// was answering and threw the body away. It is the only place the
    /// control verbs are reported, and a console that offers Pause on a
    /// machine whose control socket was never bound is a button that
    /// returns 404 and sends whoever pressed it looking at the guest.
    stormvm_vms: RwLock<std::collections::HashMap<String, Value>>,
    http: reqwest::Client,
    /// The same namespace-authorization answer the kubernetes plugin
    /// uses. A VM is a kube object in a namespace, so it is hidden by
    /// exactly the rule that hides a pod — and asking separately would
    /// mean two sets of probes for one answer, and two answers that can
    /// disagree for a cache window.
    access: Option<Arc<NamespaceAccess>>,
    /// Where vmcloud-image-operator answers, and the last thing it said.
    ///
    /// Polled rather than asked on demand: `creators()` is synchronous, and
    /// a create form must not wait on an upstream that may be slow or gone.
    image_operator: Option<String>,
    images: images::Cache,
    /// Where each user's SSH-key Secret lives (#26); copies go to the
    /// namespaces their machines are in, because `accessCredentials` can
    /// only name a Secret in the machine's own.
    keys_ns: String,
}

pub struct VmPlugin {
    inner: Arc<Inner>,
}

impl VmPlugin {
    pub fn new(
        server: Option<String>,
        token: Option<String>,
        insecure: bool,
        stormvm: Option<String>,
        access: Option<Arc<NamespaceAccess>>,
    ) -> Self {
        Self::with_images(server, token, insecure, stormvm, access, None)
    }

    /// The same, naming where `vmcloud-image-operator` answers.
    ///
    /// `None` keeps the old behaviour exactly: the root-disk field is free
    /// text. A console on a cluster without the operator should not offer an
    /// empty dropdown where a working text box used to be.
    pub fn with_images(
        server: Option<String>,
        token: Option<String>,
        insecure: bool,
        stormvm: Option<String>,
        access: Option<Arc<NamespaceAccess>>,
        image_operator: Option<String>,
    ) -> Self {
        let client = server.as_ref().map(|s| Client::new(s, token.as_deref(), insecure));
        Self {
            inner: Arc::new(Inner {
                client,
                store: Arc::new(KubeStore::with_kinds(RESOURCES.len())),
                stormvm,
                stormvm_up: RwLock::new(false),
                stormvm_vms: RwLock::new(std::collections::HashMap::new()),
                http: reqwest::Client::new(),
                access,
                image_operator,
                images: images::Cache::new(),
                keys_ns: "default".into(),
            }),
        }
    }

    /// Where users' SSH-key Secrets live. Before the plugin is shared.
    pub fn with_keys_namespace(mut self, ns: &str) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            if !ns.trim().is_empty() {
                inner.keys_ns = ns.trim().to_string();
            }
        }
        self
    }
}

#[async_trait]
impl ConsolePlugin for VmPlugin {
    fn name(&self) -> &'static str {
        "vm"
    }

    fn nav(&self) -> Vec<NavSection> {
        // A virtual machine is a workload, not a category of its own.
        //
        // It had a section to itself holding one item, which said the
        // thing a console should not: that running a machine is a
        // different kind of activity from running a pod. On this platform
        // it is the same activity — a VM *is* a kube object the kubelet
        // reconciles — and somebody choosing what to run wants the choice
        // in front of them rather than in another part of the navigator.
        //
        // Next to Pods, at a position the kubernetes plugin left free.
        vec![NavSection::new("Workloads", 10).item_at("Virtual machines", "#/vms", 10)]
    }

    fn creators(&self) -> Vec<Creator> {
        // The snapshot, not a request: this is called while the console is
        // rendering and must not wait on anything.
        create::creators(&self.inner.images.get())
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/machines/{ns}/{name}/start", post(start))
            .route("/machines/{ns}/{name}/stop", post(stop))
            .route("/machines/{ns}/{name}/restart", post(restart))
            // The verbs stormvm serves beside the doors. Proxied rather
            // than reimplemented: the hypervisor is the thing that knows
            // whether a guest took an ACPI powerdown.
            .route("/machines/{ns}/{name}/verb/{verb}", post(control))
            .route("/machines/{ns}/{name}", delete(delete_machine))
            .route("/instances/{ns}/{name}/stop", post(delete_instance))
            .route("/vms/{ns}/{name}", get(detail))
            .route("/vms/{ns}/{name}/settings", get(settings_of).put(settings_set))
            .route("/vms/{ns}/{name}/disks", post(disk_add))
            .route("/vms/{ns}/{name}/disks/{disk}", delete(disk_remove))
            .route("/vms/{ns}/{name}/snapshots", get(snapshots_list).post(snapshot_take))
            .route("/keys", get(keys_list).post(keys_add))
            .route("/keys/choices", get(keys_choices))
            .route("/keys/{item}", delete(keys_remove))
            .route("/vms/{ns}/{name}/keys", get(vm_keys).post(vm_keys_add))
            .route("/vms/{ns}/{name}/snapshots/{snap}", delete(snapshot_delete))
            .route("/vms/{ns}/{name}/snapshots/{snap}/restore", post(snapshot_restore))
            .route("/console/{ns}/{name}", get(console_caps))
            .route("/console/{ns}/{name}/serial", get(serial))
            .route("/console/{ns}/{name}/vnc", get(vnc))
            .route("/create", post(create::create))
            .with_state(self.inner.clone())
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        components::map_with(
            &self.inner.store.snapshot().await,
            &*self.inner.stormvm_vms.read().await,
        )
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

    /// Every VM component is `vm:<sub>:<ns>/<name>`, so a hidden
    /// namespace hides them by the same rule that hides a pod.
    /// What happened to one machine.
    ///
    /// A machine's events are recorded against two objects that share a
    /// name — the `VirtualMachine` the scheduler and the controller write
    /// about, and the `VirtualMachineInstance` the kubelet writes about —
    /// and somebody asking "did my start work" does not care which. So
    /// both are collected and merged, which is also why this is not left
    /// to the kubernetes plugin: it would answer for one of them.
    ///
    /// This is the question the whole feature exists for. A machine that
    /// will not start says `Running` nowhere and `Scheduling` forever, and
    /// the reason — no node with enough memory, a golden that would not
    /// clone, a bridge that does not exist on the node it was pinned to —
    /// is in an event and nowhere else.
    async fn events(&self, viewer: &Viewer, id: &str) -> Option<Events> {
        let rest = id.strip_prefix("vm:")?;
        let (sub, key) = rest.split_once(':')?;
        if !matches!(sub, "instance" | "machine") {
            return None;
        }
        let (ns, name) = key.split_once('/')?;
        if refuse_hidden(&self.inner, viewer, ns).await.is_some() {
            return Some(Events::none(format!("no virtual machine {ns}/{name}")));
        }
        let client = self.inner.client.as_ref()?;
        let list = match client.get(&format!("/api/v1/namespaces/{ns}/events")).await {
            Ok(l) => l,
            Err(e) => {
                return Some(Events::none(format!("the apiserver did not answer for events: {e}")))
            }
        };
        let items: Vec<_> = list
            .get("items")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|e| {
                        // Both objects, because they are one machine to
                        // whoever is looking at it.
                        ["VirtualMachine", "VirtualMachineInstance"].iter().any(|k| {
                            plugin_kubernetes::objevents::about(e, k, ns, name)
                        })
                    })
                    .map(plugin_kubernetes::objevents::event)
                    .collect()
            })
            .unwrap_or_default();
        Some(Events::of(items).newest(20))
    }

    async fn access(&self, viewer: &Viewer) -> Access {
        let Some(shared) = &self.inner.access else { return Access::Unrestricted };
        let Some((hidden, note)) = shared.hidden(viewer).await else {
            return Access::Unrestricted;
        };
        let count = hidden.len();
        Access::limited(move |id| visible_to(id, &hidden), count, note)
    }

    async fn run(&self, shutdown: CancellationToken) {
        // The watches need an apiserver; the console doors do not. Guarding
        // both on the client meant a node with no rustkube reported its
        // consoles permanently shut while stormvm was answering on the same
        // machine — the doors have nothing to do with the apiserver, and a
        // VM's screen is exactly what you want on a node whose control
        // plane is the thing that is broken.
        if let Some(client) = self.inner.client.clone() {
            for spec in RESOURCES {
                let store = self.inner.store.clone();
                let client = client.clone();
                let token = shutdown.clone();
                tokio::spawn(async move {
                    plugin_kubernetes::watch(client, spec, store, token).await;
                });
            }
        }
        // Keep the root-disk list current (stormcos: wire new VM to the image
        // operator). A console that offered a dropdown built once at startup
        // would never show an image goldened five minutes ago, which is
        // exactly when somebody goes looking for it.
        if let Some(base) = self.inner.image_operator.clone() {
            let inner = self.inner.clone();
            let token = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    // The node is left empty here: this console serves a
                    // cluster, and which node a VM is pinned to is a field on
                    // the form rather than a property of the console. The
                    // "already on this node" grouping needs a node, so it is
                    // answered per request by the images route below.
                    let cat = images::fetch(&inner.http, &base, "").await;
                    let got = !cat.choices.is_empty();
                    inner.images.put(cat);
                    // Until the first good answer, ask again in seconds.
                    //
                    // The console and the image operator start together on a
                    // node, so the operator is reliably not up yet when the
                    // first poll goes out. A flat minute of backoff meant the
                    // create form spent the first minute after every boot
                    // with an empty dropdown — which is precisely when
                    // somebody is at the console looking at a machine that
                    // just came up.
                    let wait = if got { images::REFRESH } else { images::RETRY };
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = token.cancelled() => return,
                    }
                }
            });
        }

        // stormvm is probed rather than assumed: the console doors say
        // which upstream is missing, and that answer has to be current.
        //
        // The probe asks for the VM collection, not `/healthz`. Every
        // daemon on this platform answers `/healthz`, so a health probe
        // says only that *something* is on that port — and the console
        // would then offer a terminal that dials a stranger. What is
        // needed is whether *stormvm's VM API* is there, which is the
        // same API the console doors hang off.
        loop {
            if let Some(url) = &self.inner.stormvm {
                let answer = self
                    .inner
                    .http
                    .get(format!("{}/api/v1/vms", url.trim_end_matches('/')))
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await;
                let mut running = std::collections::HashMap::new();
                let up = match answer {
                    Ok(r) if r.status().is_success() => {
                        // The same body, read rather than discarded: it
                        // carries what each machine can be asked to do.
                        let v: Value = r.json().await.unwrap_or(Value::Null);
                        for item in v.get("items").and_then(Value::as_array).into_iter().flatten() {
                            let ns = item.get("namespace").and_then(Value::as_str).unwrap_or("");
                            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                            if !name.is_empty() {
                                running.insert(format!("{ns}/{name}"), item.clone());
                            }
                        }
                        true
                    }
                    _ => false,
                };
                *self.inner.stormvm_up.write().await = up;
                *self.inner.stormvm_vms.write().await = running;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

/// Is this component id outside every hidden namespace? Ids are
/// `vm:<sub>:<ns>/<name>`; anything that is not shaped that way is not
/// this plugin's to hide.
fn visible_to(id: &str, hidden: &std::collections::HashSet<String>) -> bool {
    let Some(rest) = id.strip_prefix("vm:") else { return true };
    let Some((_, key)) = rest.split_once(':') else { return true };
    match key.split_once('/') {
        Some((ns, _)) => !hidden.contains(ns),
        None => true,
    }
}

/// A VM in a namespace this viewer may not see does not exist as far as
/// they are concerned — the same answer an absent one gets, so a plugin
/// route is not a way around the filtered feed.
async fn refuse_hidden(inner: &Inner, viewer: &Viewer, ns: &str) -> Option<Response> {
    let (hidden, _) = inner.access.as_ref()?.hidden(viewer).await?;
    hidden.contains(ns).then(|| {
        (StatusCode::NOT_FOUND, Json(json!({"error": format!("no namespace {ns}")}))).into_response()
    })
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
async fn set_running(
    inner: &Inner,
    viewer: &Viewer,
    ns: &str,
    name: &str,
    running: bool,
) -> Response {
    if let Some(refusal) = refuse_hidden(inner, viewer, ns).await {
        return refusal;
    }
    let Some(client) = &inner.client else { return no_apiserver() };
    let path = format!("{VM_API}/namespaces/{ns}/virtualmachines/{name}");
    // Carried as the viewer, so the apiserver's RBAC decides whether they
    // may start it — not the console's own standing.
    match client
        .patch_merge(&path, &json!({"spec": {"running": running}}), viewer.token.as_deref())
        .await
    {
        Ok((status, body)) => from_apiserver(
            status,
            body,
            if running { "start requested" } else { "stop requested" },
        ),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn start(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    set_running(&inner, &viewer, &ns, &name, true).await
}

async fn stop(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    set_running(&inner, &viewer, &ns, &name, false).await
}

/// Restart is the instance deleted out from under a definition that wants
/// it running: the definition puts it back, and that is the only restart
/// KubeVirt has — there is no verb on a VMI that reboots it in place.
///
/// Refused without a definition, rather than performed and reported as a
/// restart. Deleting a VMI nothing will recreate is a delete, and a button
/// that quietly means something else on half the rows is worse than one
/// that says no.
async fn restart(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let Some(client) = &inner.client else { return no_apiserver() };
    let defined = client
        .get_as(
            &format!("{VM_API}/namespaces/{ns}/virtualmachines/{name}"),
            viewer.token.as_deref(),
        )
        .await
        .is_ok();
    if !defined {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": format!(
                    "{ns}/{name} has no VirtualMachine defining it — stopping the instance would \
                     not bring it back, so there is nothing to restart"
                )
            })),
        )
            .into_response();
    }
    match client
        .delete(
            &format!("{VM_API}/namespaces/{ns}/virtualmachineinstances/{name}"),
            viewer.token.as_deref(),
        )
        .await
    {
        Ok(s) if s.is_success() => {
            Json(json!({"message": format!("{ns}/{name} restarting")})).into_response()
        }
        Ok(s) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("apiserver returned {}", s.as_u16())})),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn delete_machine(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let Some(client) = &inner.client else { return no_apiserver() };
    // Delete whatever this machine actually is.
    //
    // This deleted a `virtualmachines` and nothing else, so deleting a
    // machine created directly as a VMI — which is what the console's own
    // create form makes, and what a `sc apply` of an instance makes —
    // returned 404 with no explanation. The row offered Delete and the
    // delete did not work, which is worse than not offering it.
    //
    // The definition first, because deleting it takes the instance with it.
    // Then the instance, for a machine that never had one. A 404 on the
    // definition is not an error here: it is the ordinary case.
    let key = format!("{ns}/{name}");
    let has_definition = inner.store.object("vm", &key).await.is_some();
    let mut last: Option<String> = None;
    if has_definition {
        match client
            .delete(
                &format!("{VM_API}/namespaces/{ns}/virtualmachines/{name}"),
                viewer.token.as_deref(),
            )
            .await
        {
            Ok(s) if s.is_success() => {
                return Json(json!({"message": format!("{ns}/{name} deleted")})).into_response()
            }
            Ok(s) => last = Some(format!("apiserver returned {} for the definition", s.as_u16())),
            Err(e) => last = Some(e.to_string()),
        }
    }
    match client
        .delete(
            &format!("{VM_API}/namespaces/{ns}/virtualmachineinstances/{name}"),
            viewer.token.as_deref(),
        )
        .await
    {
        Ok(s) if s.is_success() => {
            Json(json!({"message": format!("{ns}/{name} deleted")})).into_response()
        }
        Ok(s) if s.as_u16() == 404 && last.is_none() => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("no virtual machine {key}")})),
        )
            .into_response(),
        Ok(s) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": last.unwrap_or_else(|| format!("apiserver returned {}", s.as_u16()))})),
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Stopping an instance is deleting it. There is no other verb: a VMI is
/// the running machine, and nothing about it survives being stopped
/// except its definition, if it has one.
async fn delete_instance(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let Some(client) = &inner.client else { return no_apiserver() };
    match client
        .delete(
            &format!("{VM_API}/namespaces/{ns}/virtualmachineinstances/{name}"),
            viewer.token.as_deref(),
        )
        .await
    {
        Ok(s) if s.is_success() => Json(json!({"message": format!("{ns}/{name} stopped")})).into_response(),
        Ok(s) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("apiserver returned {}", s.as_u16())})))
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// The verbs stormvm serves beside the doors: `pause`, `unpause`,
/// `softreboot`, `reset`, `freeze`, `thaw` (stormvm `docs/console.md`).
///
/// Proxied rather than reimplemented. Pausing a guest is QMP or
/// cloud-hypervisor's HTTP API depending on which hypervisor started it,
/// and which one that is was recorded at start precisely so nothing else
/// has to guess. The console's job is to be the origin the browser talks
/// to, and to refuse a verb for a namespace this viewer cannot see.
///
/// Not every verb every machine: `control.lifecycle` says whether the
/// control socket was bound and `control.freeze` whether the guest has its
/// own agent, and the components only offer what is reported. A verb sent
/// anyway is stormvm's to refuse, and its refusal is passed through in its
/// own words.
const VERBS: &[&str] = &["pause", "unpause", "softreboot", "reset", "freeze", "thaw"];

async fn control(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name, verb)): Path<(String, String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    if !VERBS.contains(&verb.as_str()) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("{verb:?} is not a verb this console sends")})),
        )
            .into_response();
    }
    let Some(base) = inner.stormvm.as_deref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "no stormvm configured — set [vm] url to the node's stormvm"})),
        )
            .into_response();
    };
    let url = format!("{}/api/v1/vms/{ns}/{name}/{verb}", base.trim_end_matches('/'));
    // Freeze and thaw are the pair where a timeout is the dangerous
    // outcome: a guest left frozen has every write blocked, which from
    // inside looks like a machine that has hung. stormvm bounds its own
    // wait, so this one only has to be longer than that.
    match inner.http.put(&url).timeout(Duration::from_secs(20)).send().await {
        Ok(r) => {
            let status = r.status();
            let body: Value = r.json().await.unwrap_or(Value::Null);
            if status.is_success() {
                return Json(json!({"message": format!("{verb} sent to {ns}/{name}")}))
                    .into_response();
            }
            let msg = body
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("stormvm refused {verb} ({})", status.as_u16()));
            (StatusCode::BAD_GATEWAY, Json(json!({"error": msg}))).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": format!("stormvm did not answer {verb}: {e}")})),
        )
            .into_response(),
    }
}

/// The machine's own `spec.template.spec`, and the refusal when there is
/// none to edit. Shared by the disk verbs and the settings write, because
/// they are the same question: is there something durable to patch?
async fn definition_spec(inner: &Inner, ns: &str, name: &str) -> Result<Value, Response> {
    let key = format!("{ns}/{name}");
    match inner.store.object("vm", &key).await {
        Some(m) => Ok(m.pointer("/spec/template/spec").cloned().unwrap_or(Value::Null)),
        None => {
            let s = settings::of(None, inner.store.object("vmi", &key).await.as_ref());
            Err((StatusCode::CONFLICT, Json(json!({"error": s.why}))).into_response())
        }
    }
}

/// Apply a merge patch to the `VirtualMachine`, as the viewer.
async fn patch_machine(
    inner: &Inner,
    viewer: &Viewer,
    ns: &str,
    name: &str,
    body: Value,
    done: String,
) -> Response {
    let Some(client) = &inner.client else { return no_apiserver() };
    let path = format!("{VM_API}/namespaces/{ns}/virtualmachines/{name}");
    match client.patch_merge(&path, &body, viewer.token.as_deref()).await {
        Ok((status, b)) if status.is_success() => {
            let _ = b;
            Json(json!({"message": done})).into_response()
        }
        Ok((status, b)) => from_apiserver(status, b, ""),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Add a disk to a machine.
///
/// Written to the definition, so the guest sees it at its next boot —
/// nothing on this platform attaches a disk to a running machine, and the
/// answer says so rather than letting somebody watch for a device that is
/// never going to appear.
async fn disk_add(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    Json(add): Json<disks::Add>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let spec = match definition_spec(&inner, &ns, &name).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let body = match disks::add(&spec, &add) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let running = inner.store.object("vmi", &format!("{ns}/{name}")).await.is_some();
    let done = disks::effect(running, "added", add.name.trim());
    patch_machine(&inner, &viewer, &ns, &name, body, done).await
}

async fn disk_remove(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name, disk)): Path<(String, String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let spec = match definition_spec(&inner, &ns, &name).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let body = match disks::remove(&spec, &disk) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let running = inner.store.object("vmi", &format!("{ns}/{name}")).await.is_some();
    let done = disks::effect(running, "removed", disk.trim());
    patch_machine(&inner, &viewer, &ns, &name, body, done).await
}

/// What can be changed about this machine, and when each change lands.
async fn settings_of(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let key = format!("{ns}/{name}");
    let machine = inner.store.object("vm", &key).await;
    let instance = inner.store.object("vmi", &key).await;
    if machine.is_none() && instance.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no virtual machine {key}")})))
            .into_response();
    }
    let mut s = settings::of(machine.as_ref(), instance.as_ref());
    // Being able to see a machine is not being able to change it — but
    // only overwrite the reason when there was one to overwrite. A machine
    // that cannot be edited because it has no definition should say that,
    // not be reported as a permission problem it does not have.
    if !viewer.may_write() && s.editable {
        s.editable = false;
        s.why = "changing a machine needs the `operator` role".into();
    }
    Json(s).into_response()
}

#[derive(serde::Deserialize)]
struct Change {
    field: String,
    #[serde(default)]
    value: String,
}

/// One field at a time, as a merge patch against the `VirtualMachine`.
///
/// Against the definition, never the instance: a patch to a running VMI's
/// spec is read by nothing and is gone when it stops. So a machine with no
/// definition is refused rather than half-changed.
async fn settings_set(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    Json(change): Json<Change>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    if !viewer.may_write() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "changing a machine needs the `operator` role"})),
        )
            .into_response();
    }
    let key = format!("{ns}/{name}");
    if let Err(r) = definition_spec(&inner, &ns, &name).await {
        return r;
    }
    let body = match settings::patch(&change.field, &change.value) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let running = inner.store.object("vmi", &key).await.is_some();
    let done = if running {
        format!("{} written — in force after a restart", change.field)
    } else {
        format!("{} written", change.field)
    };
    // As the viewer, so the apiserver's RBAC decides — the same rule the
    // read that showed them the field was subject to.
    patch_machine(&inner, &viewer, &ns, &name, body, done).await
}

/// Everything a VM page shows, in one answer: the definition, the running
/// instance, the disks with what backs each, the interfaces, and the two
/// console doors' state.
async fn detail(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
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
    // What the machine will have, and what it has.
    //
    // Reading the instance alone — "the instance is the truth about a
    // running machine" — meant a disk added to the definition vanished
    // from the page the moment it was added, because nothing attaches one
    // to a running guest. The answer is not to read the definition
    // instead: then a disk removed from the definition would disappear
    // while the guest still had it.
    //
    // So both, merged by name, each saying whether it is `attached` — the
    // same shape as a pending setting, for the same reason.
    let disks = disks::merged(
        machine.as_ref().and_then(|v| v.pointer("/spec/template/spec")),
        instance.as_ref().and_then(|v| v.get("spec")),
    );
    // Per interface, what was asked for beside what the node did (#24).
    //
    // `status.interfaces[]` alone is the running answer — MAC, binding,
    // address — and loses what somebody meant; the spec alone reads a
    // `pod` network as working when the node has put the guest behind a
    // NAT inside the hypervisor (stormvm#16). `network::interfaces` keeps
    // both and says whether the address is one anybody can use.
    let interfaces = network::interfaces(
        &spec,
        &[
            instance.as_ref().and_then(|v| v.get("metadata")),
            machine.as_ref().and_then(|v| v.pointer("/spec/template/metadata")),
            machine.as_ref().and_then(|v| v.get("metadata")),
        ],
        instance.as_ref(),
    );
    let networks = spec.pointer("/networks").and_then(Value::as_array).cloned().unwrap_or_default();
    let caps = console::for_vm(
        &inner.http,
        inner.stormvm.as_deref(),
        *inner.stormvm_up.read().await,
        &ns,
        &name,
        viewer.may_write(),
    )
    .await;
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
        // Whether a disk can be added at all: there has to be something
        // durable to write it to.
        "disksEditable": machine.is_some() && viewer.may_write(),
        "running": instance.is_some(),
        "console": caps,
        "settings": settings::of(machine.as_ref(), instance.as_ref()),
        "yaml": yaml,
    }))
    .into_response()
}

fn bad_gateway(e: String) -> Response {
    (StatusCode::BAD_GATEWAY, Json(json!({"error": e}))).into_response()
}

fn key_rows(list: &std::collections::BTreeMap<String, String>, source: &str) -> Vec<Value> {
    list.iter()
        .map(|(name, line)| match keys::parse(line) {
            Ok(k) => json!({"name": name, "kind": k.kind, "comment": k.comment, "short": k.short(),
                            "line": k.line(), "source": source}),
            Err(e) => json!({"name": name, "line": line, "source": source, "error": e}),
        })
        .collect()
}

/// The config file's keys for this viewer, by a stable item name, so a
/// form can offer them beside the saved ones.
fn config_keys(viewer: &Viewer) -> std::collections::BTreeMap<String, String> {
    viewer
        .ssh_keys
        .iter()
        .enumerate()
        .map(|(i, k)| (format!("config-{}", i + 1), k.trim().to_string()))
        .collect()
}

/// Account → SSH keys: the viewer's saved keys, the ones the console's
/// config gives them (read-only here), and where copies live (#26).
async fn keys_list(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let user = keystore::user_of(&viewer);
    let (saved, error) = match keystore::saved(&inner, &viewer).await {
        Ok(s) => (s, None),
        Err(e) => (Default::default(), Some(e)),
    };
    Json(json!({
        "user": user,
        "home": inner.keys_ns,
        "secret": keys::secret_name(&user),
        "keys": key_rows(&saved, "saved"),
        "config": key_rows(&config_keys(&viewer), "config"),
        "copies": keystore::copies(&inner, &viewer).await,
        "write": viewer.may_write(),
        "error": error,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct AddKeys {
    #[serde(default)]
    name: String,
    key: String,
}

async fn keys_add(State(inner): State<Arc<Inner>>, viewer: Viewer, Json(add): Json<AddKeys>) -> Response {
    let parsed = match keys::parse_all(&add.key) {
        Ok(k) => k,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let mut list = match keystore::saved(&inner, &viewer).await {
        Ok(l) => l,
        Err(e) => return bad_gateway(e),
    };
    let mut added = Vec::new();
    for (i, k) in parsed.iter().enumerate() {
        if list.values().any(|l| keys::parse(l).is_ok_and(|x| x.blob == k.blob)) {
            continue;
        }
        // The name given names the first key; the rest take their comments.
        let base = keys::item_name(if i == 0 { &add.name } else { "" }, k);
        let mut name = base.clone();
        let mut n = 2;
        while list.contains_key(&name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        list.insert(name.clone(), k.line());
        added.push(name);
    }
    if added.is_empty() {
        return Json(json!({"message": "already saved — nothing to add"})).into_response();
    }
    match keystore::store(&inner, &viewer, &list).await {
        Ok(copies) => Json(json!({"message": saved_message(&added, "saved", &copies), "added": added})).into_response(),
        Err(e) => bad_gateway(e),
    }
}

fn saved_message(names: &[String], verb: &str, copies: &[String]) -> String {
    let what = if names.len() == 1 { format!("key {}", names[0]) } else { format!("{} keys", names.len()) };
    if copies.is_empty() {
        format!("{what} {verb}")
    } else {
        format!("{what} {verb}; copies in {} updated", copies.join(", "))
    }
}

async fn keys_remove(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(item): Path<String>) -> Response {
    let mut list = match keystore::saved(&inner, &viewer).await {
        Ok(l) => l,
        Err(e) => return bad_gateway(e),
    };
    if list.remove(&item).is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no saved key {item}")}))).into_response();
    }
    match keystore::store(&inner, &viewer, &list).await {
        Ok(copies) => Json(json!({"message": saved_message(&[item], "deleted", &copies)})).into_response(),
        Err(e) => bad_gateway(e),
    }
}

/// The create form's per-key checkboxes: every key this viewer has, all
/// ticked, because the person creating a machine is the person who will
/// log into it.
async fn keys_choices(State(inner): State<Arc<Inner>>, viewer: Viewer) -> Response {
    let saved = keystore::saved(&inner, &viewer).await.unwrap_or_default();
    let mut options: Vec<Value> = Vec::new();
    for (list, from) in [(&saved, ""), (&config_keys(&viewer), " (console config)")] {
        for (name, line) in list.iter() {
            let short = keys::parse(line).map(|k| k.short()).unwrap_or_default();
            options.push(json!({"value": name, "label": format!("{name} — {short}{from}"), "checked": true}));
        }
    }
    let note = if options.is_empty() {
        "You have no saved keys. Add one under Account → SSH keys, or paste one below".to_string()
    } else {
        String::new()
    };
    Json(json!({"options": options, "note": note})).into_response()
}

/// Which keys a machine has, and from where: each `accessCredentials`
/// Secret and how it propagates, and the keys its cloud-init seed carries.
async fn vm_keys(State(inner): State<Arc<Inner>>, viewer: Viewer, Path((ns, name)): Path<(String, String)>) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let key = format!("{ns}/{name}");
    let machine = inner.store.object("vm", &key).await;
    let instance = inner.store.object("vmi", &key).await;
    let spec = machine
        .as_ref()
        .and_then(|m| m.pointer("/spec/template/spec"))
        .or_else(|| instance.as_ref().and_then(|v| v.get("spec")))
        .cloned()
        .unwrap_or(Value::Null);
    if machine.is_none() && instance.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no virtual machine {key}")}))).into_response();
    }
    let mut credentials = Vec::new();
    for (secret, how) in keys::credentials(&spec) {
        let (rows, error) = match keystore::read(&inner, &viewer, &ns, &secret).await {
            Ok(Some(s)) => (key_rows(&keys::from_secret(&s), "secret"), None),
            Ok(None) => (vec![], Some(format!("Secret {secret} does not exist in {ns}"))),
            Err(e) => (vec![], Some(e)),
        };
        credentials.push(json!({"secret": secret, "how": how, "keys": rows, "error": error}));
    }
    // The seed: where the keys actually come from today.
    let seed_name = spec
        .pointer("/volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|v| v.pointer("/cloudInitNoCloud/userDataSecretRef/name").and_then(Value::as_str))
        .map(str::to_string);
    let mut seed = Vec::new();
    if let Some(sn) = &seed_name {
        if let Ok(Some(s)) = keystore::read(&inner, &viewer, &ns, sn).await {
            for line in keys::from_seed(keys::from_secret(&s).get("userdata").map(String::as_str).unwrap_or("")) {
                let short = keys::parse(&line).map(|k| k.short()).unwrap_or_default();
                let comment = keys::parse(&line).map(|k| k.comment).unwrap_or_default();
                seed.push(json!({"short": short, "comment": comment, "line": line}));
            }
        }
    }
    // Whether the viewer's saved keys already reach this machine through a
    // credential, so the page offers "Add my keys" only when it would add.
    let mine = keys::secret_name(&keystore::user_of(&viewer));
    let has_mine = credentials.iter().any(|c| c["secret"] == mine.as_str());
    Json(json!({
        "credentials": credentials,
        "seed": seed,
        "seedSecret": seed_name,
        "hasMine": has_mine,
        "canAdd": machine.is_some() && viewer.may_write(),
        "running": instance.is_some(),
    }))
    .into_response()
}

/// "Add my keys" on a machine that exists: the viewer's Secret, copied into
/// the machine's namespace, named in its `accessCredentials` for the guest
/// agent to write.
///
/// Said as it is: cloud-init reads keys at first boot only, so for an
/// existing machine the agent is the one path, and no node honours it yet
/// (stormvm#41).
async fn vm_keys_add(State(inner): State<Arc<Inner>>, viewer: Viewer, Path((ns, name)): Path<(String, String)>) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let spec = match definition_spec(&inner, &ns, &name).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let list = match keystore::saved(&inner, &viewer).await {
        Ok(l) if !l.is_empty() => l,
        Ok(_) => {
            return (StatusCode::CONFLICT, Json(json!({"error": "you have no saved keys. Add one under Account → SSH keys first"})))
                .into_response()
        }
        Err(e) => return bad_gateway(e),
    };
    let secret = match keystore::ensure_copy(&inner, &viewer, &ns, &list).await {
        Ok(s) => s,
        Err(e) => return bad_gateway(e),
    };
    let mut creds: Vec<Value> = spec.pointer("/accessCredentials").and_then(Value::as_array).cloned().unwrap_or_default();
    if keys::credentials(&spec).iter().any(|(s, how)| s == &secret && how == "qemuGuestAgent") {
        return Json(json!({"message": format!("{name} already has your keys through the guest agent; the copy in {ns} is current")}))
            .into_response();
    }
    creds.push(keys::access_credential(&secret, true));
    let body = json!({"spec": {"template": {"spec": {"accessCredentials": creds}}}});
    let done = format!(
        "your {} key{} named on {name} for the guest agent to write to root's authorized_keys. \
         Nothing on a node does that yet (stormvm#41), and cloud-init reads keys only at first boot, \
         so until then this machine keeps the keys it was created with",
        list.len(),
        if list.len() == 1 { " is" } else { "s are" }
    );
    patch_machine(&inner, &viewer, &ns, &name, body, done).await
}

/// The Backup tab: this machine's snapshots, newest first, and the
/// restores made from them (#25).
async fn snapshots_list(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let key = format!("{ns}/{name}");
    let defined = inner.store.object("vm", &key).await.is_some();
    let running = inner.store.object("vmi", &key).await.is_some();
    if !defined && !running {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no virtual machine {key}")}))).into_response();
    }
    if inner.store.is_absent("vmsnap").await {
        return Json(json!({
            "available": false,
            "reason": "snapshot.kubevirt.io is not installed on this cluster, so there is nowhere to \
                       record a snapshot. The CRDs arrive with stormpump#28",
            "snapshots": [], "restores": [],
        }))
        .into_response();
    }
    let now = chrono::Utc::now();
    let prefix = format!("{ns}/");
    let mut snaps: Vec<Value> = inner
        .store
        .kind("vmsnap")
        .await
        .into_iter()
        .filter(|(k, o)| k.starts_with(&prefix) && snapshots::is_for(o, &name))
        .map(|(_, o)| snapshots::row(&o, now))
        .collect();
    snaps.sort_by(|a, b| b["created"].as_str().cmp(&a["created"].as_str()));
    let mut restores: Vec<Value> = inner
        .store
        .kind("vmrestore")
        .await
        .into_iter()
        .filter(|(k, o)| k.starts_with(&prefix) && snapshots::restore_is_for(o, &name))
        .map(|(_, o)| snapshots::restore_row(&o))
        .collect();
    restores.sort_by(|a, b| b["created"].as_str().cmp(&a["created"].as_str()));
    // Why Restore is greyed, said once for the whole list rather than on
    // every row.
    let restore_blocked = if !defined {
        Some("a bare instance has no definition to restore into")
    } else if running {
        Some("stop the machine to restore: a restore replaces its disks")
    } else {
        None
    };
    Json(json!({
        "available": true,
        "write": viewer.may_write(),
        "snapshots": snaps,
        "restores": restores,
        "restoreBlocked": restore_blocked,
    }))
    .into_response()
}

/// The Snapshot button. It schedules: the object is created and the node
/// does the rest, so this returns as soon as the apiserver has it.
async fn snapshot_take(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    Json(take): Json<snapshots::Take>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let key = format!("{ns}/{name}");
    let defined = inner.store.object("vm", &key).await.is_some();
    if !defined && inner.store.object("vmi", &key).await.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("no virtual machine {key}")}))).into_response();
    }
    let body = match snapshots::take_body(&ns, &name, defined, &take, chrono::Utc::now()) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"error": e}))).into_response(),
    };
    let snap = body["metadata"]["name"].as_str().unwrap_or_default().to_string();
    create_as_viewer(&inner, &viewer, &ns, "virtualmachinesnapshots", &body, format!("snapshot {snap} scheduled")).await
}

async fn snapshot_restore(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name, snap)): Path<(String, String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let key = format!("{ns}/{name}");
    let defined = inner.store.object("vm", &key).await.is_some();
    let running = inner.store.object("vmi", &key).await.is_some();
    let object = inner.store.object("vmsnap", &format!("{ns}/{snap}")).await;
    let body = match snapshots::restore_body(&ns, &name, defined, running, object.as_ref(), &snap, chrono::Utc::now()) {
        Ok(b) => b,
        Err(e) => return (StatusCode::CONFLICT, Json(json!({"error": e}))).into_response(),
    };
    create_as_viewer(&inner, &viewer, &ns, "virtualmachinerestores", &body, format!("restore of {name} from {snap} scheduled")).await
}

async fn snapshot_delete(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name, snap)): Path<(String, String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    // Only this machine's: the route names the machine, and a snapshot of
    // another one deleted through it would be a surprise.
    match inner.store.object("vmsnap", &format!("{ns}/{snap}")).await {
        Some(o) if snapshots::is_for(&o, &name) => {}
        _ => return (StatusCode::NOT_FOUND, Json(json!({"error": format!("{name} has no snapshot {snap}")}))).into_response(),
    }
    let Some(client) = &inner.client else { return no_apiserver() };
    match client
        .delete(&format!("{}/namespaces/{ns}/virtualmachinesnapshots/{snap}", snapshots::API), viewer.token.as_deref())
        .await
    {
        Ok(s) if s.is_success() => Json(json!({"message": format!("snapshot {snap} deleted")})).into_response(),
        Ok(s) => (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("apiserver returned {}", s.as_u16())})))
            .into_response(),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

/// Create a `snapshot.kubevirt.io` object, as the viewer, so the
/// apiserver's RBAC decides.
async fn create_as_viewer(inner: &Inner, viewer: &Viewer, ns: &str, plural: &str, body: &Value, done: String) -> Response {
    let Some(client) = &inner.client else { return no_apiserver() };
    let path = format!("{}/namespaces/{ns}/{plural}", snapshots::API);
    match client.post_json_as(&path, body, viewer.token.as_deref()).await {
        Ok((status, b)) => from_apiserver(status, b, &done),
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn console_caps(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    Json(
        console::for_vm(
            &inner.http,
            inner.stormvm.as_deref(),
            *inner.stormvm_up.read().await,
            &ns,
            &name,
            viewer.may_write(),
        )
        .await,
    )
    .into_response()
}

async fn door(
    inner: Arc<Inner>,
    viewer: Viewer,
    ws: WebSocketUpgrade,
    ns: String,
    name: String,
    kind: console::Door,
) -> Response {
    // A console is the most complete access a VM has: it must be gated by
    // the same rule as its row in the list.
    if let Some(refusal) = refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let Some(base) = inner.stormvm.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "no stormvm configured — set [vm] url"})),
        )
            .into_response();
    };
    // A viewer who may watch a console is not automatically one who may
    // type into it: on most guests the serial door is a root shell.
    let write = viewer.may_write();
    let token = console::mint(&inner.http, &base, kind, &ns, &name).await;
    let url = console::with_token(
        &console::ws_url(&base, &console::upstream_path(kind, &ns, &name)),
        token.as_deref(),
    );
    ws.on_upgrade(move |socket| console::relay(socket, url, write))
}

async fn serial(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    door(inner, viewer, ws, ns, name, console::Door::Serial).await
}

async fn vnc(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Response {
    door(inner, viewer, ws, ns, name, console::Door::Vnc).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_hidden_namespace_hides_its_vms() {
        let hidden: HashSet<String> = ["kube-system".to_string()].into_iter().collect();
        assert!(!visible_to("vm:instance:kube-system/dns-vm", &hidden));
        assert!(!visible_to("vm:machine:kube-system/dns-vm", &hidden));
        assert!(visible_to("vm:instance:team-a/web", &hidden));
        // Not this plugin's ids, and not this plugin's to hide.
        assert!(visible_to("k8s:pod:kube-system/x", &hidden));
        assert!(visible_to("plugin:vm", &hidden));
    }
}
