//! Who talked to whom, and what the network did about it (#4): Hubble relay's flows, the answer to "why can nothing
//! reach this". The relay gives every flow in the cluster to whoever asks, so the limit is here: a flow reaches a
//! viewer only if one end is in a project they may see, and the other end, when it is a project they may not, is
//! said to be "another project" with nothing about it. Asked narrowly of the relay too, so it does less work,
//! and checked again on what comes back, since that is the part that matters.

use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use futures_util::{Stream, StreamExt};
use hubble::flow::{self, Flow};
use serde_json::{json, Value};

use crate::Inner;

const MOST: u64 = 1000;
/// A followed stream ends here and the browser asks again, so a viewer's rights are looked at again.
const FOLLOW_FOR: Duration = Duration::from_secs(300);

#[derive(serde::Deserialize, Default)]
pub struct Ask {
    pub ns: Option<String>,
    pub pod: Option<String>,
    pub verdict: Option<String>,
    pub last: Option<u64>,
}

/// One end of a flow. A project this viewer may not see leaves nothing but that it was one. An end in no project
/// (the host, the outside world) has no one to withhold it from. `seen` is the projects they may see, or None for
/// every one: by what is allowed rather than what is known to be hidden, so a project the console has not heard of
/// yet is withheld too.
fn side(e: Option<&flow::Endpoint>, ip: &str, names: &[String], seen: Option<&HashSet<String>>) -> Value {
    let ns = e.map_or("", |e| e.namespace.as_str());
    if !ns.is_empty() && seen.is_some_and(|s| !s.contains(ns)) {
        return json!({"hidden": true});
    }
    json!({
        "project": ns,
        "pod": e.map_or("", |e| e.pod_name.as_str()),
        "ip": ip,
        "identity": e.map_or(0, |e| e.identity),
        "labels": e.map(|e| e.labels.clone()).unwrap_or_default(),
        "names": names,
    })
}

fn port(f: &Flow) -> (String, Option<u32>) {
    use flow::layer4::Protocol::*;
    match f.l4.as_ref().and_then(|l| l.protocol.as_ref()) {
        Some(Tcp(t)) => ("TCP".into(), Some(t.destination_port)),
        Some(Udp(u)) => ("UDP".into(), Some(u.destination_port)),
        Some(Sctp(s)) => ("SCTP".into(), Some(s.destination_port)),
        Some(IcmPv4(_)) => ("ICMPv4".into(), None),
        Some(IcmPv6(_)) => ("ICMPv6".into(), None),
        _ => (String::new(), None),
    }
}

/// A flow as this viewer may have it, or None when neither end is theirs to see.
pub fn row(f: &Flow, seen: Option<&HashSet<String>>) -> Option<Value> {
    if let Some(s) = seen {
        let mine = |e: &Option<flow::Endpoint>| e.as_ref().is_some_and(|e| s.contains(&e.namespace));
        if !mine(&f.source) && !mine(&f.destination) {
            return None;
        }
    }
    let (src_ip, dst_ip) = f.ip.as_ref().map_or(("", ""), |i| (i.source.as_str(), i.destination.as_str()));
    let (protocol, dest_port) = port(f);
    let verdict = flow::Verdict::try_from(f.verdict).map_or("VERDICT_UNKNOWN", |v| v.as_str_name());
    // 0 is proto3's "not set": the flow was not dropped.
    let drop = (f.drop_reason_desc != 0).then(|| flow::DropReason::try_from(f.drop_reason_desc).ok().map(|d| d.as_str_name())).flatten();
    Some(json!({
        "time": f.time.map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000),
        "verdict": verdict,
        "drop": drop,
        "source": side(f.source.as_ref(), src_ip, &f.source_names, seen),
        "destination": side(f.destination.as_ref(), dst_ip, &f.destination_names, seen),
        "protocol": protocol,
        "port": dest_port,
        "direction": flow::TrafficDirection::try_from(f.traffic_direction).ok().map(|d| d.as_str_name()),
        "reply": f.is_reply,
        "node": f.node_name,
    }))
}

/// A pod prefix-matches at the relay (`web` matches `web-2`), so the exact name is held to here.
fn this_pod(f: &Flow, ns: &str, pod: &str) -> bool {
    let is = |e: &Option<flow::Endpoint>| e.as_ref().is_some_and(|e| e.namespace == ns && e.pod_name == pod);
    is(&f.source) || is(&f.destination)
}

