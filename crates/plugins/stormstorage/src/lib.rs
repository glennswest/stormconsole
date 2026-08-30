//! The stormstorage plugin: pools, storage nodes and volumes across the
//! fleet, from stormstorage's own stormview feed on :9093. One endpoint
//! gives the cross-node view; nothing is mapped here.

use console_core::FeedPlugin;

pub fn plugin(url: &str) -> FeedPlugin {
    FeedPlugin::new("storage", "Storage", 40, "Pools", url)
}
