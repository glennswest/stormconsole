//! A client for Hubble relay (#4): gRPC, plaintext on the node's :4245 (stormcos `75-hubble-relay.yaml`), because
//! Hubble serves flows no other way. The server half of the generated code is for the rig's stand-in.

#[allow(clippy::all, deprecated, non_snake_case, missing_docs)]
pub mod flow {
    tonic::include_proto!("flow");
}
#[allow(clippy::all, deprecated, non_snake_case, missing_docs)]
pub mod relay {
    tonic::include_proto!("relay");
}
#[allow(clippy::all, deprecated, non_snake_case, missing_docs)]
pub mod observer {
    tonic::include_proto!("observer");
}

use std::time::Duration;

use futures_util::{Stream, StreamExt};
use observer::get_flows_response::ResponseTypes;
use observer::observer_client::ObserverClient;
use tonic::transport::{Channel, Endpoint};

/// Which flows to ask for. Pods are `namespace/` or `namespace/pod`, Hubble's own prefixes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// None means every flow; each entry matches either end of a flow.
    pub scope: Option<Vec<String>>,
    /// Hubble's verdict names: `FORWARDED`, `DROPPED`, …
    pub verdict: Option<String>,
    pub last: u64,
    pub follow: bool,
}

impl Query {
    /// OR across filters and AND within one, so a scope is one filter per direction per entry.
    pub fn whitelist(&self) -> Vec<flow::FlowFilter> {
        let verdict: Vec<i32> = self.verdict.as_deref().and_then(flow::Verdict::from_str_name).map(|v| vec![v as i32]).unwrap_or_default();
        match &self.scope {
            None if verdict.is_empty() => vec![],
            None => vec![flow::FlowFilter { verdict, ..Default::default() }],
            Some(scope) => scope
                .iter()
                .flat_map(|p| {
                    [
                        flow::FlowFilter { source_pod: vec![p.clone()], verdict: verdict.clone(), ..Default::default() },
                        flow::FlowFilter { destination_pod: vec![p.clone()], verdict: verdict.clone(), ..Default::default() },
                    ]
                })
                .collect(),
        }
    }

    pub fn request(&self) -> observer::GetFlowsRequest {
        observer::GetFlowsRequest { number: self.last, follow: self.follow, whitelist: self.whitelist(), ..Default::default() }
    }
}

/// What the relay says of itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub connected: Option<u32>,
    pub unavailable: Option<u32>,
    pub unavailable_nodes: Vec<String>,
    pub seen_flows: u64,
}

async fn connect(url: &str) -> Result<ObserverClient<Channel>, String> {
    let endpoint = Endpoint::from_shared(url.to_string()).map_err(|e| e.to_string())?.connect_timeout(Duration::from_secs(3));
    let channel = endpoint.connect().await.map_err(|e| format!("{e}: {}", cause(&e)))?;
    // A flow with a large L7 payload is larger than the 4 MiB default says is usual.
    Ok(ObserverClient::new(channel).max_decoding_message_size(16 << 20))
}

fn cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = Vec::new();
    let mut at = e.source();
    while let Some(s) = at {
        out.push(s.to_string());
        at = s.source();
    }
    out.join(": ")
}

pub async fn status(url: &str) -> Result<Status, String> {
    let mut c = connect(url).await?;
    let s = c.server_status(observer::ServerStatusRequest {}).await.map_err(|e| e.message().to_string())?.into_inner();
    Ok(Status {
        connected: s.num_connected_nodes,
        unavailable: s.num_unavailable_nodes,
        unavailable_nodes: s.unavailable_nodes,
        seen_flows: s.seen_flows,
    })
}

/// The flows, as they come. The relay's other messages on this stream (node status, lost events) are left out.
pub async fn flows(url: &str, q: &Query) -> Result<impl Stream<Item = Result<flow::Flow, String>>, String> {
    let mut c = connect(url).await?;
    let stream = c.get_flows(q.request()).await.map_err(|e| e.message().to_string())?.into_inner();
    Ok(stream.filter_map(|m| async move {
        match m {
            Ok(r) => match r.response_types {
                Some(ResponseTypes::Flows(f)) => Some(Ok(f)),
                _ => None,
            },
            Err(e) => Some(Err(e.message().to_string())),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_is_either_end_of_a_flow() {
        let q = Query { scope: Some(vec!["shop/web".into()]), verdict: Some("DROPPED".into()), last: 5, ..Default::default() };
        let w = q.whitelist();
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].source_pod, ["shop/web"]);
        assert!(w[0].destination_pod.is_empty());
        assert_eq!(w[1].destination_pod, ["shop/web"]);
        assert_eq!(w[0].verdict, [flow::Verdict::Dropped as i32]);
        assert_eq!(q.request().number, 5);
    }

    #[test]
    fn no_scope_is_every_flow() {
        assert!(Query::default().whitelist().is_empty());
        assert_eq!(Query { verdict: Some("FORWARDED".into()), ..Default::default() }.whitelist().len(), 1);
        assert!(Query { verdict: Some("nonsense".into()), ..Default::default() }.whitelist().is_empty());
    }

    #[test]
    fn an_empty_scope_asks_for_nothing_it_was_not_given() {
        assert!(Query { scope: Some(vec![]), ..Default::default() }.whitelist().is_empty(), "the caller has to not ask at all");
    }
}
