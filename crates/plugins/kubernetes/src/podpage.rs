//! The pod page's routes (#69): the detail, the logs (live, previous and
//! the runs the console kept), and the traffic counters.
//!
//! Everything is read as the viewer where the apiserver is asked, and a
//! pod in a namespace hidden from them answers as an absent one.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use console_core::Viewer;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::pod;
use crate::{refuse_hidden, Inner};

/// The kubelet's port. rustkube has no node proxy (rustkube#108), so the
/// counters are read from the kubelet itself, which checks the bearer
/// with a TokenReview against the same apiserver.
const KUBELET_PORT: u16 = 10250;

/// Where stormcentral is, for the provenance of a `stormpump://` golden.
#[derive(Clone)]
pub struct Stormcentral {
    pub url: String,
    pub token: Option<String>,
}

fn not_found(ns: &str, name: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": format!("no pod {ns}/{name}")}))).into_response()
}

async fn the_pod(inner: &Inner, viewer: &Viewer, ns: &str, name: &str) -> Result<Value, Response> {
    if let Some(refusal) = refuse_hidden(inner, viewer, ns).await {
        return Err(refusal);
    }
    inner.store.object("pod", &format!("{ns}/{name}")).await.ok_or_else(|| not_found(ns, name))
}

/// `GET /pods/{ns}/{name}` — the page.
pub async fn detail(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    let pod = match the_pod(&inner, &viewer, &ns, &name).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let snap = inner.store.snapshot().await;
    let mut d = pod::detail(&snap, &ns, &pod);

    d["owners"] = json!(owner_chain(&inner, &viewer, &ns, &pod).await);

    // Each selecting Service's Endpoints: selected is not the same as
    // receiving traffic.
    let ips: Vec<String> = pod::addresses(&pod);
    if let (Some(client), Some(svcs)) = (&inner.client, d["network"]["services"].as_array_mut()) {
        for svc in svcs.iter_mut() {
            let sname = svc["name"].as_str().unwrap_or("").to_string();
            svc["endpoints"] = match client
                .get_as(&format!("/api/v1/namespaces/{ns}/endpoints/{sname}"), viewer.token.as_deref())
                .await
            {
                Ok(eps) => pod::endpoints_summary(&eps, &name, &ips),
                Err(e) => json!({"error": format!("no Endpoints read: {e}")}),
            };
        }
    }

    // The runs the console kept, on each container.
    let uid = pod.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or("");
    let runs = inner.runs.of(uid).await;
    let asset = pod.pointer("/metadata/labels/storm.io~1asset").and_then(Value::as_str).map(str::to_string);
    if let Some(cs) = d["containers"].as_array_mut() {
        for c in cs.iter_mut() {
            let cname = c["name"].as_str().unwrap_or("").to_string();
            c["runs"] = json!(runs.get(&cname).cloned().unwrap_or_default());
            if c["source"] == "stormpump" {
                let image = c["image"].as_str().unwrap_or("");
                let component = asset.clone().unwrap_or_else(|| golden_component(image));
                c["golden"] = golden(&inner, &component).await;
            }
        }
    }
    d["keptRuns"] = json!(crate::logruns::KEEP);

    // What the probes were last seen doing, from this pod's events (#124).
    let events = pod_events(&inner, &ns, &name).await;
    if let (Ok(evs), Some(cs)) = (&events, d["containers"].as_array_mut()) {
        pod::observe_probes(cs, evs);
    }
    Json(d).into_response()
}

/// This pod's events, as the apiserver has them. Listed with the console's
/// credential and filtered here: the viewer's sight of the namespace was
/// settled by [`the_pod`].
async fn pod_events(inner: &Inner, ns: &str, name: &str) -> Result<Vec<Value>, String> {
    let client = inner.client.as_ref().ok_or("no apiserver is configured")?;
    let list = client.get(&crate::objevents::list_path(ns)).await.map_err(|e| e.to_string())?;
    Ok(list
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| crate::objevents::about(e, "Pod", ns, name))
        .cloned()
        .collect())
}

/// The container a field path names: `spec.containers{app}` → `app`.
pub fn container_in(field_path: &str) -> &str {
    field_path
        .split_once('{')
        .and_then(|(_, rest)| rest.split_once('}'))
        .map(|(c, _)| c)
        .unwrap_or("")
}

