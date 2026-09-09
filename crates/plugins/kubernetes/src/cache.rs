//! The watch-backed cache: one list+watch loop per resource kind, a shared
//! store of raw objects. The cache serves the UI instantly and the
//! components mapping reads a consistent snapshot; a broken watch re-lists
//! with backoff, so the store converges after any interruption.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::client::RkClient;

/// One watched resource.
///
/// `kind` is the console's short noun — the component id suffix, the hash
/// route (`#/k8s/pod`) and the key the SPA asks for. `title` is what a
/// page calls it. `namespaced` is the one fact the whole namespace
/// dimension turns on (issue #5), and it lives here because this is where
/// the resource is declared: the SPA used to carry three separate
/// hardcoded lists of it, which is three chances to disagree.
pub struct ResourceSpec {
    pub kind: &'static str,
    pub title: &'static str,
    pub list_path: &'static str,
    pub namespaced: bool,
    /// A CRD that may not be installed: a 404 means "none", not "broken",
    /// and the kind counts as synced with nothing in it.
    pub optional: bool,
    /// Counted in a namespace's inventory. Cluster-scoped kinds are not,
    /// and neither is the namespace itself.
    pub inventory: bool,
}

const fn ns_scoped(kind: &'static str, title: &'static str, list_path: &'static str) -> ResourceSpec {
    ResourceSpec { kind, title, list_path, namespaced: true, optional: false, inventory: true }
}

const fn cluster(kind: &'static str, title: &'static str, list_path: &'static str) -> ResourceSpec {
    ResourceSpec { kind, title, list_path, namespaced: false, optional: false, inventory: false }
}

const fn crd(
    kind: &'static str,
    title: &'static str,
    list_path: &'static str,
    namespaced: bool,
) -> ResourceSpec {
    ResourceSpec { kind, title, list_path, namespaced, optional: true, inventory: namespaced }
}

pub const RESOURCES: &[ResourceSpec] = &[
    cluster("ns", "Namespaces", "/api/v1/namespaces"),
    cluster("node", "Nodes", "/api/v1/nodes"),
    ns_scoped("pod", "Pods", "/api/v1/pods"),
    ns_scoped("deploy", "Deployments", "/apis/apps/v1/deployments"),
    ns_scoped("sts", "StatefulSets", "/apis/apps/v1/statefulsets"),
    ns_scoped("ds", "DaemonSets", "/apis/apps/v1/daemonsets"),
    ns_scoped("job", "Jobs", "/apis/batch/v1/jobs"),
    ns_scoped("cronjob", "CronJobs", "/apis/batch/v1/cronjobs"),
    ns_scoped("svc", "Services", "/api/v1/services"),
    ns_scoped("pvc", "PersistentVolumeClaims", "/api/v1/persistentvolumeclaims"),
    ns_scoped("cm", "ConfigMaps", "/api/v1/configmaps"),
    ns_scoped("netpol", "Network policies", "/apis/networking.k8s.io/v1/networkpolicies"),
    // A namespace's limits — what it may use, and what it is using. Both
    // are usually absent, and "no quota" is an answer worth showing
    // (issue #6) rather than an empty space.
    ns_scoped("quota", "Resource quotas", "/api/v1/resourcequotas"),
    ns_scoped("limits", "Limit ranges", "/api/v1/limitranges"),
    // Cilium, through its CRDs — the agent's own API is a unix socket and
    // Hubble is gRPC, neither reachable from a golden.
    crd("cep", "Cilium endpoints", "/apis/cilium.io/v2/ciliumendpoints", true),
    crd("cn", "Cilium nodes", "/apis/cilium.io/v2/ciliumnodes", false),
    crd("cid", "Cilium identities", "/apis/cilium.io/v2/ciliumidentities", false),
    crd("cnp", "Cilium network policies", "/apis/cilium.io/v2/ciliumnetworkpolicies", true),
    crd(
        "ccnp",
        "Cilium clusterwide policies",
        "/apis/cilium.io/v2/ciliumclusterwidenetworkpolicies",
        false,
    ),
];

impl ResourceSpec {
    /// The collection's group-version base and its plural, split out of
    /// the all-namespaces list path: `/apis/apps/v1/deployments` is
    /// `/apis/apps/v1` and `deployments`.
    pub fn split_path(&self) -> (&'static str, &'static str) {
        self.list_path.rsplit_once('/').unwrap_or(("", self.list_path))
    }

