//! The stormblock plugin: the block engine's volumes, exports, LUNs,
//! slabs, and arrays (:9090), rendered as components with typed edges
//! (array → slabs → volumes → exports). Storage health is the engine's own
//! readiness — never a cached row.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::{ComponentSummary, ConsolePlugin, Health, NavSection, Probe};
use tokio_util::sync::CancellationToken;

pub struct StormblockPlugin {
    url: Option<String>,
    probe: Option<Arc<Probe>>,
    client: reqwest::Client,
}

impl StormblockPlugin {
    pub fn new(url: Option<String>) -> Self {
        let probe = url
            .as_ref()
            .map(|u| Arc::new(Probe::new(format!("{}/api/v1/volumes", u.trim_end_matches('/')))));
        Self { url, probe, client: reqwest::Client::new() }
    }
}

#[async_trait]
impl ConsolePlugin for StormblockPlugin {
    fn name(&self) -> &'static str {
        "sb"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Storage", 40).item("Block engine", "#/grid?id=plugin:sb")]
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let (health, detail) = match &self.probe {
            Some(p) => {
                let s = p.state().await;
                (s.health, format!("block engine · {}", s.detail))
            }
            None => (Health::Idle, "no stormblock endpoint configured".to_string()),
        };
        vec![ComponentSummary {
            id: "sb:engine".into(),
            kind: "engine".into(),
            label: "stormblock".into(),
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
        match &self.url {
            Some(u) => format!("engine at {u}"),
            None => "no stormblock endpoint configured".to_string(),
        }
    }

    async fn run(&self, shutdown: CancellationToken) {
        match &self.probe {
            Some(p) => p.run(self.client.clone(), Duration::from_secs(10), shutdown).await,
            None => shutdown.cancelled().await,
        }
    }
}
