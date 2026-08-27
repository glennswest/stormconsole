//! The kubernetes plugin: rustkube apiserver views. Phase 1 establishes
//! the connection (probe on /version); Phase 2 adds the watch-backed cache
//! and turns namespaces, workloads, and nodes into components.
//!
//! rustkube only — this console has no other orchestrator.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::{ComponentSummary, ConsolePlugin, Health, NavSection, Probe};
use tokio_util::sync::CancellationToken;

pub struct KubernetesPlugin {
    server: Option<String>,
    probe: Option<Arc<Probe>>,
    client: reqwest::Client,
}

impl KubernetesPlugin {
    pub fn new(server: Option<String>, token: Option<String>, insecure: bool) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(t) = &token {
            if let Ok(v) = format!("Bearer {t}").parse() {
                headers.insert(reqwest::header::AUTHORIZATION, v);
            }
        }
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(insecure)
            .build()
            .expect("reqwest client");
        let probe = server
            .as_ref()
            .map(|s| Arc::new(Probe::new(format!("{}/version", s.trim_end_matches('/')))));
        Self { server, probe, client }
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
            NavSection::new("Workloads", 10).item("All workloads", "#/grid?id=plugin:k8s"),
        ]
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        // Phase 2: the watch cache emits namespaces, pods, workloads, nodes
        // here. Until then the apiserver itself is the one component.
        let (health, detail) = match &self.probe {
            Some(p) => {
                let s = p.state().await;
                (s.health, format!("rustkube apiserver · {}", s.detail))
            }
            None => (Health::Idle, "no rustkube endpoint configured".to_string()),
        };
        vec![ComponentSummary {
            id: "k8s:apiserver".into(),
            kind: "apiserver".into(),
            label: "rustkube".into(),
            health,
            detail,
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        }]
    }

    async fn health(&self) -> Health {
        match &self.probe {
            Some(p) => p.state().await.health,
            None => Health::Idle,
        }
    }

    async fn detail(&self) -> String {
        match &self.server {
            Some(s) => format!("rustkube at {s}"),
            None => "no rustkube endpoint configured".to_string(),
        }
    }

    async fn run(&self, shutdown: CancellationToken) {
        match &self.probe {
            Some(p) => p.run(self.client.clone(), Duration::from_secs(10), shutdown).await,
            None => shutdown.cancelled().await,
        }
    }
}