    /// Where one object of this kind lives, from the key the store uses.
    /// This is the path a YAML edit writes back to (#4).
    pub fn object_path(&self, key: &str) -> String {
        let (base, plural) = self.split_path();
        match (self.namespaced, key.split_once('/')) {
            (true, Some((ns, name))) => format!("{base}/namespaces/{ns}/{plural}/{name}"),
            _ => format!("{base}/{plural}/{key}"),
        }
    }
}

pub fn spec(kind: &str) -> Option<&'static ResourceSpec> {
    RESOURCES.iter().find(|r| r.kind == kind)
}

/// Is this kind scoped to a namespace? Unknown kinds are treated as
/// cluster-scoped, because filtering something by a namespace it does not
/// have hides it entirely.
pub fn is_namespaced(kind: &str) -> bool {
    spec(kind).map(|s| s.namespaced).unwrap_or(false)
}

/// The catalogue the SPA renders from: what exists, what to call it, and
/// whether the namespace selector applies to it.
pub fn catalogue() -> Vec<serde_json::Value> {
    RESOURCES
        .iter()
        .map(|r| {
            serde_json::json!({
                "kind": r.kind,
                "title": r.title,
                "namespaced": r.namespaced,
                "optional": r.optional,
                "inventory": r.inventory,
                "href": format!("#/k8s/{}", r.kind),
            })
        })
        .collect()
}

pub struct Store {
    /// kind → (ns/name or name → object)
    objects: RwLock<HashMap<&'static str, HashMap<String, Value>>>,
    /// kind → whether the initial list has completed since the last break
    synced: RwLock<HashMap<&'static str, bool>>,
    /// kind → the apiserver does not serve this resource at all. Only an
    /// optional kind can be absent, and the difference between "absent"
    /// and "present and empty" is the difference between "this cluster
    /// cannot do that" and "nobody has done it yet" — which is the whole
    /// answer a card is being asked for.
    absent: RwLock<HashMap<&'static str, bool>>,
    /// How many kinds this store is meant to hold, so "3/5 synced" is
    /// honest for a store that is not this plugin's own.
    kinds: usize,
}

impl Default for Store {
    fn default() -> Self {
        Self::with_kinds(RESOURCES.len())
    }
}

impl Store {
    pub fn with_kinds(kinds: usize) -> Self {
        Self {
            objects: RwLock::new(HashMap::new()),
            synced: RwLock::new(HashMap::new()),
            absent: RwLock::new(HashMap::new()),
            kinds,
        }
    }
}

pub fn object_key(obj: &Value) -> Option<String> {
    let meta = obj.get("metadata")?;
    let name = meta.get("name")?.as_str()?;
    match meta.get("namespace").and_then(Value::as_str) {
        Some(ns) => Some(format!("{ns}/{name}")),
        None => Some(name.to_string()),
    }
}

impl Store {
    pub async fn snapshot(&self) -> HashMap<&'static str, HashMap<String, Value>> {
        self.objects.read().await.clone()
    }

    pub async fn synced_kinds(&self) -> (usize, usize) {
        let s = self.synced.read().await;
        (s.values().filter(|v| **v).count(), self.kinds)
    }

    /// One kind's objects, keyed as the store keys them (`ns/name`, or
    /// `name` for a cluster-scoped kind).
    pub async fn kind(&self, kind: &str) -> HashMap<String, Value> {
        self.objects.read().await.get(kind).cloned().unwrap_or_default()
    }

    /// One object, or None when the kind is not watched or the key is gone.
    pub async fn object(&self, kind: &str, key: &str) -> Option<Value> {
        self.objects.read().await.get(kind)?.get(key).cloned()
    }

    /// Every namespace name the cache has seen.
    pub async fn namespaces(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .objects
            .read()
            .await
            .get("ns")
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// How many objects of `kind` live in `ns`. Keys are `ns/name`, so
    /// this is a prefix count and needs no per-object parse.
    pub async fn count_in(&self, kind: &str, ns: &str) -> usize {
        let prefix = format!("{ns}/");
        self.objects
            .read()
            .await
            .get(kind)
            .map(|m| m.keys().filter(|k| k.starts_with(&prefix)).count())
            .unwrap_or(0)
    }

    /// Is this kind one the apiserver does not serve? `false` for a kind
    /// that is served, whether or not anything of it exists.
    pub async fn is_absent(&self, kind: &str) -> bool {
        self.absent.read().await.get(kind).copied().unwrap_or(false)
    }

    async fn replace(&self, kind: &'static str, items: HashMap<String, Value>) {
        self.objects.write().await.insert(kind, items);
        self.synced.write().await.insert(kind, true);
        self.absent.write().await.insert(kind, false);
    }

    /// The apiserver does not serve this kind. The key is *removed*, not
    /// emptied: "absent" and "present and empty" have to be tellable
    /// apart by anything reading the snapshot, and an empty map that is
    /// present says the wrong one. The Cilium card turns on exactly this
    /// difference — a node that never ran Cilium must not get a Cilium
    /// card at all, let alone a failed one.
    async fn set_absent(&self, kind: &'static str) {
        self.objects.write().await.remove(kind);
        self.synced.write().await.insert(kind, true);
        self.absent.write().await.insert(kind, true);
    }

    async fn set_stale(&self, kind: &'static str) {
        self.synced.write().await.insert(kind, false);
    }

    async fn apply(&self, kind: &'static str, event: &str, obj: Value) {
        let Some(key) = object_key(&obj) else { return };
        let mut map = self.objects.write().await;
        let entry = map.entry(kind).or_default();
        match event {
            "ADDED" | "MODIFIED" => {
                entry.insert(key, obj);
            }
            "DELETED" => {
                entry.remove(&key);
            }
            _ => {}
        }
    }
}

