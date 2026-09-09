//! The stormdrive plugin: this node's physical drives, straight from
//! stormdrive's own stormview feed on :9092 (shelves, drives, SMART, wear,
//! locate/fleet/test/designation actions — all carried by the feed, none
//! mapped here). Fleet-wide aggregation across nodes rides on the fleet
//! plugin's discovery later; one node first.
//!
//! Drives are **hardware**, not storage (issue #8). A drive is a physical
//! object with a serial, a shelf and a bay that somebody walks up to and
//! pulls; a stormblock volume is an allocation on top of one. Listing them
//! as peers in one column made both unreadable, so they live in different
//! sections of the navigator and the drives get a view that groups them
//! the way the hardware is actually arranged.

use console_core::FeedPlugin;

pub fn plugin(url: &str) -> FeedPlugin {
    FeedPlugin::new("drive", "stormdrive", "Hardware", 45, "Drives", url)
        .nav_items(&[("Drives", "#/drives"), ("Shelves", "#/drives?group=shelf")])
}
