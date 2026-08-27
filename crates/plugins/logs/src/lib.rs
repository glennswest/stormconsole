//! The logs plugin is the fleet log collector: it joins the stormcast
//! multicast group, parses RFC 5424, and stores into a bounded SQLite
//! ring. Query API patterned on mcastsyslog (`/events`, `/around`,
//! `/summary`, SSE `/stream`). Phase 3 implements the collector; per-entity
//! logs stay at their source (a node's stormd), reachable via the fleet
//! plugin's proxy.

use async_trait::async_trait;
use console_core::{ComponentSummary, ConsolePlugin, Health, NavSection};

pub struct LogsPlugin {
    mcast_group: String,
    db_path: String,
}

impl LogsPlugin {
    pub fn new(mcast_group: String, db_path: String) -> Self {
        Self { mcast_group, db_path }
    }
}

#[async_trait]
impl ConsolePlugin for LogsPlugin {
    fn name(&self) -> &'static str {
        "logs"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Observe", 30).item("Logs", "#/logs")]
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        vec![ComponentSummary {
            id: "logs:collector".into(),
            kind: "collector".into(),
            label: "fleet log collector".into(),
            health: Health::Idle,
            detail: format!("awaiting collector · group {} · {}", self.mcast_group, self.db_path),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: Some("#/logs".into()),
        }]
    }

    async fn health(&self) -> Health {
        Health::Idle
    }

    async fn detail(&self) -> String {
        format!("collector on {} (phase 3)", self.mcast_group)
    }
}
