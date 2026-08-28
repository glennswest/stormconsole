//! The kubernetes plugin: rustkube apiserver views through a watch-backed
//! cache. rustkube only — this console has no other orchestrator.
//!
//! One list+watch loop per resource kind feeds a shared store; the
//! components mapping renders a consistent snapshot with health derived
//! from the same conditions kubectl reads. Actions surface as POST routes
//! under /api/plugins/k8s so any stormview renderer can wire them.

mod cache;
mod client;
mod components;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use console_core::{ComponentSummary, ConsolePlugin, Health, NavSection, Probe};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use cache::{watch_resource, Store, RESOURCES};
use client::RkClient;

struct Inner {
    server: Option<String>,
    client: Option<RkClient>,
    probe: Option<Probe>,
    store: Arc<Store>,
    http: reqwest::Client,
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
            NavSection::new("Networking", 25).item("Services", "#/k8s/svc"),
            NavSection::new("Compute", 20).item("Cluster nodes", "#/k8s/node"),
            NavSection::new("Storage", 40).item("PVCs", "#/k8s/pvc"),
            NavSection::new("Observe", 30).item("Events", "#/k8s/events"),
            NavSection::new("Administration", 60).item("Namespaces", "#/k8s/ns"),
        ]
    }

    fn routes(&self) -> Router {
        Router::new()
            .route("/pods/{ns}/{name}/delete", post(delete_pod))
            .route("/events", get(events))
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
            Some(s) => format!("rustkube at {s}"),
            None => "no rustkube endpoint configured".to_string(),
        }
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
