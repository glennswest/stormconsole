//! Scale a workload; cordon, uncordon and drain a node (#36).
//!
//! All of it goes **as the person**, so the apiserver's RBAC decides. A node is the whole cluster's, so its
//! verbs also need `admin` on this console and a kubernetes identity, and never go on the console's credential
//! (as a node's rack does, #77). Scaling is an operator's, like deleting a pod: someone with no identity is
//! bound by the console's own role, which refuses it (docs/rbac.md).
//!
//! A drain is what `kubectl drain` does, and refuses what it refuses: cordon, then an Eviction (`policy/v1`) for
//! each pod, so a PodDisruptionBudget can say no. A DaemonSet's pod comes straight back and a mirror pod is the
//! node's own, so neither is evicted. A pod with no controller would not come back, so it is left unless asked for.
//! One at a time: rustkube's budget check and delete are not one step, and two at once could both pass it.
//!
//! This is the apiserver's cordon. A cluster member's drain, on the Cluster page, is stormcluster's (#88).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use serde_json::{json, Value};

use crate::Inner;

fn refuse(code: StatusCode, why: impl Into<String>) -> Response {
    (code, Json(json!({"error": why.into()}))).into_response()
}

/// The apiserver's refusal, in its words.
fn upstream(status: reqwest::StatusCode, body: &Value) -> Response {
    let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let said = body.get("message").and_then(Value::as_str).map_or_else(|| format!("the apiserver answered {status}"), str::to_string);
    refuse(code, said)
}

/// A workload the console can scale, as its row names it: the API path of its collection.
pub fn scalable(kind: &str) -> Option<&'static str> {
    match kind {
        "deploy" => Some("deployments"),
        "sts" => Some("statefulsets"),
        _ => None,
    }
}