/// list+watch forever: list seeds the store and yields the resourceVersion,
/// the watch applies deltas, any break re-lists after a backoff.
pub async fn watch_resource(
    client: RkClient,
    spec: &'static ResourceSpec,
    store: Arc<Store>,
    shutdown: CancellationToken,
) {
    let mut backoff = Duration::from_secs(1);
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        match client.get(spec.list_path).await {
            Ok(list) => {
                backoff = Duration::from_secs(1);
                let rv = list
                    .pointer("/metadata/resourceVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("0")
                    .to_string();
                let mut items = HashMap::new();
                for obj in list.get("items").and_then(Value::as_array).cloned().unwrap_or_default()
                {
                    if let Some(key) = object_key(&obj) {
                        items.insert(key, obj);
                    }
                }
                debug!(kind = spec.kind, count = items.len(), "listed");
                store.replace(spec.kind, items).await;

                // The watch writes into a channel and a serial consumer
                // applies deltas, so event order is preserved.
                let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, Value)>(256);
                let applier = {
                    let store = store.clone();
                    tokio::spawn(async move {
                        while let Some((event, obj)) = rx.recv().await {
                            store.apply(spec.kind, &event, obj).await;
                        }
                    })
                };
                let result = client.watch(spec.list_path, &rv, &shutdown, tx).await;
                let _ = applier.await;
                if shutdown.is_cancelled() {
                    return;
                }
                if let Err(e) = result {
                    warn!(kind = spec.kind, error = %e, "watch broke");
                }
                store.set_stale(spec.kind).await;
            }
            Err(crate::client::RkError::Status(st)) if spec.optional && st.as_u16() == 404 => {
                // Not installed. Synced, empty; look again in a minute in
                // case someone installs it.
                debug!(kind = spec.kind, "CRD not served — treating as empty");
                store.set_absent(spec.kind).await;
                backoff = Duration::from_secs(60);
            }
            Err(e) => {
                warn!(kind = spec.kind, error = %e, "list failed");
                store.set_stale(spec.kind).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown.cancelled() => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_path_is_derived_from_the_list_path() {
        assert_eq!(spec("pod").unwrap().object_path("default/web"), "/api/v1/namespaces/default/pods/web");
        assert_eq!(
            spec("deploy").unwrap().object_path("kube-system/dns"),
            "/apis/apps/v1/namespaces/kube-system/deployments/dns"
        );
        assert_eq!(spec("node").unwrap().object_path("storm-1"), "/api/v1/nodes/storm-1");
        assert_eq!(spec("ns").unwrap().object_path("default"), "/api/v1/namespaces/default");
        assert_eq!(
            spec("cnp").unwrap().object_path("default/allow-dns"),
            "/apis/cilium.io/v2/namespaces/default/ciliumnetworkpolicies/allow-dns"
        );
        assert_eq!(
            spec("ccnp").unwrap().object_path("deny-all"),
            "/apis/cilium.io/v2/ciliumclusterwidenetworkpolicies/deny-all"
        );
    }
}
