//! The plugin contract. Compiled-in plugins implement this today; the trait
//! is the seam where dynamically registered remote plugins attach later.

use async_trait::async_trait;
use stormview::{ComponentSummary, Health};
use tokio_util::sync::CancellationToken;

use crate::access::{Access, Viewer};
use crate::create::Creator;
use crate::nav::NavSection;

#[async_trait]
pub trait ConsolePlugin: Send + Sync {
    /// Stable short name. Prefixes this plugin's component ids
    /// ("k8s:pod:default/web") and its API mount ("/api/plugins/k8s/…").
    fn name(&self) -> &'static str;

    /// Navigation contribution; sections with the same label merge across
    /// plugins.
    fn nav(&self) -> Vec<NavSection> {
        Vec::new()
    }

    /// What this plugin lets a user create, and how (see [`Creator`]).
    fn creators(&self) -> Vec<Creator> {
        Vec::new()
    }

    /// API routes, mounted at `/api/plugins/{name}`. Routers carry their
    /// own state (typically an `Arc<Self>`), so the host stays uncoupled.
    fn routes(&self) -> axum::Router {
        axum::Router::new()
    }

    /// This plugin's slice of the aggregated component feed. Ids must be
    /// prefixed with `{name}:`.
    async fn components(&self) -> Vec<ComponentSummary>;

    /// The plugin's own health — surfaced as a component by the host and
    /// aggregated into /readyz.
    async fn health(&self) -> Health {
        Health::Ok
    }

    /// One human line for the plugin's card.
    async fn detail(&self) -> String {
        String::new()
    }

    /// What this viewer may see of this plugin's slice. The default is
    /// [`Access::Unrestricted`] — a plugin with nothing to authorize says
    /// so rather than implying an enforcement it is not doing. The host
    /// applies the answer before a snapshot leaves the process, so this is
    /// an authorization result and not a display choice.
    async fn access(&self, _viewer: &Viewer) -> Access {
        Access::Unrestricted
    }

    /// Background work: watches, pollers, multicast listeners. Runs for the
    /// life of the process; must return promptly once `shutdown` fires.
    async fn run(&self, shutdown: CancellationToken) {
        shutdown.cancelled().await;
    }
}