/// `GET /pods/{ns}/{name}/events` — every event about this pod, newest
/// first, with first and last seen, count, source and the container
/// (`kubectl describe`'s Events table).
pub async fn events(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    if let Err(r) = the_pod(&inner, &viewer, &ns, &name).await {
        return r;
    }
    match pod_events(&inner, &ns, &name).await {
        Ok(evs) => {
            let mut rows: Vec<Value> = evs
                .iter()
                .map(|e| {
                    let ev = crate::objevents::event(e);
                    let container = container_in(&ev.field_path).to_string();
                    let mut v = json!(ev);
                    v["container"] = json!(container);
                    v
                })
                .collect();
            rows.sort_by(|a, b| b["time"].as_str().unwrap_or("").cmp(a["time"].as_str().unwrap_or("")));
            Json(json!({"available": true, "items": rows, "at": chrono::Utc::now().to_rfc3339()})).into_response()
        }
        Err(e) => Json(json!({"available": false, "reason": format!("the apiserver did not answer for events: {e}"), "items": []}))
            .into_response(),
    }
}

/// The component a `stormpump://` image names: `stormpump://cilium` and
/// `stormpump://cilium:1.2` are both `cilium`.
pub fn golden_component(image: &str) -> String {
    let rest = image.strip_prefix("stormpump://").unwrap_or(image);
    let rest = rest.split('/').next().unwrap_or(rest);
    rest.split(':').next().unwrap_or(rest).to_string()
}

/// The newest golden stormcentral built for a component.
///
/// Labelled as exactly that: nothing on the pod says *which* golden the
/// node runs (rustkube-node#130), so the page says "the newest built"
/// rather than claiming it is this one.
async fn golden(inner: &Inner, component: &str) -> Value {
    let Some(sc) = &inner.stormcentral else {
        return json!({
            "available": false,
            "reason": "stormcentral is not configured here ([stormcentral] url and token_file)",
        });
    };
    let url = format!("{}/api/v1/goldens?component={component}", sc.url.trim_end_matches('/'));
    let mut req = inner.http.get(&url).timeout(Duration::from_secs(5));
    if let Some(t) = &sc.token {
        req = req.bearer_auth(t.trim());
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return json!({"available": false, "reason": format!("stormcentral did not answer: {e}")}),
    };
    if !resp.status().is_success() {
        return json!({"available": false, "reason": format!("stormcentral answered {}", resp.status())});
    }
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    match body.pointer("/goldens/0") {
        Some(g) => json!({
            "available": true,
            "component": component,
            "which": "the newest registry image stormcentral built for this component; the node does not report which one its instance was cloned from (rustkube-node#130)",
            "name": g.get("name"),
            "version": g.get("version"),
            "commit": g.get("commit"),
            "buildId": g.get("build_id"),
            "builtAt": g.get("built_at"),
            "builtBy": g.get("built_by"),
            "tarSha256": g.get("tar_sha256"),
            "deviceSha256": g.get("device_sha256"),
            "sources": g.get("sources"),
            "releases": g.get("releases"),
        }),
        None => json!({"available": false, "reason": format!("stormcentral has no registry image for {component}")}),
    }
}

