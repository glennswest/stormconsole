//! The fleet plugin: nodes announce themselves by existing on the
//! stormcast multicast group — there is no inventory service, by design.
//! Phase 4 derives the node set from group traffic (host + source address),
//! proxies into each node's own daemons (stormd :9080, stormdrive :9092,
//! stormblock :9090), and carries the day-2 actions from stormcos
//! CLUSTER.md: join, promote, demote, drain.

use async_trait::async_trait;
use console_core::{ComponentSummary, ConsolePlugin, Health, NavSection};

pub struct FleetPlugin {
    mcast_group: String,
}

impl FleetPlugin {
    pub fn new(mcast_group: String) -> Self {
        Self { mcast_group }
    }
}

#[async_trait]
impl ConsolePlugin for FleetPlugin {
    fn name(&self) -> &'static str {
        "fleet"
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new("Compute", 20).item("Nodes", "#/grid?id=plugin:fleet")]
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        vec![ComponentSummary {
            id: "fleet:discovery".into(),
            kind: "discovery".into(),
            label: "node discovery".into(),
            health: Health::Idle,
            detail: format!("awaiting collector · group {}", self.mcast_group),
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
        format!("discovery on {} (phase 4)", self.mcast_group)
    }
}
