//! The stormdrive plugin: physical drives fleet-wide. Endpoints come from
//! fleet discovery (one stormdrive per storage node on :9092), so this
//! plugin needs no configuration — Phase 5 fans out over discovered
//! storage nodes and namespaces drives per node (`drive:{node}:{id}`).

use async_trait::async_trait;
use console_core::{ComponentSummary, ConsolePlugin, Health, NavSection};

pub struct StormdrivePlugin;

impl StormdrivePlugin {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ConsolePlugin for StormdrivePlugin {
    fn name(&self) -> &'static str {
        "drive"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Storage", 40).item("Drives", "#/grid?id=plugin:drive")]
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        vec![ComponentSummary {
            id: "drive:aggregator".into(),
            kind: "aggregator".into(),
            label: "drive aggregation".into(),
            health: Health::Idle,
            detail: "awaiting fleet discovery (phase 5)".into(),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        }]
    }

    async fn health(&self) -> Health {
        Health::Idle
    }

    async fn detail(&self) -> String {
        "per-node stormdrive aggregation (phase 5)".into()
    }
}
