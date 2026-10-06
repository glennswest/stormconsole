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
        self.check_as(client, None).await
    }

    /// The same, carrying a bearer — for an upstream that refuses anonymous
    /// reads, such as an apiserver with `--anonymous-auth false`, where a
    /// bare probe reads 401 on a connection that is working (#33).
    pub async fn check_as(&self, client: &reqwest::Client, bearer: Option<&str>) {
        let mut req = client.get(&self.url).timeout(Duration::from_secs(5));
        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }
        let observed = match req.send().await {
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

/// reqwest's top level repeats the URL, so it is skipped; the rest of the
/// chain is said whole. One level read "client error (Connect)" for every
/// TLS failure — the cause (`invalid peer certificate: UnknownIssuer`) is
/// below it (#33, as #47 found for fastetcd).
pub(crate) fn concise(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut parts: Vec<String> = Vec::new();
    let mut next = e.source();
    while let Some(s) = next {
        let m = s.to_string();
        if !parts.iter().any(|p| p.contains(&m)) {
            parts.push(m);
        }
        next = s.source();
    }
    if parts.is_empty() { e.to_string() } else { parts.join(": ") }
}