enum Scope {
    Query(hubble::Query, Option<HashSet<String>>),
    /// A viewer who may see no project: nothing to ask.
    Nothing,
    Refused(Response),
}

async fn scope(inner: &Inner, viewer: &Viewer, a: &Ask, follow: bool) -> Scope {
    let visible = inner.access.visible(viewer).await;
    let ns = a.ns.as_deref().filter(|n| !n.is_empty());
    let pod = a.pod.as_deref().filter(|p| !p.is_empty());
    if let Some(n) = ns {
        if let Some(r) = crate::refuse_hidden(inner, viewer, n).await {
            return Scope::Refused(r);
        }
    }
    let scope = match (ns, pod, &visible) {
        (Some(n), Some(p), _) => Some(vec![format!("{n}/{p}")]),
        (Some(n), None, _) => Some(vec![format!("{n}/")]),
        (None, _, Some(list)) if list.is_empty() => return Scope::Nothing,
        (None, _, Some(list)) => Some(list.iter().map(|n| format!("{n}/")).collect()),
        (None, _, None) => None,
    };
    let seen = visible.map(|l| l.into_iter().collect());
    let verdict = a.verdict.clone().filter(|v| flow::Verdict::from_str_name(v).is_some());
    Scope::Query(hubble::Query { scope, verdict, last: a.last.unwrap_or(100).clamp(1, MOST), follow }, seen)
}

/// This node's network has no relay to ask (#285).
fn not_here() -> Value {
    json!({"available": false, "reason": "This node's network serves no flows.", "items": []})
}

/// Why the relay gave nothing: its own words beside what they mean here.
fn no_relay(url: &str, why: &str) -> Value {
    json!({
        "available": false,
        "reason": format!("Hubble relay did not answer at {url}: {why}."),
        "items": [],
    })
}

