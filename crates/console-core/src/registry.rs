//! The plugin host: merges navigation, aggregates the component feed,
//! pushes snapshots to subscribers, and drives plugin background work.

use std::sync::Arc;
use std::time::Duration;

use stormview::{ComponentSummary, Health, Relation};
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::nav::{merge, NavSection};
use crate::plugin::ConsolePlugin;

pub struct Registry {
    plugins: Vec<Arc<dyn ConsolePlugin>>,
    snapshot: RwLock<Arc<Vec<ComponentSummary>>>,
    tx: broadcast::Sender<Arc<Vec<ComponentSummary>>>,
}

impl Registry {
    pub fn new(plugins: Vec<Arc<dyn ConsolePlugin>>) -> Self {
        let (tx, _) = broadcast::channel(16);
        Self { plugins, snapshot: RwLock::new(Arc::new(Vec::new())), tx }
    }

    pub fn plugins(&self) -> &[Arc<dyn ConsolePlugin>] {
        &self.plugins
    }

    pub fn nav(&self) -> Vec<NavSection> {
        merge(self.plugins.iter().flat_map(|p| p.nav()).collect())
    }

    /// Every plugin's creators, each stamped with its owner.
    pub fn creators(&self) -> Vec<crate::create::Creator> {
        self.plugins
            .iter()
            .flat_map(|p| {
                p.creators().into_iter().map(|mut c| {
                    c.plugin = p.name().to_string();
                    c
                })
            })
            .collect()
    }

    /// The current aggregated feed (cheap clone of an Arc).
    pub async fn components(&self) -> Arc<Vec<ComponentSummary>> {
        self.snapshot.read().await.clone()
    }

    /// Subscribe to full-snapshot pushes, stormd-style.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Vec<ComponentSummary>>> {
        self.tx.subscribe()
    }

    /// Worst plugin health — the console's readiness.
    pub async fn overall_health(&self) -> Health {
        let mut worst = Health::Ok;
        for p in &self.plugins {
            let h = p.health().await;
            if severity(h) < severity(worst) {
                worst = h;
            }
        }
        worst
    }

    /// Rebuild the aggregate: one component per plugin (the plugin card,
    /// owning `has_many` edges to its components), then every plugin's own
    /// slice. Publishes only when the snapshot changed.
    pub async fn refresh(&self) {
        let mut all: Vec<ComponentSummary> = Vec::new();
        for p in &self.plugins {
            let slice = p.components().await;
            for c in &slice {
                if !c.id.starts_with(&format!("{}:", p.name())) && c.id != p.name() {
                    warn!(plugin = p.name(), id = %c.id, "component id missing plugin prefix");
                }
            }
            let card = ComponentSummary {
                id: format!("plugin:{}", p.name()),
                kind: "plugin".to_string(),
                label: p.name().to_string(),
                health: p.health().await,
                detail: p.detail().await,
                metrics: vec![stormview::Metric::new("components", slice.len().to_string())],
                actions: vec![],
                relations: if slice.is_empty() {
                    vec![]
                } else {
                    vec![Relation::has_many(
                        "components",
                        slice.iter().map(|c| c.id.clone()).collect(),
                    )]
                },
                link: Some(format!("#/grid?id=plugin:{}", p.name())),
            };
            all.push(card);
            all.extend(slice);
        }

        let changed = { **self.snapshot.read().await != all };
        if changed {
            let arc = Arc::new(all);
            *self.snapshot.write().await = arc.clone();
            let _ = self.tx.send(arc);
        }
    }

    /// Spawn every plugin's background task, then refresh the aggregate on
    /// a fixed cadence until shutdown.
    pub async fn run(self: Arc<Self>, shutdown: CancellationToken) {
        for p in self.plugins.iter().cloned() {
            let token = shutdown.clone();
            tokio::spawn(async move { p.run(token).await });
        }
        loop {
            self.refresh().await;
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

pub(crate) fn severity(h: Health) -> u8 {
    match h {
        Health::Error => 0,
        Health::Warn => 1,
        Health::Ok => 2,
        Health::Idle => 3,
        Health::Unknown => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Stub;

    #[async_trait]
    impl ConsolePlugin for Stub {
        fn name(&self) -> &'static str {
            "stub"
        }
        async fn components(&self) -> Vec<ComponentSummary> {
            vec![ComponentSummary {
                id: "stub:thing".into(),
                kind: "thing".into(),
                label: "thing".into(),
                health: Health::Ok,
                detail: String::new(),
                metrics: vec![],
                actions: vec![],
                relations: vec![],
                link: None,
            }]
        }
    }

    #[tokio::test]
    async fn refresh_builds_plugin_card_plus_slice() {
        let r = Registry::new(vec![Arc::new(Stub)]);
        r.refresh().await;
        let feed = r.components().await;
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].id, "plugin:stub");
        assert_eq!(feed[1].id, "stub:thing");
    }
}
