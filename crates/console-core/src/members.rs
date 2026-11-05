//! Which machines are this cluster's (#161).
//!
//! The apiserver's Node objects are the answer, and the only one. Hearing a
//! machine on the log group says it is on the segment, not that it is ours:
//! every single-node cluster on a network announces itself there.

use async_trait::async_trait;

/// One node of this cluster.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    /// The addresses its Node object reports.
    pub addrs: Vec<String>,
}

/// Asked often, so an answer comes from a cache, not from a request.
#[async_trait]
pub trait Membership: Send + Sync {
    /// The members known now; empty until the apiserver has answered.
    async fn members(&self) -> Vec<Member>;
}
