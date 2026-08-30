//! The stormconsole plugin contract and host services.
//!
//! The console core knows nothing about kubernetes, drives, or images —
//! every domain is a [`ConsolePlugin`] that contributes navigation, API
//! routes, and a slice of the aggregated stormview component feed. The
//! [`Registry`] is the host: it merges navigation, aggregates and pushes
//! the feed, and drives each plugin's background work.

pub use stormview::{Action, ComponentSummary, Health, Metric, Relation, RelationKind};

pub mod feed;
mod nav;
mod plugin;
mod probe;
pub mod proxy;
mod registry;
pub mod value;

pub use feed::{Feed, FeedPlugin, FeedState};
pub use nav::{NavItem, NavSection};
pub use plugin::ConsolePlugin;
pub use probe::{Probe, ProbeState};
pub use registry::Registry;
