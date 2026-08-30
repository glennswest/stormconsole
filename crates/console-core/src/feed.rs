//! A stormview components feed served by another daemon, folded into this
//! console's feed. stormd, stormdrive and stormstorage all serve
//! `/api/v1/components`; the console re-prefixes their ids so the aggregate
//! stays collision-free, and routes their actions through the owning
//! plugin's proxy so the browser only ever talks to the console.

use std::sync::Arc;
use std::time::Duration;

use stormview::{ComponentSummary, Health};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::nav::NavSection;
use crate::plugin::ConsolePlugin;

#[derive(Debug, Clone)]
pub struct FeedState {
    pub health: Health,
    pub detail: String,
    pub components: Vec<ComponentSummary>,
}

impl Default for FeedState {
    fn default() -> Self {
        Self { health: Health::Unknown, detail: "not yet polled".to_string(), components: vec![] }
    }
}

pub struct Feed {
    /// Upstream base URL, no trailing slash.
    pub base: String,
    /// Id prefix for everything from this feed, e.g. "drive".
    pub prefix: String,
    /// Where this feed's actions go through the console, e.g.
    /// "/api/plugins/drive/proxy".
    pub proxy_base: String,
    state: RwLock<FeedState>,
}

impl Feed {
    pub fn new(base: &str, prefix: &str, proxy_base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            prefix: prefix.to_string(),
            proxy_base: proxy_base.trim_end_matches('/').to_string(),
            state: RwLock::new(FeedState::default()),
        }
    }

    pub async fn state(&self) -> FeedState {
        self.state.read().await.clone()
    }

    pub async fn components(&self) -> Vec<ComponentSummary> {
        self.state.read().await.components.clone()
    }

    /// One poll. An upstream that cannot be reached shows as an error with
    /// no components — stale rows would say the opposite of the truth.
    pub async fn poll(&self, client: &reqwest::Client) {
        let url = format!("{}/api/v1/components", self.base);
        let observed = match client.get(&url).timeout(Duration::from_secs(5)).send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<Vec<ComponentSummary>>().await {
                Ok(list) => {
                    let (health, detail) = summarize(&list);
                    FeedState { health, detail, components: remap(list, &self.prefix, &self.proxy_base) }
                }
                Err(e) => FeedState {
                    health: Health::Warn,
                    detail: format!("feed unreadable: {}", concise(&e)),
                    components: vec![],
                },
            },
            Ok(resp) => FeedState {
                health: Health::Warn,
                detail: format!("responded {}", resp.status()),
                components: vec![],
            },
            Err(e) => FeedState {
                health: Health::Error,
                detail: format!("unreachable: {}", concise(&e)),
                components: vec![],
            },
        };
        *self.state.write().await = observed;
    }

    pub async fn run(&self, client: reqwest::Client, interval: Duration, shutdown: CancellationToken) {
        loop {
            self.poll(&client).await;
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

/// The upstream's own `system` card is its verdict on itself; without one,
/// the worst component health stands in.
fn summarize(list: &[ComponentSummary]) -> (Health, String) {
    if let Some(sys) = list.iter().find(|c| c.id == "system") {
        return (sys.health, sys.detail.clone());
    }
    let worst = list
        .iter()
        .map(|c| c.health)
        .min_by_key(|h| crate::registry::severity(*h))
        .unwrap_or(Health::Idle);
    (worst, format!("{} components", list.len()))
}

/// Re-prefix ids and relation targets; send actions through the proxy;
/// drop links, which are hash routes inside the upstream's own UI.
pub fn remap(list: Vec<ComponentSummary>, prefix: &str, proxy_base: &str) -> Vec<ComponentSummary> {
    list.into_iter()
        .map(|mut c| {
            c.id = format!("{prefix}:{}", c.id);
            for r in &mut c.relations {
                r.targets = r.targets.iter().map(|t| format!("{prefix}:{t}")).collect();
                r.href = None;
            }
            for a in &mut c.actions {
                a.path = format!("{proxy_base}/{}", a.path.trim_start_matches('/'));
            }
            c.link = None;
            c
        })
        .collect()
}

fn concise(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
}

/// A plugin that is exactly one upstream feed: name, a nav item, the URL.
/// stormdrive and stormstorage are this; a node's stormd instances are
/// this several times over.
pub struct FeedPlugin {
    name: &'static str,
    section: &'static str,
    order: i32,
    item: &'static str,
    feed: Arc<Feed>,
    client: reqwest::Client,
}

impl FeedPlugin {
    pub fn new(
        name: &'static str,
        section: &'static str,
        order: i32,
        item: &'static str,
        base_url: &str,
    ) -> Self {
        let feed = Arc::new(Feed::new(base_url, name, &format!("/api/plugins/{name}/proxy")));
        Self { name, section, order, item, feed, client: reqwest::Client::new() }
    }

    pub fn upstream(&self) -> &str {
        &self.feed.base
    }
}

#[async_trait::async_trait]
impl ConsolePlugin for FeedPlugin {
    fn name(&self) -> &'static str {
        self.name
    }

    fn nav(&self) -> Vec<NavSection> {
        vec![NavSection::new(self.section, self.order)
            .item(self.item, format!("#/grid?id=plugin:{}", self.name))]
    }

    fn routes(&self) -> axum::Router {
        axum::Router::new().nest("/proxy", crate::proxy::router(self.client.clone(), self.feed.base.clone()))
    }

    async fn components(&self) -> Vec<ComponentSummary> {
        self.feed.components().await
    }

    async fn health(&self) -> Health {
        self.feed.state().await.health
    }

    async fn detail(&self) -> String {
        let s = self.feed.state().await;
        format!("{} · {}", self.feed.base, s.detail)
    }

    async fn run(&self, shutdown: CancellationToken) {
        self.feed.run(self.client.clone(), Duration::from_secs(3), shutdown).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stormview::{Action, Relation};

    #[test]
    fn remap_prefixes_ids_and_targets_and_proxies_actions() {
        let list = vec![ComponentSummary {
            id: "drive:abc".into(),
            kind: "drive".into(),
            label: "sda".into(),
            health: Health::Ok,
            detail: String::new(),
            metrics: vec![],
            actions: vec![Action {
                id: "locate".into(),
                label: "Locate".into(),
                method: "POST".into(),
                path: "/api/v1/drives/abc/locate/on".into(),
                enabled: true,
                danger: false,
            }],
            relations: vec![Relation::belongs_to("shelf", "shelf:1")],
            link: Some("#/drives".into()),
        }];
        let out = remap(list, "drive", "/api/plugins/drive/proxy");
        assert_eq!(out[0].id, "drive:drive:abc");
        assert_eq!(out[0].relations[0].targets, vec!["drive:shelf:1"]);
        assert_eq!(out[0].actions[0].path, "/api/plugins/drive/proxy/api/v1/drives/abc/locate/on");
        assert!(out[0].link.is_none());
    }

    #[test]
    fn summary_prefers_the_system_card() {
        let mk = |id: &str, h: Health| ComponentSummary {
            id: id.into(),
            kind: "x".into(),
            label: id.into(),
            health: h,
            detail: format!("{id} detail"),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        };
        let (h, d) = summarize(&[mk("system", Health::Warn), mk("a", Health::Error)]);
        assert_eq!(h, Health::Warn);
        assert_eq!(d, "system detail");
        let (h, _) = summarize(&[mk("a", Health::Ok), mk("b", Health::Error)]);
        assert_eq!(h, Health::Error);
    }
}
