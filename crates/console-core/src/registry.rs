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
use crate::storage::{self, Decision, Guarded, Reviewer};

pub struct Registry {
    plugins: Vec<Arc<dyn ConsolePlugin>>,
    snapshot: RwLock<Arc<Vec<ComponentSummary>>>,
    tx: broadcast::Sender<Arc<Vec<ComponentSummary>>>,
    reviewer: Reviewer,
}

/// What the console says about one request before it is made: whether it
/// is destructive storage, whether this viewer may, and what to type.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Guard {
    #[serde(flatten)]
    pub guarded: Guarded,
    #[serde(flatten)]
    pub decision: Decision,
    /// The word the person types to confirm: the drive's serial, or the
    /// object's name.
    pub confirm: String,
}

impl Registry {
    /// A registry with no apiserver to ask: destructive storage is refused
    /// to everyone. [`Registry::with_reviewer`] gives it one.
    pub fn new(plugins: Vec<Arc<dyn ConsolePlugin>>) -> Self {
        let (tx, _) = broadcast::channel(16);
        Self { plugins, snapshot: RwLock::new(Arc::new(Vec::new())), tx, reviewer: Reviewer::new(None, false) }
    }

    /// Ask this reviewer whether a viewer is a storage-admin (#82).
    pub fn with_reviewer(mut self, reviewer: Reviewer) -> Self {
        self.reviewer = reviewer;
        self
    }

    /// Is this request destructive storage, may this viewer, and what must
    /// they type? `None` for everything that is not destructive storage.
    pub async fn guard(&self, viewer: &Viewer, method: &axum::http::Method, path: &str, query: Option<&str>) -> Option<Guard> {
        let guarded = storage::classify(method, path, query)?;
        let decision = self.reviewer.review(viewer, &guarded.resource, &guarded.verb).await;
        let confirm = self.confirm_word(method.as_str(), path, &guarded).await;
        Some(Guard { guarded, decision, confirm })
    }

    /// What to type to confirm: taken from the component that offers this
    /// action — its serial where it has one (a drive), else its label — so
    /// the word is the one on the screen and not an id from the path.
    async fn confirm_word(&self, method: &str, path: &str, guarded: &Guarded) -> String {
        let all = self.components().await;
        let owner = all.iter().find(|c| {
            c.actions.iter().any(|a| a.method.eq_ignore_ascii_case(method) && a.path.split('?').next() == Some(path))
        });
        match owner {
            Some(c) => c
                .metrics
                .iter()
                .find(|m| m.label == "serial" && !m.value.trim().is_empty())
                .map(|m| m.value.trim().to_string())
                .unwrap_or_else(|| c.label.clone()),
            None => guarded.target.clone(),
        }
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
        let all = self.scoped_for(viewer).await;
        self.strip_storage(viewer, all).await
    }

    /// Drop every destructive storage action this viewer may not take
    /// (#82): shown only to storage-admins, so a viewer sees the drive and
    /// not the Format button. One review per distinct resource and verb,
    /// and the shared Arc comes straight back when nothing is withheld.
    async fn strip_storage(&self, viewer: &Viewer, all: Arc<Vec<ComponentSummary>>) -> Arc<Vec<ComponentSummary>> {
        use std::collections::HashMap;
        let mut asked: HashMap<(String, String), bool> = HashMap::new();
        let mut refused = false;
        for c in all.iter() {
            for a in &c.actions {
                let Some(g) = classify_action(a) else { continue };
                let key = (g.resource, g.verb);
                if !asked.contains_key(&key) {
                    let ok = self.reviewer.review(viewer, &key.0, &key.1).await.allowed;
                    refused |= !ok;
                    asked.insert(key, ok);
                }
            }
        }
        if !refused {
            return all;
        }
        let mut out = all.as_ref().clone();
        for c in &mut out {
            c.actions.retain(|a| match classify_action(a) {
                Some(g) => asked.get(&(g.resource, g.verb)).copied().unwrap_or(false),
                None => true,
            });
        }
        Arc::new(out)
    }

