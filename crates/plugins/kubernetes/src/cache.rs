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

/// One watched resource. `kind` is the console's short noun (also the
/// component kind); `list_path` lists across all namespaces.
pub struct ResourceSpec {
    pub kind: &'static str,
    pub list_path: &'static str,
    pub namespaced: bool,
}

pub const RESOURCES: &[ResourceSpec] = &[
    ResourceSpec { kind: "ns", list_path: "/api/v1/namespaces", namespaced: false },
    ResourceSpec { kind: "node", list_path: "/api/v1/nodes", namespaced: false },
    ResourceSpec { kind: "pod", list_path: "/api/v1/pods", namespaced: true },
    ResourceSpec { kind: "deploy", list_path: "/apis/apps/v1/deployments", namespaced: true },
    ResourceSpec { kind: "sts", list_path: "/apis/apps/v1/statefulsets", namespaced: true },
    ResourceSpec { kind: "ds", list_path: "/apis/apps/v1/daemonsets", namespaced: true },
    ResourceSpec { kind: "job", list_path: "/apis/batch/v1/jobs", namespaced: true },
    ResourceSpec { kind: "cronjob", list_path: "/apis/batch/v1/cronjobs", namespaced: true },
    ResourceSpec { kind: "svc", list_path: "/api/v1/services", namespaced: true },
    ResourceSpec { kind: "pvc", list_path: "/api/v1/persistentvolumeclaims", namespaced: true },
];

#[derive(Default)]
pub struct Store {
    /// kind → (ns/name or name → object)
    objects: RwLock<HashMap<&'static str, HashMap<String, Value>>>,
    /// kind → whether the initial list has completed since the last break
    synced: RwLock<HashMap<&'static str, bool>>,
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
        (s.values().filter(|v| **v).count(), RESOURCES.len())
    }

    async fn replace(&self, kind: &'static str, items: HashMap<String, Value>) {
        self.objects.write().await.insert(kind, items);
        self.synced.write().await.insert(kind, true);
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
