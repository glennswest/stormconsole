use super::*;
use axum::routing::post;

/// A stand-in agent on a loopback port, in flowsdn's shapes: Kubernetes
/// mode when `k8s`, else standalone (the Kubernetes routes 404).
async fn agent(k8s: bool) -> (String, tokio::task::JoinHandle<()>) {
    let ep = model::tests::endpoint_k8s();
    let mut other = ep.clone();
    other["id"] = json!(43);
    other["status"]["state"] = json!("disconnecting");
    other["status"]["pod"]["namespace"] = json!("secret");
    other["status"]["pod"]["pod_name"] = json!("vault-0");
    other["status"]["pod"]["node_name"] = json!("node-a");
    let list = json!([ep.clone(), other]);
    let healthz = if k8s {
        json!({"agent": {"state": "Ok", "msg": "initial endpoint API ready"},
               "kubernetes": {"state": "Ok", "msg": "node-a: 1 node, 2 pods", "node-name": "node-a",
                              "auto-direct-node-routes": true, "service-lb": true}})
    } else {
        json!({"agent": {"state": "Ok", "msg": "initial endpoint API ready"}})
    };
    let k8s_route = move |v: Value| move || async move {
        if k8s { Json(v).into_response() } else {
            (StatusCode::NOT_FOUND, Json(json!("kubernetes node discovery is not enabled"))).into_response()
        }
    };
    let app = Router::new()
        .route("/v1/healthz", get(move || async move { Json(healthz) }))
        .route("/v1/health/modules", get(|| async {
            Json(json!([{"ID": {"Module": ["agent"], "Component": ["api"]}, "Level": "OK", "Message": "listening",
                         "Error": "", "LastOK": "2026-10-10T10:00:00Z", "Updated": "2026-10-10T10:00:00Z", "Count": 1}]))
        }))
        .route("/v1/endpoint", get(move || async move { Json(list) }))
        .route("/v1/endpoint/{id}", get(move |Path(id): Path<String>| async move {
            match id.as_str() {
                "42" | "sandbox-id%3Aeth0" | "sandbox-id:eth0" => Json(ep).into_response(),
                _ => (StatusCode::NOT_FOUND, Json(json!("endpoint not found"))).into_response(),
            }
        }))
        .route("/v1/endpoint/{id}/healthz", get(|| async {
            Json(json!({"overallHealth": "OK", "bpf": "OK", "policy": "Disabled", "connected": true}))
        }))
        .route("/v1/ipam", get(|| async {
            Json(json!({"pools": [{"pool": "default", "family": "ipv4", "cidr": "10.0.0.0/24", "capacity": "254",
                "allocated": "2", "excluded": "1", "allocated-excluded": "0", "available": "251"}]}))
        }))
        .route("/v1/config", get(|| async {
            Json(json!({"status": {"datapath-mode": "veth", "ipam-mode": "kubernetes", "device-mtu": 1500,
                                   "route-mtu": 1450, "host-addressing": {}}}))
        }))
        .route("/v1/statedb/query", post(|Json(q): Json<Value>| async move {
            assert_eq!(q["table"], "health");
            "{\"rev\":3,\"obj\":{\"ID\":{\"Module\":[\"agent\"],\"Component\":[\"api\"]},\"Level\":\"OK\"}}\n".to_string()
        }))
        .route("/v1/service", get(k8s_route(json!([{"spec": {"id": 1,
            "frontend-address": {"ip": "10.96.0.1", "port": 443, "protocol": "TCP", "scope": "external"},
            "backend-addresses": [{"ip": "192.168.8.10", "port": 6443, "protocol": "TCP", "state": "active"}],
            "flags": {"type": "ClusterIP", "name": "kubernetes", "namespace": "default"}}}]))))
        .route("/v1/node/routes", get(k8s_route(json!([]))))
        .route("/v1/identity", get(k8s_route(json!([{"id": 31337, "labels": ["k8s:app=web"]}]))));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let h = tokio::spawn(async move {
        axum::serve(l, app).await.unwrap();
    });
    (url, h)
}

