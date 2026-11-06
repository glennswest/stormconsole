//! A relay for `deploy/verify-flows.sh` (#4), speaking the real protocol from the vendored protos and honouring the
//! whitelist: a real relay needs a Cilium agent, which needs BPF and root. `standin_relay <port>`.
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hubble::flow::{self, Flow, Verdict};
use hubble::observer::get_flows_response::ResponseTypes;
use hubble::observer::observer_server::{Observer, ObserverServer};
use hubble::observer::*;
use tokio_stream::{Stream, StreamExt};
use tonic::{transport::Server, Request, Response, Status};

fn end(ns: &str, pod: &str, ip: u8) -> (flow::Endpoint, String) {
    (flow::Endpoint { namespace: ns.into(), pod_name: pod.into(), identity: 1000 + ip as u32, labels: vec![format!("k8s:app={pod}")], ..Default::default() }, format!("10.0.0.{ip}"))
}

fn one(src: (flow::Endpoint, String), dst: (flow::Endpoint, String), v: Verdict, port: u32) -> Flow {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    Flow {
        time: Some(prost_types::Timestamp { seconds: now.as_secs() as i64, nanos: 0 }),
        verdict: v as i32,
        drop_reason_desc: if v == Verdict::Dropped { flow::DropReason::PolicyDenied as i32 } else { 0 },
        ip: Some(flow::Ip { source: src.1, destination: dst.1, ..Default::default() }),
        l4: Some(flow::Layer4 { protocol: Some(flow::layer4::Protocol::Tcp(flow::Tcp { source_port: 40000, destination_port: port, flags: None })) }),
        source: Some(src.0),
        destination: Some(dst.0),
        node_name: "n1".into(),
        traffic_direction: flow::TrafficDirection::Ingress as i32,
        ..Default::default()
    }
}

fn script() -> Vec<Flow> {
    let web = || end("shop", "web", 1);
    let db = || end("shop", "db", 2);
    let vault = || end("secret", "vault", 3);
    let world = || (flow::Endpoint { labels: vec!["reserved:world".into()], identity: 2, ..Default::default() }, "203.0.113.9".to_string());
    vec![
        one(web(), db(), Verdict::Forwarded, 5432),
        one(web(), vault(), Verdict::Dropped, 8200),
        one(world(), web(), Verdict::Forwarded, 443),
        one(vault(), end("secret", "keys", 4), Verdict::Forwarded, 9000),
        one(end("shop", "web-2", 5), db(), Verdict::Forwarded, 5432),
    ]
}

fn wanted(f: &Flow, list: &[flow::FlowFilter]) -> bool {
    let name = |e: &Option<flow::Endpoint>| e.as_ref().map(|e| format!("{}/{}", e.namespace, e.pod_name)).unwrap_or_default();
    list.is_empty()
        || list.iter().any(|w| {
            (w.source_pod.is_empty() || w.source_pod.iter().any(|p| name(&f.source).starts_with(p)))
                && (w.destination_pod.is_empty() || w.destination_pod.iter().any(|p| name(&f.destination).starts_with(p)))
                && (w.verdict.is_empty() || w.verdict.contains(&f.verdict))
        })
}

// The stream's item type is tonic's, large Err and all.
#[allow(clippy::result_large_err)]
fn wrap(f: Flow) -> Result<GetFlowsResponse, Status> {
    Ok(GetFlowsResponse { node_name: f.node_name.clone(), time: f.time, response_types: Some(ResponseTypes::Flow(f)) })
}

struct Relay;

type FlowStream = Pin<Box<dyn Stream<Item = Result<GetFlowsResponse, Status>> + Send>>;

#[tonic::async_trait]
impl Observer for Relay {
    type GetFlowsStream = FlowStream;
    async fn get_flows(&self, r: Request<GetFlowsRequest>) -> Result<Response<FlowStream>, Status> {
        let r = r.into_inner();
        let list = r.whitelist;
        let mut flows: Vec<Flow> = script().into_iter().filter(|f| wanted(f, &list)).collect();
        if r.number > 0 && (r.number as usize) < flows.len() {
            flows = flows.split_off(flows.len() - r.number as usize);
        }
        let first = tokio_stream::iter(flows.into_iter().map(wrap));
        if !r.follow {
            return Ok(Response::new(Box::pin(first)));
        }
        let live = tokio_stream::StreamExt::throttle(tokio_stream::iter(std::iter::repeat_with(script).flatten()), Duration::from_millis(400))
            .filter(move |f| wanted(f, &list))
            .map(wrap);
        Ok(Response::new(Box::pin(first.chain(live))))
    }
    type GetAgentEventsStream = Pin<Box<dyn Stream<Item = Result<GetAgentEventsResponse, Status>> + Send>>;
    async fn get_agent_events(&self, _: Request<GetAgentEventsRequest>) -> Result<Response<Self::GetAgentEventsStream>, Status> {
        Err(Status::unimplemented("not here"))
    }
    type GetDebugEventsStream = Pin<Box<dyn Stream<Item = Result<GetDebugEventsResponse, Status>> + Send>>;
    async fn get_debug_events(&self, _: Request<GetDebugEventsRequest>) -> Result<Response<Self::GetDebugEventsStream>, Status> {
        Err(Status::unimplemented("not here"))
    }
    async fn get_nodes(&self, _: Request<GetNodesRequest>) -> Result<Response<GetNodesResponse>, Status> {
        Ok(Response::new(GetNodesResponse::default()))
    }
    async fn get_namespaces(&self, _: Request<GetNamespacesRequest>) -> Result<Response<GetNamespacesResponse>, Status> {
        Ok(Response::new(GetNamespacesResponse::default()))
    }
    async fn server_status(&self, _: Request<ServerStatusRequest>) -> Result<Response<ServerStatusResponse>, Status> {
        let peers = std::env::var("RELAY_PEERS").ok().and_then(|p| p.parse().ok()).unwrap_or(1);
        Ok(Response::new(ServerStatusResponse { seen_flows: 5, num_connected_nodes: Some(peers), num_unavailable_nodes: Some(0), ..Default::default() }))
    }
}

#[tokio::main]
async fn main() {
    let port = std::env::args().nth(1).expect("a port");
    Server::builder().add_service(ObserverServer::new(Relay)).serve(format!("127.0.0.1:{port}").parse().unwrap()).await.unwrap();
}