/// The owner references, followed up: a pod's ReplicaSet to its
/// Deployment, a Job to its CronJob. Four steps at most.
async fn owner_chain(inner: &Inner, viewer: &Viewer, ns: &str, pod: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let mut owners: Vec<Value> =
        pod.pointer("/metadata/ownerReferences").and_then(Value::as_array).cloned().unwrap_or_default();
    for _ in 0..4 {
        let Some(o) = owners.iter().find(|o| o.get("controller").and_then(Value::as_bool) == Some(true)).or(owners.first()).cloned() else {
            break;
        };
        let kind = o.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
        let name = o.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        out.push(json!({
            "kind": kind,
            "name": name,
            "href": pod::owner_href(&kind, ns, &name),
        }));
        let next = if let Some(k) = pod::cached_kind(&kind) {
            inner.store.object(k, &format!("{ns}/{name}")).await
        } else if kind == "ReplicaSet" {
            match &inner.client {
                Some(c) => c
                    .get_as(&format!("/apis/apps/v1/namespaces/{ns}/replicasets/{name}"), viewer.token.as_deref())
                    .await
                    .ok(),
                None => None,
            }
        } else {
            None
        };
        owners = next
            .and_then(|v| v.pointer("/metadata/ownerReferences").and_then(Value::as_array).cloned())
            .unwrap_or_default();
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogQuery {
    container: Option<String>,
    #[serde(default)]
    previous: bool,
    tail_lines: Option<u64>,
    limit_bytes: Option<u64>,
    #[serde(default)]
    timestamps: bool,
    #[serde(default)]
    follow: bool,
    #[serde(default)]
    download: bool,
}

/// The apiserver's `pods/log` query for these options.
pub fn log_path(ns: &str, name: &str, q: &LogQuery) -> String {
    let mut parts = Vec::new();
    if let Some(c) = &q.container {
        parts.push(format!("container={}", urlencode(c)));
    }
    if q.previous {
        parts.push("previous=true".into());
    }
    if let Some(n) = q.tail_lines {
        parts.push(format!("tailLines={n}"));
    }
    if let Some(n) = q.limit_bytes {
        parts.push(format!("limitBytes={n}"));
    }
    if q.timestamps {
        parts.push("timestamps=true".into());
    }
    if q.follow {
        parts.push("follow=true".into());
    }
    let mut path = format!("/api/v1/namespaces/{ns}/pods/{name}/log");
    if !parts.is_empty() {
        path.push('?');
        path.push_str(&parts.join("&"));
    }
    path
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `GET /pods/{ns}/{name}/log` — the apiserver's pods/log, asked as the
/// viewer and passed through as it arrives, so following is a stream.
pub async fn log(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    Query(q): Query<LogQuery>,
) -> Response {
    if let Err(r) = the_pod(&inner, &viewer, &ns, &name).await {
        return r;
    }
    let Some(client) = &inner.client else {
        return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": "no apiserver"}))).into_response();
    };
    let resp = match client.get_raw_as(&log_path(&ns, &name, &q), viewer.token.as_deref()).await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, Json(json!({"error": format!("the apiserver did not answer: {e}")})))
                .into_response()
        }
    };
    let status = resp.status();
    if !status.is_success() {
        // The apiserver's own words: "previous terminated container not
        // found" is the answer, not a bare 400.
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or(body);
        let code = StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
        return (code, Json(json!({"error": msg.trim()}))).into_response();
    }
    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no");
    if q.download {
        let c = q.container.as_deref().unwrap_or("log");
        let which = if q.previous { "-previous" } else { "" };
        builder = builder.header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{ns}_{name}_{c}{which}.log\""),
        );
    }
    builder.body(Body::from_stream(resp.bytes_stream())).unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `GET /pods/{ns}/{name}/runs/{container}/{run}` — one kept run.
pub async fn run_text(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name, container, run)): Path<(String, String, String, i64)>,
    Query(q): Query<LogQuery>,
) -> Response {
    let pod = match the_pod(&inner, &viewer, &ns, &name).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let uid = pod.pointer("/metadata/uid").and_then(Value::as_str).unwrap_or("");
    match inner.runs.text(uid, &container, run).await {
        Some(text) => {
            let mut b = Response::builder().header(header::CONTENT_TYPE, "text/plain; charset=utf-8");
            if q.download {
                b = b.header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{ns}_{name}_{container}-run{run}.log\""),
                );
            }
            b.body(Body::from(text)).unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("run {run} of {container} is not kept (the console keeps the last {})", crate::logruns::KEEP)})),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct StatsQuery {
    /// Seconds back from now; 15 minutes unless asked, an hour at most.
    window: Option<i64>,
}

/// `GET /pods/{ns}/{name}/stats` — this pod's samples over the window, as
/// rates, from what the console scraped of its node's kubelet (#124).
pub async fn stats(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
    Query(q): Query<StatsQuery>,
) -> Response {
    let pod = match the_pod(&inner, &viewer, &ns, &name).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let node = pod.pointer("/spec/nodeName").and_then(Value::as_str).unwrap_or("");
    let host_network = pod.pointer("/spec/hostNetwork").and_then(Value::as_bool) == Some(true);
    let window = q.window.unwrap_or(900).clamp(60, crate::stats::WINDOW_SECS);
    let now = chrono::Utc::now().timestamp_millis();
    let mut v = inner.stats.view(&format!("{ns}/{name}"), node, host_network, window, now).await;
    v["now"] = json!(now);
    Json(v).into_response()
}