/// A replica count, as a person typed or a page sent it.
pub fn replicas(body: &Value) -> Result<i64, String> {
    let n = match &body["replicas"] {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    match n {
        Some(n) if (0..=10_000).contains(&n) => Ok(n),
        _ => Err("replicas: a whole number from 0 to 10000".to_string()),
    }
}

/// `POST /scale/{kind}/{ns}/{name} {"replicas": 3}`, through the `/scale` subresource. With no body it says what
/// it needs, so a plain button anywhere can ask for the number.
pub async fn scale(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((kind, ns, name)): Path<(String, String, String)>,
    body: Bytes,
) -> Response {
    if let Some(refusal) = crate::refuse_hidden(&inner, &viewer, &ns).await {
        return refusal;
    }
    let Some(plural) = scalable(&kind) else {
        return refuse(StatusCode::NOT_FOUND, format!("a {kind} is not something the console scales"));
    };
    if body.is_empty() {
        let current = inner.store.object(&kind, &format!("{ns}/{name}")).await.and_then(|o| o.pointer("/spec/replicas").and_then(Value::as_i64));
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "how many replicas?",
                "ask": {"field": "replicas", "label": format!("Replicas for {name}"), "current": current},
            })),
        )
            .into_response();
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let n = match replicas(&parsed) {
        Ok(n) => n,
        Err(why) => return refuse(StatusCode::BAD_REQUEST, why),
    };
    let Some(client) = &inner.client else { return refuse(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    let path = format!("/apis/apps/v1/namespaces/{ns}/{plural}/{name}/scale");
    match client.patch_merge(&path, &json!({"spec": {"replicas": n}}), viewer.token.as_deref()).await {
        Ok((status, _)) if status.is_success() => Json(json!({"message": format!("{name}: {n} replica{}", if n == 1 { "" } else { "s" }), "replicas": n})).into_response(),
        Ok((status, said)) => upstream(status, &said),
        Err(e) => refuse(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// Why this viewer may not change a node, or `None` when it is the apiserver's to say.
pub fn node_refusal(admin: bool, identity: bool) -> Option<&'static str> {
    if !admin {
        return Some("Changing a node needs the admin role on this console.");
    }
    if !identity {
        return Some("This goes as you, and this sign-in has no kubernetes identity. Sign in as a user with one.");
    }
    None
}

async fn set_unschedulable(inner: &Inner, viewer: &Viewer, name: &str, on: bool) -> Result<(), Response> {
    let Some(client) = &inner.client else { return Err(refuse(StatusCode::SERVICE_UNAVAILABLE, "no apiserver")) };
    match client.patch_merge(&format!("/api/v1/nodes/{name}"), &json!({"spec": {"unschedulable": on}}), viewer.token.as_deref()).await {
        Ok((status, saved)) if status.is_success() => {
            // Seen at once by the page's re-read, not when the watch catches up.
            inner.store.observe("node", saved).await;
            Ok(())
        }
        Ok((status, said)) => Err(upstream(status, &said)),
        Err(e) => Err(refuse(StatusCode::BAD_GATEWAY, e.to_string())),
    }
}

async fn cordoning(inner: Arc<Inner>, viewer: Viewer, name: String, on: bool) -> Response {
    if let Some(why) = node_refusal(viewer.has_role("admin"), viewer.token.is_some()) {
        return refuse(StatusCode::FORBIDDEN, why);
    }
    match set_unschedulable(&inner, &viewer, &name, on).await {
        Ok(()) => Json(json!({"message": format!("{name}: {}", if on { "cordoned" } else { "uncordoned" })})).into_response(),
        Err(r) => r,
    }
}

pub async fn cordon(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    cordoning(inner, viewer, name, true).await
}

pub async fn uncordon(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    cordoning(inner, viewer, name, false).await
}

/// What a drain would do with one pod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    Evict,
    /// No controller would bring it back.
    Unmanaged,
    /// Stays, for the reason given.
    Stays(&'static str),
}

pub fn fate(pod: &Value) -> Fate {
    if pod.pointer("/metadata/annotations/kubernetes.io~1config.mirror").is_some() {
        return Fate::Stays("the node's own (a mirror pod)");
    }
    if pod.pointer("/metadata/deletionTimestamp").is_some() {
        return Fate::Stays("already going");
    }
    if matches!(pod.pointer("/status/phase").and_then(Value::as_str), Some("Succeeded" | "Failed")) {
        return Fate::Stays("finished");
    }
    let owner = pod
        .pointer("/metadata/ownerReferences")
        .and_then(Value::as_array)
        .and_then(|o| o.iter().find(|r| r["controller"] == true))
        .and_then(|r| r["kind"].as_str());
    match owner {
        Some("DaemonSet") => Fate::Stays("a DaemonSet's, which would put it straight back"),
        Some("Node") => Fate::Stays("the node's own (a mirror pod)"),
        Some(_) => Fate::Evict,
        None => Fate::Unmanaged,
    }
}

/// Data that goes with the pod: an `emptyDir` is gone when it is evicted.
fn loses_data(pod: &Value) -> bool {
    pod.pointer("/spec/volumes").and_then(Value::as_array).is_some_and(|v| v.iter().any(|v| v.get("emptyDir").is_some()))
}

fn row(pod: &Value) -> Value {
    json!({
        "namespace": pod.pointer("/metadata/namespace"),
        "name": pod.pointer("/metadata/name"),
        "phase": pod.pointer("/status/phase"),
        "losesData": loses_data(pod),
    })
}

/// The pods on a node, sorted into what a drain does with them.
pub fn plan(pods: &[Value]) -> Value {
    let (mut evict, mut unmanaged, mut stays) = (Vec::new(), Vec::new(), Vec::new());
    for p in pods {
        match fate(p) {
            Fate::Evict => evict.push(row(p)),
            Fate::Unmanaged => unmanaged.push(row(p)),
            Fate::Stays(why) => {
                let mut r = row(p);
                r["why"] = why.into();
                stays.push(r);
            }
        }
    }
    json!({"evict": evict, "unmanaged": unmanaged, "stays": stays})
}

/// What is said before a drain that would leave nothing to go to.
pub fn only_node_warning(nodes: &[&Value], name: &str) -> Option<String> {
    let elsewhere = nodes.iter().any(|n| {
        n.pointer("/metadata/name").and_then(Value::as_str) != Some(name) && n.pointer("/spec/unschedulable").and_then(Value::as_bool) != Some(true)
    });
    (!elsewhere).then(|| format!("No other node can take work: what is evicted from {name} waits until this one is uncordoned."))
}

async fn pods_on(inner: &Inner, viewer: &Viewer, name: &str) -> Result<Vec<Value>, Response> {
    let Some(client) = &inner.client else { return Err(refuse(StatusCode::SERVICE_UNAVAILABLE, "no apiserver")) };
    let r = client
        .get_raw_as(&format!("/api/v1/pods?fieldSelector=spec.nodeName%3D{name}"), viewer.token.as_deref())
        .await
        .map_err(|e| refuse(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(upstream(status, &body));
    }
    Ok(body["items"].as_array().cloned().unwrap_or_default())
}

/// `GET /nodes/{name}/drain`: what a drain would do, and whether this viewer may. From the watched cache, because
/// someone who may look at a node may not be allowed to list every project's pods. A project this viewer may not
/// see is counted and not named.
pub async fn drain_plan(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>) -> Response {
    let Some(node) = inner.store.object("node", &name).await else {
        return refuse(StatusCode::NOT_FOUND, format!("no Node {name}"));
    };
    let mut pods: Vec<Value> = inner
        .store
        .kind("pod")
        .await
        .into_values()
        .filter(|p| p.pointer("/spec/nodeName").and_then(Value::as_str) == Some(name.as_str()))
        .collect();
    pods.sort_by_key(|p| (p.pointer("/metadata/namespace").map(Value::to_string), p.pointer("/metadata/name").map(Value::to_string)));
    let before = pods.len();
    if let Some((hidden, _)) = inner.access.hidden(&viewer).await {
        pods.retain(|p| !p.pointer("/metadata/namespace").and_then(Value::as_str).is_some_and(|ns| hidden.contains(ns)));
    }
    let nodes = inner.store.kind("node").await;
    let why = node_refusal(viewer.has_role("admin"), viewer.token.is_some());
    let mut out = plan(&pods);
    out["node"] = name.clone().into();
    out["cordoned"] = (node.pointer("/spec/unschedulable").and_then(Value::as_bool) == Some(true)).into();
    out["hidden"] = (before - pods.len()).into();
    out["may"] = why.is_none().into();
    out["why"] = why.into();
    out["warning"] = only_node_warning(&nodes.values().collect::<Vec<_>>(), &name).into();
    Json(out).into_response()
}

/// `POST /nodes/{name}/drain {"unmanaged": false}`: cordon, then evict. What could not be evicted is listed, not
/// an error: draining again is how it is retried.
pub async fn drain(State(inner): State<Arc<Inner>>, viewer: Viewer, Path(name): Path<String>, body: Bytes) -> Response {
    if let Some(why) = node_refusal(viewer.has_role("admin"), viewer.token.is_some()) {
        return refuse(StatusCode::FORBIDDEN, why);
    }
    let Some(client) = &inner.client else { return refuse(StatusCode::SERVICE_UNAVAILABLE, "no apiserver") };
    let with_unmanaged = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["unmanaged"].as_bool()).unwrap_or(false);
    // Listed first: someone who may not see the node's pods changes nothing.
    let pods = match pods_on(&inner, &viewer, &name).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(r) = set_unschedulable(&inner, &viewer, &name, true).await {
        return r;
    }
    let (mut evicted, mut blocked, mut left) = (Vec::new(), Vec::new(), Vec::new());
    for p in &pods {
        let f = fate(p);
        if f == Fate::Unmanaged && !with_unmanaged {
            left.push(row(p));
            continue;
        }
        if matches!(f, Fate::Stays(_)) {
            continue;
        }
        let ns = p.pointer("/metadata/namespace").and_then(Value::as_str).unwrap_or("default");
        let pod = p.pointer("/metadata/name").and_then(Value::as_str).unwrap_or("");
        let eviction = json!({"apiVersion": "policy/v1", "kind": "Eviction", "metadata": {"name": pod, "namespace": ns}});
        let at = format!("/api/v1/namespaces/{ns}/pods/{pod}/eviction");
        match client.post_json_as(&at, &eviction, viewer.token.as_deref()).await {
            // A 404 is a pod that went by itself, which is what was wanted.
            Ok((status, _)) if status.is_success() || status == reqwest::StatusCode::NOT_FOUND => evicted.push(row(p)),
            Ok((status, said)) => {
                let mut r = row(p);
                r["why"] = said.get("message").and_then(Value::as_str).map_or_else(|| format!("the apiserver answered {status}"), str::to_string).into();
                blocked.push(r);
            }
            Err(e) => {
                let mut r = row(p);
                r["why"] = e.to_string().into();
                blocked.push(r);
            }
        }
    }
    let mut message = format!("{name}: cordoned, {} evicted", evicted.len());
    if !blocked.is_empty() {
        message.push_str(&format!(", {} could not be", blocked.len()));
    }
    if !left.is_empty() {
        message.push_str(&format!(", {} with no controller left", left.len()));
    }
    Json(json!({"message": message, "evicted": evicted, "blocked": blocked, "left": left})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(kind: &str) -> Value {
        json!({"metadata": {"name": "p", "namespace": "n", "ownerReferences": [{"kind": kind, "controller": true}]}, "status": {"phase": "Running"}})
    }

    #[test]
    fn what_a_drain_does_with_each_pod() {
        assert_eq!(fate(&owned("ReplicaSet")), Fate::Evict);
        assert_eq!(fate(&owned("StatefulSet")), Fate::Evict);
        assert!(matches!(fate(&owned("DaemonSet")), Fate::Stays(w) if w.contains("DaemonSet")));
        assert!(matches!(fate(&owned("Node")), Fate::Stays(w) if w.contains("mirror")));
        assert!(matches!(
            fate(&json!({"metadata": {"annotations": {"kubernetes.io/config.mirror": "x"}}})),
            Fate::Stays(w) if w.contains("mirror")
        ));
        assert_eq!(fate(&json!({"metadata": {"name": "solo"}, "status": {"phase": "Running"}})), Fate::Unmanaged);
        assert_eq!(
            fate(&json!({"metadata": {"ownerReferences": [{"kind": "ReplicaSet"}]}})),
            Fate::Unmanaged,
            "an owner that is not its controller will not bring it back"
        );
        let mut done = owned("Job");
        done["status"]["phase"] = "Succeeded".into();
        assert!(matches!(fate(&done), Fate::Stays("finished")));
        let mut going = owned("ReplicaSet");
        going["metadata"]["deletionTimestamp"] = "2026-01-01T00:00:00Z".into();
        assert!(matches!(fate(&going), Fate::Stays("already going")));
    }

    #[test]
    fn a_plan_sorts_them_and_says_what_data_goes() {
        let mut web = owned("ReplicaSet");
        web["spec"] = json!({"volumes": [{"name": "c", "emptyDir": {}}]});
        let p = plan(&[web, owned("DaemonSet"), json!({"metadata": {"name": "solo", "namespace": "n"}})]);
        assert_eq!(p["evict"].as_array().unwrap().len(), 1);
        assert_eq!(p["evict"][0]["losesData"], true);
        assert_eq!(p["stays"][0]["why"], "a DaemonSet's, which would put it straight back");
        assert_eq!(p["unmanaged"][0]["name"], "solo");
    }

    #[test]
    fn a_replica_count_is_a_whole_number() {
        assert_eq!(replicas(&json!({"replicas": 3})), Ok(3));
        assert_eq!(replicas(&json!({"replicas": "0"})), Ok(0));
        for bad in [json!({"replicas": -1}), json!({"replicas": 1.5}), json!({"replicas": "many"}), json!({}), json!({"replicas": 10_001})] {
            assert!(replicas(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_a_deployment_or_statefulset_scales() {
        assert_eq!(scalable("deploy"), Some("deployments"));
        assert_eq!(scalable("sts"), Some("statefulsets"));
        assert_eq!(scalable("ds"), None);
        assert_eq!(scalable("pod"), None);
    }

    #[test]
    fn who_may_change_a_node() {
        assert!(node_refusal(false, true).unwrap().contains("admin"));
        assert!(node_refusal(true, false).unwrap().contains("identity"), "never the console's credential");
        assert_eq!(node_refusal(true, true), None);
    }

    #[test]
    fn a_drain_with_nowhere_to_go_says_so() {
        let n = |name: &str, off: bool| json!({"metadata": {"name": name}, "spec": {"unschedulable": off}});
        let (a, b) = (n("a", false), n("b", false));
        assert_eq!(only_node_warning(&[&a, &b], "a"), None);
        assert!(only_node_warning(&[&a], "a").is_some(), "a single node");
        let c = n("c", true);
        assert!(only_node_warning(&[&a, &c], "a").is_some(), "the only other one is cordoned");
    }
}
