//! Upstream reachability. A plugin that fronts a daemon (rustkube,
//! stormblock, sbregistry, a node's stormdrive) holds a [`Probe`] per
//! endpoint and drives it from its `run` loop; components derive health
//! from the last observation instead of blocking a feed refresh on a
//! network call.

use std::time::Duration;

use stormview::Health;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct ProbeState {
    pub health: Health,
    pub detail: String,
}

impl Default for ProbeState {
    fn default() -> Self {
        Self { health: Health::Unknown, detail: "not yet checked".to_string() }
    }
}

pub struct Probe {
    /// Full URL of the upstream's health/readiness endpoint.
    pub url: String,
    state: RwLock<ProbeState>,
}

impl Probe {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), state: RwLock::new(ProbeState::default()) }
    }

    pub async fn state(&self) -> ProbeState {
        self.state.read().await.clone()
    }

    /// One observation: GET the URL, record health from the HTTP outcome.
    pub async fn check(&self, client: &reqwest::Client) {
        let observed = match client.get(&self.url).timeout(Duration::from_secs(5)).send().await {
            Ok(resp) if resp.status().is_success() => {
                ProbeState { health: Health::Ok, detail: format!("reachable · {}", resp.status()) }
            }
            Ok(resp) => {
                ProbeState { health: Health::Warn, detail: format!("responded {}", resp.status()) }
            }
            Err(e) => ProbeState {
                health: Health::Error,
                detail: format!("unreachable: {}", concise(&e)),
            },
        };
        *self.state.write().await = observed;
    }

    /// Probe every `interval` until shutdown. The first check runs
    /// immediately so the feed is honest from the first paint.
    pub async fn run(
        &self,
        client: reqwest::Client,
        interval: Duration,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        loop {
            self.check(&client).await;
            tokio::select! {
                _ = tokio::time::sleep(interval) => {}
                _ = shutdown.cancelled() => return,
            }
        }
    }
}

/// reqwest error chains repeat the URL and the source; one level is enough
/// for a card detail line.
fn concise(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    e.source().map(|s| s.to_string()).unwrap_or_else(|| e.to_string())
}
