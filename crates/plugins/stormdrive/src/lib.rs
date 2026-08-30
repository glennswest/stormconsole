//! The stormdrive plugin: this node's physical drives, straight from
//! stormdrive's own stormview feed on :9092 (shelves, drives, SMART, wear,
//! locate/fleet/test/designation actions — all carried by the feed, none
//! mapped here). Fleet-wide aggregation across nodes rides on the fleet
//! plugin's discovery later; one node first.

use console_core::FeedPlugin;

pub fn plugin(url: &str) -> FeedPlugin {
    FeedPlugin::new("drive", "Storage", 40, "Drives", url)
}
