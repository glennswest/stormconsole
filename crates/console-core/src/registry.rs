//! The plugin host: merges navigation, aggregates the component feed,
//! pushes snapshots to subscribers, and drives plugin background work.

use std::sync::Arc;
use std::time::Duration;

use stormview::{ComponentSummary, Health, Relation};
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::access::{Access, Viewer};
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

    /// The feed as one viewer may see it. Every plugin that limits this
    /// viewer has its components dropped and every surviving relation
    /// re-pointed, so a hidden object is not reachable by following an
    /// edge either. Cheap when nothing is limited: the shared Arc comes
    /// straight back.
    pub async fn components_for(&self, viewer: &Viewer) -> Arc<Vec<ComponentSummary>> {
        let all = self.components().await;
        let limits = self.limits(viewer).await;
        if limits.is_empty() {
            return all;
        }
        let visible = |id: &str| match limits.iter().find(|(name, _)| owns(name, id)) {
            Some((_, access)) => access.allows(id),
            None => true,
        };
        let mut out: Vec<ComponentSummary> =
            all.iter().filter(|c| visible(&c.id)).cloned().collect();
        let kept: std::collections::HashSet<&str> = out.iter().map(|c| c.id.as_str()).collect();
        let kept: std::collections::HashSet<String> = kept.into_iter().map(str::to_string).collect();
        for c in &mut out {
            for r in &mut c.relations {
                r.targets.retain(|t| kept.contains(t));
            }
            c.relations.retain(|r| !r.targets.is_empty());
        }
        // A plugin card's component count is the viewer's count, not the
        // cluster's — a card claiming 40 pods over a list of 6 is worse
        // than saying 6.
        for c in &mut out {
            if c.kind == "plugin" {
                if let Some(name) = c.id.strip_prefix("plugin:") {
                    let n = kept.iter().filter(|id| owns(name, id) && *id != &c.id).count();
                    if let Some(m) = c.metrics.iter_mut().find(|m| m.label == "components") {
                        m.value = n.to_string();
                    }
                }
            }
        }
        Arc::new(out)
    }

    /// Every plugin that limits this viewer, with its answer.
    async fn limits(&self, viewer: &Viewer) -> Vec<(&'static str, Access)> {
        let mut out = Vec::new();
        for p in &self.plugins {
            let a = p.access(viewer).await;
            if !a.is_unrestricted() {
                out.push((p.name(), a));
            }
        }
        out
    }

    /// What this viewer is not being shown, per plugin — so the UI can say
    /// "3 namespaces you cannot view" instead of showing a short list that
    /// reads like a broken console.
    pub async fn access_report(&self, viewer: &Viewer) -> serde_json::Value {
        let limits = self.limits(viewer).await;
        let plugins: serde_json::Map<String, serde_json::Value> = limits
            .iter()
            .map(|(name, a)| {
                (
                    name.to_string(),
                    serde_json::json!({"hidden": a.hidden(), "note": a.note()}),
                )
            })
            .collect();
        // No aggregate count: two plugins hiding the same four namespaces
        // would sum to eight, which is a number that means nothing. Each
        // plugin's own count and line stand on their own, and `enforced`
        // answers "is anything being withheld at all".
        serde_json::json!({
            "enforced": !limits.is_empty(),
            "identified": !viewer.is_anonymous(),
            "plugins": plugins,
        })
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

/// Does `plugin` own this component id? Its own card, or anything under
/// its `{name}:` prefix — the same rule `refresh` warns about.
fn owns(plugin: &str, id: &str) -> bool {
    id == plugin
        || id.starts_with(&format!("{plugin}:"))
        || id == format!("plugin:{plugin}")
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

    struct Scoped;

    #[async_trait]
    impl ConsolePlugin for Scoped {
        fn name(&self) -> &'static str {
            "sc"
        }
        async fn components(&self) -> Vec<ComponentSummary> {
            ["sc:a", "sc:b"]
                .iter()
                .map(|id| ComponentSummary {
                    id: (*id).into(),
                    kind: "thing".into(),
                    label: (*id).into(),
                    health: Health::Ok,
                    detail: String::new(),
                    metrics: vec![],
                    actions: vec![],
                    relations: vec![Relation::has_many(
                        "peers",
                        vec!["sc:a".into(), "sc:b".into()],
                    )],
                    link: None,
                })
                .collect()
        }
        async fn access(&self, viewer: &Viewer) -> Access {
            if viewer.is_anonymous() {
                Access::Unrestricted
            } else {
                Access::limited(|id| id != "sc:b", 1, "1 thing you cannot view")
            }
        }
    }

    #[tokio::test]
    async fn a_limited_viewer_never_receives_the_hidden_component() {
        let r = Registry::new(vec![Arc::new(Scoped)]);
        r.refresh().await;

        let open = r.components_for(&Viewer::anonymous()).await;
        assert_eq!(open.len(), 3, "card + two things");

        let viewer = Viewer { user: Some("gw".into()), token: Some("t".into()) };
        let seen = r.components_for(&viewer).await;
        let ids: Vec<&str> = seen.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["plugin:sc", "sc:a"]);
        // The edge to the hidden component is gone too, so it cannot be
        // reached by following a relation.
        let a = seen.iter().find(|c| c.id == "sc:a").unwrap();
        assert_eq!(a.relations[0].targets, vec!["sc:a"]);
        // And the card counts what this viewer can see.
        let card = seen.iter().find(|c| c.id == "plugin:sc").unwrap();
        assert_eq!(card.metrics[0].value, "1");
    }

    #[tokio::test]
    async fn the_report_says_what_is_hidden_and_whether_anything_is_enforced() {
        let r = Registry::new(vec![Arc::new(Scoped)]);
        r.refresh().await;
        let open = r.access_report(&Viewer::anonymous()).await;
        assert_eq!(open["enforced"], false);
        assert_eq!(open["identified"], false);
        let viewer = Viewer { user: Some("gw".into()), token: Some("t".into()) };
        let closed = r.access_report(&viewer).await;
        assert_eq!(closed["enforced"], true);
        assert_eq!(closed["plugins"]["sc"]["hidden"], 1);
        assert_eq!(closed["plugins"]["sc"]["note"], "1 thing you cannot view");
        assert!(closed.get("hidden").is_none(), "no meaningless cross-plugin sum");
    }

    #[test]
    fn ownership_is_the_prefix_rule_the_feed_already_uses() {
        assert!(owns("k8s", "k8s:pod:default/web"));
        assert!(owns("k8s", "plugin:k8s"));
        assert!(!owns("k8s", "k8sx:thing"));
        assert!(!owns("sb", "k8s:pod:default/web"));
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
