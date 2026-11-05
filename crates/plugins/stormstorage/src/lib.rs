//! The stormstorage plugin: pools, storage nodes and volumes across the
//! fleet, from stormstorage's own stormview feed on :9093. One endpoint
//! gives the cross-node view; nothing is mapped here.
//!
//! With `[api] api_token` set, stormstorage takes no write without it
//! (stormstorage#6): publish, assemble, move and delete on a volume. Reads
//! and the feed stay open. So the token — `[stormstorage] token_file` —
//! goes on what the proxy forwards, added server-side (#53).
//!
//! Its `node` rows say what the engine on a machine holds. They are not
//! machines to the console (#161): the fleet plugin puts the numbers on the
//! node's own row, and here they are `storage node`.

use console_core::FeedPlugin;

/// `token` is stormstorage's `[api] api_token`, when it has one.
pub fn plugin(url: &str, token: Option<String>) -> FeedPlugin {
    FeedPlugin::new("storage", "stormstorage", "Storage", 40, "Pools", url).admin().bearer(token).rekind("node", "storage node")
}