    async fn scoped_for(&self, viewer: &Viewer) -> Arc<Vec<ComponentSummary>> {
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
    /// What happened to one object.
    ///
    /// Asked of every plugin in turn; the first to claim the id answers.
    /// Nobody claiming it is itself an answer — said in terms of where
    /// events come from here, because "none" in front of a volume that
    /// has just failed to attach is a statement somebody will act on.
    pub async fn events(&self, viewer: &Viewer, id: &str) -> crate::Events {
        for p in &self.plugins {
            if let Some(e) = p.events(viewer, id).await {
                return e;
            }
        }
        crate::events::unclaimed(id)
    }

    /// Recent activity across every plugin that records any, newest
    /// first. Merged rather than first-wins: the dock is the one place
    /// that wants everything at once.
    pub async fn recent_events(&self, viewer: &Viewer) -> crate::Events {
        let mut all = Vec::new();
        let mut said = Vec::new();
        for p in &self.plugins {
            match p.recent_events(viewer).await {
                Some(e) if e.available => all.extend(e.items),
                Some(e) => said.push(e.reason),
                None => {}
            }
        }
        if all.is_empty() && !said.is_empty() {
            return crate::Events::none(said.join(" · "));
        }
        crate::Events::of(all).newest(80)
    }

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

/// An action in the feed, classified by the rule the host enforces.
fn classify_action(a: &stormview::Action) -> Option<Guarded> {
    let method = axum::http::Method::from_bytes(a.method.to_ascii_uppercase().as_bytes()).ok()?;
    let (path, query) = match a.path.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (a.path.as_str(), None),
    };
    storage::classify(&method, path, query)
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

        let viewer =
            Viewer { user: Some("gw".into()), token: Some("t".into()), ..Default::default() };
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
        let viewer =
            Viewer { user: Some("gw".into()), token: Some("t".into()), ..Default::default() };
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

    struct Drives;

    #[async_trait]
    impl ConsolePlugin for Drives {
        fn name(&self) -> &'static str {
            "drive"
        }
        async fn components(&self) -> Vec<ComponentSummary> {
            let act = |id: &str, path: &str| stormview::Action {
                id: id.into(),
                label: id.into(),
                method: "POST".into(),
                path: format!("/api/plugins/drive/proxy/api/v1/drives/7f3a/{path}"),
                enabled: true,
                danger: false,
                tone: None,
            };
            vec![ComponentSummary {
                id: "drive:7f3a".into(),
                kind: "drive".into(),
                label: "sdb · ST4000".into(),
                health: Health::Ok,
                detail: String::new(),
                metrics: vec![stormview::Metric::new("serial", "ZC1234")],
                actions: vec![act("locate-on", "locate/on"), act("format-4k", "format/4096")],
                relations: vec![],
                link: None,
            }]
        }
    }

    /// With nobody to ask, nobody is a storage-admin: the drive is shown,
    /// Locate stays, Format goes — for an administrator of the console too.
    #[tokio::test]
    async fn destructive_storage_is_withheld_from_whoever_the_review_refuses() {
        let r = Registry::new(vec![Arc::new(Drives)]);
        r.refresh().await;
        let admin = Viewer { user: Some("root".into()), token: Some("t".into()), roles: vec!["admin".into()], ..Default::default() };
        let seen = r.components_for(&admin).await;
        let d = seen.iter().find(|c| c.id == "drive:7f3a").unwrap();
        let ids: Vec<&str> = d.actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["locate-on"]);

        let m = axum::http::Method::POST;
        let g = r.guard(&admin, &m, "/api/plugins/drive/proxy/api/v1/drives/7f3a/format/4096", None).await.unwrap();
        assert!(!g.decision.allowed && g.decision.reason.contains("no apiserver"));
        assert_eq!(g.confirm, "ZC1234", "the serial, not the id in the path");
        assert!(r.guard(&admin, &m, "/api/plugins/drive/proxy/api/v1/drives/7f3a/locate/on", None).await.is_none());
        // A request no component offers is confirmed by the id it names.
        let g = r.guard(&admin, &m, "/api/plugins/drive/proxy/api/v1/drives/9999/sanitize", None).await.unwrap();
        assert_eq!(g.confirm, "9999");
    }
}