/// `GET /flows?ns=&pod=&verdict=&last=`
pub async fn list(State(inner): State<Arc<Inner>>, viewer: Viewer, Query(a): Query<Ask>) -> Response {
    if !inner.network().is_cilium() {
        return Json(not_here()).into_response();
    }
    let (q, seen) = match scope(&inner, &viewer, &a, false).await {
        Scope::Query(q, h) => (q, h),
        Scope::Nothing => return Json(json!({"available": true, "items": [], "note": "You may see no project yet."})).into_response(),
        Scope::Refused(r) => return r,
    };
    let url = &inner.hubble_relay;
    let status = match hubble::status(url).await {
        Ok(s) => s,
        Err(e) => return Json(no_relay(url, &e)).into_response(),
    };
    let stream = match hubble::flows(url, &q).await {
        Ok(s) => s,
        Err(e) => return Json(no_relay(url, &e)).into_response(),
    };
    let exact = a.ns.as_deref().zip(a.pod.as_deref()).filter(|(_, p)| !p.is_empty()).map(|(n, p)| (n.to_string(), p.to_string()));
    let mut stream = Box::pin(stream);
    let mut items = Vec::new();
    let mut error = None;
    // The relay ends a query when it has them, unless a node it waits on does not answer.
    let wait = tokio::time::timeout(Duration::from_secs(8), async {
        while let Some(m) = stream.next().await {
            match m {
                Ok(f) => {
                    if exact.as_ref().is_some_and(|(n, p)| !this_pod(&f, n, p)) {
                        continue;
                    }
                    items.extend(row(&f, seen.as_ref()));
                }
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
    })
    .await;
    let note = (status.connected == Some(0)).then(|| {
        "The relay has no nodes connected, so it has no flows to give. It reaches each agent over mutual TLS on :4244, and waits on those certificates (stormcos#327).".to_string()
    });
    Json(json!({
        "available": true,
        "items": items,
        "connected": status.connected,
        "unavailable": status.unavailable,
        "unavailableNodes": status.unavailable_nodes,
        "partial": wait.is_err(),
        "error": error,
        "note": note,
    }))
    .into_response()
}

/// `GET /flows/stream?…`: the flows as they happen, one event each.
pub async fn stream(State(inner): State<Arc<Inner>>, viewer: Viewer, Query(a): Query<Ask>) -> Response {
    if !inner.network().is_cilium() {
        return Json(not_here()).into_response();
    }
    let (q, seen) = match scope(&inner, &viewer, &a, true).await {
        Scope::Query(q, h) => (q, h),
        Scope::Nothing => return Json(json!({"items": []})).into_response(),
        Scope::Refused(r) => return r,
    };
    let q = hubble::Query { last: 1, ..q };
    let flows = match hubble::flows(&inner.hubble_relay, &q).await {
        Ok(s) => s,
        Err(e) => return Json(no_relay(&inner.hubble_relay, &e)).into_response(),
    };
    let exact = a.ns.clone().zip(a.pod.clone()).filter(|(_, p)| !p.is_empty());
    let until = Instant::now() + FOLLOW_FOR;
    let events = flows
        .take_while(move |_| futures_util::future::ready(Instant::now() < until))
        .filter_map(move |m| {
            let got = m.ok().and_then(|f| {
                if exact.as_ref().is_some_and(|(n, p)| !this_pod(&f, n, p)) {
                    return None;
                }
                row(&f, seen.as_ref())
            });
            futures_util::future::ready(got)
        })
        .map(|v| Ok::<_, Infallible>(Event::default().data(v.to_string())));
    sse(events).into_response()
}

fn sse(s: impl Stream<Item = Result<Event, Infallible>> + Send + 'static) -> impl IntoResponse {
    Sse::new(s).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(ns: &str, pod: &str) -> flow::Endpoint {
        flow::Endpoint { namespace: ns.into(), pod_name: pod.into(), identity: 7, labels: vec!["k8s:app=x".into()], ..Default::default() }
    }

    fn a_flow(src: flow::Endpoint, dst: flow::Endpoint) -> Flow {
        Flow {
            verdict: flow::Verdict::Dropped as i32,
            drop_reason_desc: flow::DropReason::PolicyDenied as i32,
            ip: Some(flow::Ip { source: "10.0.0.1".into(), destination: "10.0.0.2".into(), ..Default::default() }),
            l4: Some(flow::Layer4 { protocol: Some(flow::layer4::Protocol::Tcp(flow::Tcp { source_port: 4, destination_port: 443, flags: None })) }),
            source: Some(src),
            destination: Some(dst),
            ..Default::default()
        }
    }

    #[test]
    fn a_flow_in_words() {
        let r = row(&a_flow(end("shop", "web"), end("shop", "db")), None).unwrap();
        assert_eq!((r["verdict"].as_str(), r["drop"].as_str(), r["protocol"].as_str(), r["port"].as_u64()), (Some("DROPPED"), Some("POLICY_DENIED"), Some("TCP"), Some(443)));
        assert_eq!((r["source"]["pod"].as_str(), r["destination"]["ip"].as_str()), (Some("web"), Some("10.0.0.2")));
    }

    #[test]
    fn a_viewer_sees_a_flow_only_with_one_end_theirs_and_nothing_of_the_other() {
        let seen: HashSet<String> = ["shop".to_string()].into();
        assert!(row(&a_flow(end("secret", "a"), end("secret", "b")), Some(&seen)).is_none());
        assert!(row(&a_flow(end("shop", "web"), end("not-heard-of-yet", "x")), Some(&seen)).unwrap()["destination"]["hidden"] == true);
        let r = row(&a_flow(end("shop", "web"), end("secret", "vault")), Some(&seen)).unwrap();
        assert_eq!(r["destination"], json!({"hidden": true}));
        assert_eq!(r["source"]["pod"], "web");
        let text = r.to_string();
        assert!(!text.contains("vault") && !text.contains("secret") && !text.contains("10.0.0.2"), "{text}");
    }

    #[test]
    fn with_no_project_on_either_end_a_limited_viewer_is_told_nothing() {
        let seen: HashSet<String> = ["shop".to_string()].into();
        let world = flow::Endpoint { labels: vec!["reserved:world".into()], ..Default::default() };
        assert!(row(&a_flow(world.clone(), world.clone()), Some(&seen)).is_none());
        assert_eq!(row(&a_flow(world.clone(), end("shop", "web")), Some(&seen)).unwrap()["source"]["ip"], "10.0.0.1", "the outside world is nobody's to withhold");
        assert!(row(&a_flow(world.clone(), world), None).is_some(), "the whole cluster's viewer has it");
    }

    #[test]
    fn the_relays_prefix_is_not_the_pod() {
        let f = a_flow(end("shop", "web-2"), end("shop", "db"));
        assert!(!this_pod(&f, "shop", "web"));
        assert!(this_pod(&f, "shop", "db"));
    }
}