fn manifest(dir: &std::path::Path, body: &str) -> String {
    let p = dir.join("manifest.json");
    std::fs::write(&p, body).unwrap();
    p.to_string_lossy().into()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("flowsdn-test-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn a_kubernetes_agent_becomes_rows_that_lead_with_the_pod() {
    let (url, _h) = agent(true).await;
    let m = manifest(&tmp("k8s"), r#"{"version":"12.13-flowsdn"}"#);
    let p = FlowsdnPlugin::new(&url, &m, None);
    assert!(!p.nav().is_empty(), "a flowsdn node has the page");
    p.poll().await;
    let s = p.snapshot().await;
    assert!(s.reachable, "{}", s.error);
    assert_eq!(s.error, "");
    assert_eq!(s.node(), "node-a");
    assert_eq!(s.services.as_ref().unwrap()[0].name, "kubernetes");
    assert_eq!(s.identities.as_ref().unwrap().len(), 1);

    let c = p.components().await;
    let agent = &c[0];
    assert_eq!((agent.label.as_str(), agent.health), ("flowsdn agent on node-a", Health::Ok));
    assert!(agent.metrics.iter().any(|m| m.label == "datapath" && m.value == "veth, MTU 1450"));
    assert!(agent.metrics.iter().any(|m| m.label == "ready" && m.value == "1/2"));
    let ep = c.iter().find(|c| c.id == "flowsdn:ep:42").unwrap();
    assert_eq!(ep.label, "default/example-pod");
    assert_eq!(ep.link.as_deref(), Some("#/flowsdn?ep=42"));
    let rel = |n: &str| ep.relations.iter().find(|r| r.name == n).map(|r| r.targets[0].clone());
    assert_eq!(rel("namespace").as_deref(), Some("k8s:ns:default"));
    assert_eq!(rel("node").as_deref(), Some("k8s:node:node-a"));
    assert_eq!(rel("pod").as_deref(), Some("k8s:pod:default/example-pod"));
    assert!(ep.metrics.iter().any(|m| m.label == "identity" && m.value == "31337"));
    let down = c.iter().find(|c| c.id == "flowsdn:ep:43").unwrap();
    assert_eq!(down.health, Health::Warn);
    let pool = c.iter().find(|c| c.id == "flowsdn:pool:default:ipv4").unwrap();
    assert_eq!(pool.detail, "10.0.0.0/24 · 251 of 254 free");
}

#[tokio::test]
async fn a_standalone_agent_serves_no_kubernetes_routes_and_that_is_not_an_error() {
    let (url, _h) = agent(false).await;
    let p = FlowsdnPlugin::new(&url, "/nonexistent/manifest.json", None);
    p.poll().await;
    let s = p.snapshot().await;
    assert!(s.reachable);
    assert_eq!(s.error, "", "404 on /v1/service is 'not served'");
    assert!(s.services.is_none() && s.routes.is_none() && s.identities.is_none());
    assert_eq!(p.health().await, Health::Ok);
    let c = p.components().await;
    assert!(c[0].metrics.iter().any(|m| m.label == "mode" && m.value == "standalone"));
}

#[tokio::test]
async fn an_agent_that_stops_answering_keeps_its_last_good_answer() {
    let (url, h) = agent(true).await;
    let p = FlowsdnPlugin::new(&url, &manifest(&tmp("stale"), r#"{"edition":"flowsdn"}"#), None);
    p.poll().await;
    h.abort();
    let _ = h.await;
    p.poll().await;
    let s = p.snapshot().await;
    assert!(!s.reachable);
    assert_eq!(s.endpoints.len(), 2, "the endpoints are kept");
    let (health, sentence) = s.health();
    assert_eq!(health, Health::Error);
    assert!(sentence.starts_with("the agent stopped answering"), "{sentence}");
    let c = p.components().await;
    assert!(c[0].metrics.iter().any(|m| m.label == "as of"));
    assert_eq!(c.iter().filter(|c| c.kind == "endpoint").count(), 2);
}

#[tokio::test]
async fn silence_is_an_error_on_a_flowsdn_node_and_not_on_a_node_of_unknown_edition() {
    // A port nothing listens on.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    drop(l);
    let flowsdn = FlowsdnPlugin::new(&url, &manifest(&tmp("silent"), r#"{"edition":"flowsdn"}"#), None);
    flowsdn.poll().await;
    let (h, why) = flowsdn.snapshot().await.health();
    assert_eq!(h, Health::Error);
    assert!(why.contains("no flowsdn agent answers") && why.contains("nothing answers"), "{why}");

    let unknown = FlowsdnPlugin::new(&url, "/nonexistent/manifest.json", None);
    unknown.poll().await;
    let (h, why) = unknown.snapshot().await.health();
    assert_eq!(h, Health::Unknown);
    assert!(why.contains("cilium-edition node has none"), "{why}");
}

#[tokio::test]
async fn a_cilium_node_says_not_this_edition_and_asks_nothing() {
    let (url, _h) = agent(true).await;
    let p = FlowsdnPlugin::new(&url, &manifest(&tmp("cilium"), r#"{"edition":"cilium"}"#), None);
    assert!(p.nav().is_empty(), "no page on a cilium node");
    p.poll().await;
    let s = p.snapshot().await;
    assert!(s.answered.is_none(), "the agent is not asked");
    let c = p.components().await;
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].health, Health::Idle);
    assert!(c[0].detail.starts_with("not this edition"), "{}", c[0].detail);
    assert!(c[0].link.is_none());
}

#[tokio::test]
async fn routes_read_one_endpoint_live_and_only_named_tables() {
    let (url, _h) = agent(true).await;
    let p = FlowsdnPlugin::new(&url, "/nonexistent", None);
    p.poll().await;
    let app = Router::new().nest("/p", p.routes());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/p", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let c = reqwest::Client::new();
    let get = |p: String| {
        let c = c.clone();
        async move {
            let r = c.get(p).send().await.unwrap();
            (r.status().as_u16(), r.json::<Value>().await.unwrap())
        }
    };
    let (st, v) = get(format!("{base}/endpoint/42")).await;
    assert_eq!(st, 200);
    assert_eq!(v["row"]["pod"], "example-pod");
    assert_eq!(v["link"]["connected"], true);
    assert_eq!(v["identity"]["labels"][0], "k8s:app=web");
    let (st, _) = get(format!("{base}/endpoint/sandbox-id:eth0")).await;
    assert_eq!(st, 200, "an attachment id, encoded");
    let (st, v) = get(format!("{base}/endpoint/99")).await;
    assert_eq!((st, v["error"].as_str().unwrap()), (404, "the agent has no endpoint 99"));
    let (st, v) = get(format!("{base}/state/health")).await;
    assert_eq!(st, 200);
    assert_eq!(v["rows"][0]["row"]["id"], "agent.api");
    assert_eq!(v["rows"][0]["rev"], 3);
    let (st, _) = get(format!("{base}/state/secrets")).await;
    assert_eq!(st, 404, "a table off the allowlist is never asked for");
    let (st, v) = get(format!("{base}/snapshot")).await;
    assert_eq!(st, 200);
    assert_eq!(v["node"], "node-a");
    assert_eq!(v["snapshot"]["endpoints"].as_array().unwrap().len(), 2);
    assert!(v["flows"].as_str().unwrap().contains("flowsdn#293"));
}

#[test]
fn endpoints_follow_their_namespace() {
    let ns: HashMap<String, String> = [("flowsdn:ep:42".to_string(), "default".to_string()),
                                       ("flowsdn:ep:43".to_string(), "secret".to_string())].into();
    let hidden: HashSet<String> = ["secret".to_string()].into();
    assert!(visible("flowsdn:ep:42", &ns, &hidden));
    assert!(!visible("flowsdn:ep:43", &ns, &hidden));
    assert!(visible("flowsdn:agent", &ns, &hidden), "the node's own rows are not namespaced");
    assert!(visible("flowsdn:pool:default:ipv4", &ns, &hidden));
}

#[test]
fn ids_are_encoded_for_the_agents_path() {
    assert_eq!(encode_id("42"), "42");
    assert_eq!(encode_id("sandbox-id:eth0"), "sandbox-id%3Aeth0");
    assert_eq!(encode_id("../v1/config"), "..%2Fv1%2Fconfig");
}
