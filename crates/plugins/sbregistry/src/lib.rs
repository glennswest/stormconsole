//! The sbregistry plugin: the image side — goldens, clones, pallets,
//! warm-up state. sbregistry does not yet serve a stormview components
//! feed (stormblock-registry#24); until it does, Phase 5 maps its JSON
//! (`/v1/goldens`, `/v1/clones`, `/v1/pallets`, `/v1/warmup`) to
//! components here.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use console_core::{ComponentSummary, ConsolePlugin, Health, NavSection, Probe};
use tokio_util::sync::CancellationToken;

pub struct SbregistryPlugin {
    url: Option<String>,
    probe: Option<Arc<Probe>>,
    client: reqwest::Client,
}

impl SbregistryPlugin {
    pub fn new(url: Option<String>) -> Self {
        let probe = url
            .as_ref()
            .map(|u| Arc::new(Probe::new(format!("{}/readyz", u.trim_end_matches('/')))));
        Self { url, probe, client: reqwest::Client::new() }
    }
}

#[async_trait]
impl ConsolePlugin for SbregistryPlugin {
    fn name(&self) -> &'static str {
        "reg"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Images", 50).item("Registry", "#/grid?id=plugin:reg")]
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        let (health, detail) = match &self.probe {
            Some(p) => {
                let s = p.state().await;
                (s.health, format!("sbregistry · {}", s.detail))
            }
            None => (Health::Idle, "no sbregistry endpoint configured".to_string()),
        };
        vec![ComponentSummary {
            id: "reg:registry".into(),
            kind: "registry".into(),
            label: "sbregistry".into(),
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
            Some(u) => format!("registry at {u}"),
            None => "no sbregistry endpoint configured".to_string(),
        }
    }

    async fn run(&self, shutdown: CancellationToken) {
        match &self.probe {
            Some(p) => p.run(self.client.clone(), Duration::from_secs(10), shutdown).await,
            None => shutdown.cancelled().await,
        }
    }
}