/// `GET /pods/{ns}/{name}/traffic` — this pod's counters from its node's
/// kubelet, with the time they were read so the page can make rates.
pub async fn traffic(
    State(inner): State<Arc<Inner>>,
    viewer: Viewer,
    Path((ns, name)): Path<(String, String)>,
) -> Response {
    let pod = match the_pod(&inner, &viewer, &ns, &name).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let at = chrono::Utc::now().timestamp_millis();
    let none = |reason: String| Json(json!({"available": false, "reason": reason, "at": at})).into_response();
    if pod.pointer("/spec/hostNetwork").and_then(Value::as_bool) == Some(true) {
        return none("this pod is on the host network: its traffic is the node's, which the kubelet does not count per pod".into());
    }
    let node = pod.pointer("/spec/nodeName").and_then(Value::as_str).unwrap_or("");
    let mut host = pod.pointer("/status/hostIP").and_then(Value::as_str).unwrap_or("").to_string();
    if host.is_empty() {
        let snap = inner.store.snapshot().await;
        host = pod::node_address(&snap, node).unwrap_or_default();
    }
    if host.is_empty() {
        return none("the pod is not on a node yet".into());
    }
    let h = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    let url = format!("https://{h}:{KUBELET_PORT}/metrics/cadvisor");
    let token = viewer.token.clone().or_else(|| inner.client.as_ref().and_then(|c| c.token()));
    // The apiserver connection's client: a bearer goes to the kubelet only
    // when its certificate is checked the way the apiserver's is (#33).
    let http = inner.client.as_ref().map(|c| c.conn().http()).unwrap_or_else(|| inner.http.clone());
    let mut req = http.get(&url).timeout(Duration::from_secs(5));
    if let Some(t) = &token {
        req = req.bearer_auth(t);
    }
    let text = match req.send().await {
        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED || r.status() == reqwest::StatusCode::FORBIDDEN => {
            return none(format!(
                "the kubelet on {node} refused the console's credential ({}): it checks the bearer with the apiserver, and {}",
                r.status(),
                if token.is_some() { "this one was not accepted" } else { "the console has none ([kubernetes] token or token_file)" }
            ))
        }
        Ok(r) => return none(format!("the kubelet on {node} ({host}) answered {}", r.status())),
        Err(e) => return none(format!("the kubelet on {node} ({host}) did not answer: {e}")),
    };
    let interfaces = pod::traffic(&text, &ns, &name);
    if interfaces.is_empty() {
        return none(format!("the kubelet on {node} reports no interfaces for this pod"));
    }
    let complete = interfaces.iter().any(|i| i.get("rxErrors").is_some());
    Json(json!({
        "available": true,
        "at": at,
        "node": node,
        "interfaces": interfaces,
        "missing": if complete { Value::Null } else {
            json!(format!("packets, errors and drops: the kubelet exports bytes only ({})", pod::NETWORK_ISSUE))
        },
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_golden_is_named_by_its_component() {
        assert_eq!(golden_component("stormpump://cilium"), "cilium");
        assert_eq!(golden_component("stormpump://cilium:1.16"), "cilium");
        assert_eq!(golden_component("stormpump://stormd/agent"), "stormd");
    }

    #[test]
    fn a_field_path_names_its_container() {
        assert_eq!(container_in("spec.containers{app}"), "app");
        assert_eq!(container_in("spec.initContainers{setup}"), "setup");
        assert_eq!(container_in(""), "");
    }

    #[test]
    fn log_options_become_the_apiservers_query() {
        let q = LogQuery {
            container: Some("app x".into()),
            previous: true,
            tail_lines: Some(500),
            limit_bytes: None,
            timestamps: true,
            follow: false,
            download: false,
        };
        assert_eq!(
            log_path("shop", "web-1", &q),
            "/api/v1/namespaces/shop/pods/web-1/log?container=app%20x&previous=true&tailLines=500&timestamps=true"
        );
        let q = LogQuery { container: None, previous: false, tail_lines: None, limit_bytes: None, timestamps: false, follow: true, download: true };
        assert_eq!(log_path("a", "b", &q), "/api/v1/namespaces/a/pods/b/log?follow=true");
    }
}
