//! The stormconsole plugin contract and host services.
//!
//! The console core knows nothing about kubernetes, drives, or images —
//! every domain is a [`ConsolePlugin`] that contributes navigation, API
//! routes, create forms ([`Creator`]), a slice of the aggregated stormview
//! component feed, what a viewer may see of it ([`Access`]) and the events
//! for its objects. The [`Registry`] is the host: it merges navigation,
//! aggregates, filters per viewer and pushes the feed, and drives each
//! plugin's background work. [`Feed`]/[`FeedPlugin`] consume an upstream's
//! own stormview feed; `proxy` forwards to an upstream, with the console's
//! bearer where it has one.

pub use stormview::{Action, ComponentSummary, Health, Metric, Relation, RelationKind};

pub mod events;
pub mod access;
pub mod create;
pub mod feed;
mod nav;
mod plugin;
mod probe;
pub mod proxy;
mod registry;
pub mod upstream;
pub mod value;

pub use access::{Access, Scope, Viewer};
pub use events::{Event, Events};
pub use create::{Creator, Field, FieldOption};
pub use feed::{Feed, FeedPlugin, FeedState};
pub use nav::{NavItem, NavKind, NavSection};
pub use plugin::ConsolePlugin;
pub use probe::{Probe, ProbeState};
pub use registry::Registry;
