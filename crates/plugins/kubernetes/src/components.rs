//! Raw kube objects → stormview components. Health comes from the same
//! conditions kubectl reads; relations wire pods to their namespace and
//! node so the grid nests the way the cluster actually hangs together.

use std::collections::HashMap;

use console_core::{Action, ComponentSummary, Health, Metric, Relation};
use serde_json::Value;

type Snapshot = HashMap<&'static str, HashMap<String, Value>>;

fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

fn n(v: &Value, ptr: &str) -> i64 {
    v.pointer(ptr).and_then(Value::as_i64).unwrap_or(0)
}

fn condition<'a>(v: &'a Value, kind: &str) -> Option<&'a str> {
    v.pointer("/status/conditions")?
        .as_array()?
        .iter()
        .find(|c| s(c, "/type") == Some(kind))
        .and_then(|c| s(c, "/status"))
}

fn split_key(key: &str) -> (Option<&str>, &str) {
    match key.split_once('/') {
        Some((ns, name)) => (Some(ns), name),
        None => (None, key),
    }
}

fn base(kind: &str, key: &str, label: &str, health: Health, detail: String) -> ComponentSummary {
    ComponentSummary {
        id: format!("k8s:{kind}:{key}"),
        kind: format!("k8s-{kind}"),
        label: label.to_string(),
        health,
        detail,
        metrics: vec![],
        actions: vec![],
        relations: vec![],
        link: None,
    }
}

fn ns_relation(key: &str) -> Option<Relation> {
    let (ns, _) = split_key(key);
    ns.map(|ns| Relation::belongs_to("namespace", format!("k8s:ns:{ns}")))
}

pub fn map(snap: &Snapshot) -> Vec<ComponentSummary> {
    let mut out = Vec::new();
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);

    // Namespaces first, carrying has_many edges per workload kind.
    for (key, obj) in of("ns") {
        let health = match s(obj, "/status/phase") {
            Some("Active") | None => Health::Ok,
            Some("Terminating") => Health::Warn,
            Some(_) => Health::Unknown,
        };
        let pods = of("pod").keys().filter(|k| k.starts_with(&format!("{key}/"))).count();
        let mut c = base("ns", key, key, health, format!("{pods} pods"));
        for (kind, name) in [
            ("pod", "pods"),
            ("deploy", "deployments"),
            ("sts", "statefulsets"),
            ("ds", "daemonsets"),
            ("job", "jobs"),
            ("cronjob", "cronjobs"),
            ("svc", "services"),
            ("pvc", "pvcs"),
        ] {
            let targets: Vec<String> = of(kind)
                .keys()
                .filter(|k| k.starts_with(&format!("{key}/")))
                .map(|k| format!("k8s:{kind}:{k}"))
                .collect();
            if !targets.is_empty() {
                c.relations.push(Relation::has_many(name, targets));
            }
        }
        c.metrics.push(Metric::new("pods", pods.to_string()));
        c.link = Some(format!("#/grid?id=k8s:ns:{key}"));
        out.push(c);
    }

    for (key, obj) in of("node") {
        let ready = condition(obj, "Ready") == Some("True");
        let unschedulable = obj.pointer("/spec/unschedulable").and_then(Value::as_bool)
            == Some(true);
        let health = match (ready, unschedulable) {
            (true, false) => Health::Ok,
            (true, true) => Health::Warn,
            (false, _) => Health::Error,
        };
        let kubelet = s(obj, "/status/nodeInfo/kubeletVersion").unwrap_or("?");
        let pods_on: Vec<String> = of("pod")
            .iter()
            .filter(|(_, p)| s(p, "/spec/nodeName") == Some(key))
            .map(|(k, _)| format!("k8s:pod:{k}"))
            .collect();
        let detail = format!(
            "{}{} · kubelet {kubelet}",
            if ready { "Ready" } else { "NotReady" },
            if unschedulable { " · cordoned" } else { "" }
        );
        let mut c = base("node", key, key, health, detail);
        c.metrics.push(Metric::new("pods", pods_on.len().to_string()));
        if !pods_on.is_empty() {
            c.relations.push(Relation::has_many("pods", pods_on));
        }
        c.link = Some(format!("#/grid?id=k8s:node:{key}"));
        out.push(c);
    }

    for (key, obj) in of("pod") {
        let (_, name) = split_key(key);
        let phase = s(obj, "/status/phase").unwrap_or("Unknown");
        let ready = condition(obj, "Ready") == Some("True");
        let health = match phase {
            "Running" if ready => Health::Ok,
            "Running" => Health::Warn,
            "Succeeded" => Health::Idle,
            "Pending" => Health::Warn,
            "Failed" => Health::Error,
            _ => Health::Unknown,
        };
        let restarts: i64 = obj
            .pointer("/status/containerStatuses")
            .and_then(Value::as_array)
            .map(|cs| cs.iter().map(|c| n(c, "/restartCount")).sum())
            .unwrap_or(0);
        let node = s(obj, "/spec/nodeName");
        let detail = match node {
            Some(nd) => format!("{phase} · {nd}"),
            None => phase.to_string(),
        };
        let mut c = base("pod", key, name, health, detail);
        c.metrics.push(
            Metric::new("restarts", restarts.to_string())
                .tone(if restarts > 0 { "warn" } else { "muted" }),
        );
        c.relations.extend(ns_relation(key));
        if let Some(nd) = node {
            c.relations.push(Relation::has_one("node", format!("k8s:node:{nd}")));
        }
        c.actions.push(Action {
            id: "delete".into(),
            label: "Delete".into(),
            method: "POST".into(),
            path: format!("/api/plugins/k8s/pods/{key}/delete"),
            enabled: true,
            danger: true,
        });
        out.push(c);
    }

    for (key, obj) in of("deploy") {
        out.push(workload("deploy", key, n(obj, "/spec/replicas"), n(obj, "/status/readyReplicas")));
    }
    for (key, obj) in of("sts") {
        out.push(workload("sts", key, n(obj, "/spec/replicas"), n(obj, "/status/readyReplicas")));
    }
    for (key, obj) in of("ds") {
        out.push(workload(
            "ds",
            key,
            n(obj, "/status/desiredNumberScheduled"),
            n(obj, "/status/numberReady"),
        ));
    }

    for (key, obj) in of("job") {
        let (_, name) = split_key(key);
        let (active, succeeded, failed) =
            (n(obj, "/status/active"), n(obj, "/status/succeeded"), n(obj, "/status/failed"));
        let (health, detail) = if failed > 0 {
            (Health::Error, format!("{failed} failed"))
        } else if active > 0 {
            (Health::Ok, format!("{active} active"))
        } else if succeeded > 0 {
            (Health::Idle, "completed".to_string())
        } else {
            (Health::Unknown, "pending".to_string())
        };
        let mut c = base("job", key, name, health, detail);
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    for (key, obj) in of("cronjob") {
        let (_, name) = split_key(key);
        let schedule = s(obj, "/spec/schedule").unwrap_or("?");
        let suspended = obj.pointer("/spec/suspend").and_then(Value::as_bool) == Some(true);
        let health = if suspended { Health::Idle } else { Health::Ok };
        let detail =
            format!("{schedule}{}", if suspended { " · suspended" } else { "" });
        let mut c = base("cronjob", key, name, health, detail);
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    for (key, obj) in of("svc") {
        let (_, name) = split_key(key);
        let svc_type = s(obj, "/spec/type").unwrap_or("ClusterIP");
        let ip = s(obj, "/spec/clusterIP").unwrap_or("-");
        let mut c = base("svc", key, name, Health::Ok, format!("{svc_type} · {ip}"));
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    for (key, obj) in of("pvc") {
        let (_, name) = split_key(key);
        let phase = s(obj, "/status/phase").unwrap_or("Unknown");
        let health = match phase {
            "Bound" => Health::Ok,
            "Pending" => Health::Warn,
            "Lost" => Health::Error,
            _ => Health::Unknown,
        };
        let size = s(obj, "/status/capacity/storage")
            .or_else(|| s(obj, "/spec/resources/requests/storage"))
            .unwrap_or("?");
        let mut c = base("pvc", key, name, health, format!("{phase} · {size}"));
        c.relations.extend(ns_relation(key));
        out.push(c);
    }

    cilium(snap, &mut out);
    out
}

/// A DELETE against the apiserver path, through the console.
fn delete_action(path: &str) -> stormview::Action {
    stormview::Action {
        id: "delete".into(),
        label: "Delete".into(),
        method: "DELETE".into(),
        path: format!("/api/plugins/k8s/raw{path}"),
        enabled: true,
        danger: true,
    }
}

/// `k=v, k=v` from a matchLabels map, Cilium's `k8s:` prefixes dropped.
fn labels_summary(v: Option<&Value>, max: usize) -> String {
    let Some(map) = v.and_then(Value::as_object) else { return String::new() };
    let mut parts: Vec<String> = map
        .iter()
        .filter(|(k, _)| !k.starts_with("k8s:io.cilium") && !k.starts_with("k8s:io.kubernetes.pod.namespace"))
        .map(|(k, v)| format!("{}={}", k.trim_start_matches("k8s:"), v.as_str().unwrap_or("")))
        .collect();
    parts.sort();
    let more = parts.len().saturating_sub(max);
    parts.truncate(max);
    let mut out = parts.join(", ");
    if more > 0 {
        out.push_str(&format!(" +{more}"));
    }
    out
}

/// One line for a Cilium or core network policy: whom it selects, and how
/// many ingress/egress rules it carries (`specs` counted too).
fn policy_summary(obj: &Value) -> String {
    let mut specs: Vec<&Value> = Vec::new();
    if let Some(sp) = obj.get("spec") {
        specs.push(sp);
    }
    if let Some(arr) = obj.get("specs").and_then(Value::as_array) {
        specs.extend(arr.iter());
    }
    let mut ingress = 0;
    let mut egress = 0;
    let mut selector = String::new();
    for sp in &specs {
        ingress += sp.get("ingress").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        ingress += sp.get("ingressDeny").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        egress += sp.get("egress").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        egress += sp.get("egressDeny").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        if selector.is_empty() {
            selector = labels_summary(
                sp.pointer("/endpointSelector/matchLabels")
                    .or_else(|| sp.pointer("/podSelector/matchLabels"))
                    .or_else(|| sp.pointer("/nodeSelector/matchLabels")),
                3,
            );
        }
    }
    let types = obj
        .pointer("/spec/policyTypes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("/"))
        .unwrap_or_default();
    format!(
        "{} · {ingress} ingress · {egress} egress{}",
        if selector.is_empty() { "all endpoints".to_string() } else { format!("selects {selector}") },
        if types.is_empty() { String::new() } else { format!(" · {types}") }
    )
}

/// Cilium through its CRDs: endpoints (one per pod, with its identity and
/// address), nodes, identities, and policies — plus the core
/// NetworkPolicy — under one `k8s:cilium` card.
fn cilium(snap: &Snapshot, out: &mut Vec<ComponentSummary>) {
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);

    let mut ready = 0usize;
    let mut endpoint_ids = Vec::new();
    for (key, obj) in of("cep") {
        let state = s(obj, "/status/state").unwrap_or("");
        let health = match state {
            "ready" => Health::Ok,
            "waiting-for-identity" | "waiting-to-regenerate" | "regenerating" | "restoring" | "creating" => Health::Warn,
            "" => Health::Unknown,
            _ => Health::Error,
        };
        if health == Health::Ok {
            ready += 1;
        }
        let ipv4 = obj.pointer("/status/networking/addressing/0/ipv4").and_then(Value::as_str).unwrap_or("?");
        let identity = obj.pointer("/status/identity/id").and_then(Value::as_i64);
        let (_, name) = split_key(key);
        let mut c = base(
            "cep",
            key,
            name,
            health,
            format!(
                "{ipv4} · identity {} · {}",
                identity.map(|i| i.to_string()).unwrap_or_else(|| "?".into()),
                if state.is_empty() { "no state" } else { state }
            ),
        );
        c.metrics.push(Metric::new("ipv4", ipv4));
        if let Some(id) = identity {
            c.metrics.push(Metric::new("identity", id.to_string()).tone("muted"));
            if of("cid").contains_key(&id.to_string()) {
                c.relations.push(Relation::has_one("identity", format!("k8s:cid:{id}")));
            }
        }
        if let Some(r) = ns_relation(key) {
            c.relations.push(r);
        }
        if of("pod").contains_key(key) {
            c.relations.push(Relation::has_one("pod", format!("k8s:pod:{key}")));
        }
        endpoint_ids.push(c.id.clone());
        out.push(c);
    }

    let mut node_ids = Vec::new();
    for (key, obj) in of("cn") {
        let ip = obj
            .pointer("/spec/addresses")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|x| s(x, "/type") == Some("InternalIP")))
            .and_then(|x| s(x, "/ip"))
            .unwrap_or("?");
        let cidr = obj
            .pointer("/spec/ipam/podCIDRs/0")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let mut c = base("cn", key, key, Health::Ok, format!("{ip} · pod CIDR {cidr}"));
        c.metrics.push(Metric::new("ip", ip));
        c.metrics.push(Metric::new("pod cidr", cidr).tone("muted"));
        if of("node").contains_key(key) {
            c.relations.push(Relation::has_one("node", format!("k8s:node:{key}")));
        }
        node_ids.push(c.id.clone());
        out.push(c);
    }

    let mut identity_ids = Vec::new();
    for (key, obj) in of("cid") {
        let labels = obj.get("security-labels");
        let ns = labels
            .and_then(|l| l.get("k8s:io.kubernetes.pod.namespace"))
            .and_then(Value::as_str);
        let summary = labels_summary(labels, 3);
        let detail = match ns {
            Some(ns) => format!("{ns} · {summary}"),
            None => summary,
        };
        let mut c = base("cid", key, key, Health::Ok, detail);
        if let Some(ns) = ns {
            c.relations.push(Relation::belongs_to("namespace", format!("k8s:ns:{ns}")));
        }
        identity_ids.push(c.id.clone());
        out.push(c);
    }

    let mut policy_ids = Vec::new();
    for (key, obj) in of("cnp") {
        let (ns, name) = split_key(key);
        let mut c = base("cnp", key, name, Health::Ok, policy_summary(obj));
        if let Some(r) = ns_relation(key) {
            c.relations.push(r);
        }
        c.actions.push(delete_action(&format!(
            "/apis/cilium.io/v2/namespaces/{}/ciliumnetworkpolicies/{name}",
            ns.unwrap_or("default")
        )));
        policy_ids.push(c.id.clone());
        out.push(c);
    }
    for (key, obj) in of("ccnp") {
        let mut c = base("ccnp", key, key, Health::Ok, format!("clusterwide · {}", policy_summary(obj)));
        c.actions.push(delete_action(&format!("/apis/cilium.io/v2/ciliumclusterwidenetworkpolicies/{key}")));
        policy_ids.push(c.id.clone());
        out.push(c);
    }
    for (key, obj) in of("netpol") {
        let (ns, name) = split_key(key);
        let mut c = base("netpol", key, name, Health::Ok, policy_summary(obj));
        if let Some(r) = ns_relation(key) {
            c.relations.push(r);
        }
        c.actions.push(delete_action(&format!(
            "/apis/networking.k8s.io/v1/namespaces/{}/networkpolicies/{name}",
            ns.unwrap_or("default")
        )));
        out.push(c);
    }

    // The card: only once Cilium's CRDs are being served at all.
    if !snap.contains_key("cep") && !snap.contains_key("cn") {
        return;
    }
    let eps = endpoint_ids.len();
    let (health, detail) = if eps == 0 && node_ids.is_empty() {
        (Health::Idle, "no Cilium objects — agent not running or CRDs not installed".to_string())
    } else if eps > 0 && ready == 0 {
        (Health::Error, format!("0/{eps} endpoints ready"))
    } else if ready < eps {
        (Health::Warn, format!("{ready}/{eps} endpoints ready"))
    } else {
        (Health::Ok, format!("{ready}/{eps} endpoints ready"))
    };
    let mut c = base(
        "cilium",
        "cilium",
        "Cilium",
        health,
        format!(
            "{detail} · {} identities · {} nodes · {} policies",
            identity_ids.len(),
            node_ids.len(),
            policy_ids.len() + of("netpol").len()
        ),
    );
    c.kind = "cni".into();
    c.id = "k8s:cilium".into();
    c.metrics = vec![
        Metric::new("endpoints", format!("{ready}/{eps}")).tone(match health {
            Health::Ok => "ok",
            Health::Warn => "warn",
            Health::Error => "error",
            _ => "muted",
        }),
        Metric::new("identities", identity_ids.len().to_string()),
        Metric::new("nodes", node_ids.len().to_string()),
        Metric::new("policies", (policy_ids.len() + of("netpol").len()).to_string()),
    ];
    for (name, ids) in [("endpoints", endpoint_ids), ("nodes", node_ids), ("identities", identity_ids), ("policies", policy_ids)] {
        if !ids.is_empty() {
            c.relations.push(Relation::has_many(name, ids));
        }
    }
    c.link = Some("#/k8s/cep".into());
    out.push(c);
}

fn workload(kind: &'static str, key: &str, desired: i64, ready: i64) -> ComponentSummary {
    let (_, name) = split_key(key);
    let health = if desired == 0 {
        Health::Idle
    } else if ready >= desired {
        Health::Ok
    } else if ready == 0 {
        Health::Error
    } else {
        Health::Warn
    };
    let mut c = base(kind, key, name, health, format!("{ready}/{desired} ready"));
    c.metrics.push(Metric::new("ready", format!("{ready}/{desired}")));
    c.relations.extend(ns_relation(key));
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap_with(kind: &'static str, key: &str, obj: Value) -> Snapshot {
        let mut m = HashMap::new();
        m.insert(kind, HashMap::from([(key.to_string(), obj)]));
        m
    }

    #[test]
    fn running_ready_pod_is_ok() {
        let snap = snap_with(
            "pod",
            "default/web",
            json!({
                "metadata": {"name": "web", "namespace": "default"},
                "spec": {"nodeName": "n1"},
                "status": {
                    "phase": "Running",
                    "conditions": [{"type": "Ready", "status": "True"}],
                    "containerStatuses": [{"restartCount": 2}]
                }
            }),
        );
        let out = map(&snap);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "k8s:pod:default/web");
        assert_eq!(out[0].health, Health::Ok);
        assert_eq!(out[0].detail, "Running · n1");
        assert_eq!(out[0].metrics[0].value, "2");
        assert!(out[0].actions.iter().any(|a| a.id == "delete"));
    }

    #[test]
    fn degraded_deployment_is_warn() {
        let snap = snap_with(
            "deploy",
            "default/api",
            json!({
                "metadata": {"name": "api", "namespace": "default"},
                "spec": {"replicas": 3},
                "status": {"readyReplicas": 1}
            }),
        );
        let out = map(&snap);
        assert_eq!(out[0].health, Health::Warn);
        assert_eq!(out[0].detail, "1/3 ready");
    }

    #[test]
    fn notready_node_is_error_and_namespace_links_pods() {
        let mut snap = snap_with(
            "node",
            "n1",
            json!({
                "metadata": {"name": "n1"},
                "status": {"conditions": [{"type": "Ready", "status": "False"}],
                            "nodeInfo": {"kubeletVersion": "v0.2.3"}}
            }),
        );
        snap.insert(
            "ns",
            HashMap::from([(
                "default".to_string(),
                json!({"metadata": {"name": "default"}, "status": {"phase": "Active"}}),
            )]),
        );
        snap.insert(
            "pod",
            HashMap::from([(
                "default/web".to_string(),
                json!({"metadata": {"name": "web", "namespace": "default"},
                        "spec": {"nodeName": "n1"}, "status": {"phase": "Running"}}),
            )]),
        );
        let out = map(&snap);
        let node = out.iter().find(|c| c.id == "k8s:node:n1").unwrap();
        assert_eq!(node.health, Health::Error);
        assert_eq!(node.relations[0].targets, vec!["k8s:pod:default/web"]);
        let ns = out.iter().find(|c| c.id == "k8s:ns:default").unwrap();
        assert!(ns.relations.iter().any(|r| r.name == "pods"));
    }

    #[test]
    fn cilium_endpoint_and_card() {
        let mut snap: Snapshot = HashMap::new();
        snap.insert("cep", HashMap::from([(
            "kube-system/coredns-1".to_string(),
            json!({"status": {"state": "ready", "identity": {"id": 42},
                "networking": {"addressing": [{"ipv4": "10.0.0.5"}]}}}),
        )]));
        snap.insert("cid", HashMap::from([(
            "42".to_string(),
            json!({"security-labels": {"k8s:io.kubernetes.pod.namespace": "kube-system", "k8s:k8s-app": "kube-dns", "k8s:io.cilium.k8s.policy.cluster": "default"}}),
        )]));
        snap.insert("cn", HashMap::from([(
            "storm-1".to_string(),
            json!({"spec": {"addresses": [{"type": "InternalIP", "ip": "192.168.8.106"}], "ipam": {"podCIDRs": ["10.0.0.0/24"]}}}),
        )]));
        snap.insert("cnp", HashMap::from([(
            "default/allow-dns".to_string(),
            json!({"spec": {"endpointSelector": {"matchLabels": {"app": "web"}}, "egress": [{}, {}]}}),
        )]));
        let out = map(&snap);
        let ep = out.iter().find(|c| c.id == "k8s:cep:kube-system/coredns-1").unwrap();
        assert_eq!(ep.health, Health::Ok);
        assert!(ep.detail.starts_with("10.0.0.5 · identity 42 · ready"), "{}", ep.detail);
        assert!(ep.relations.iter().any(|r| r.targets == vec!["k8s:cid:42"]));
        let id = out.iter().find(|c| c.id == "k8s:cid:42").unwrap();
        assert_eq!(id.detail, "kube-system · k8s-app=kube-dns");
        let pol = out.iter().find(|c| c.id == "k8s:cnp:default/allow-dns").unwrap();
        assert_eq!(pol.detail, "selects app=web · 0 ingress · 2 egress");
        assert_eq!(pol.actions[0].method, "DELETE");
        assert_eq!(pol.actions[0].path, "/api/plugins/k8s/raw/apis/cilium.io/v2/namespaces/default/ciliumnetworkpolicies/allow-dns");
        let card = out.iter().find(|c| c.id == "k8s:cilium").unwrap();
        assert_eq!(card.health, Health::Ok);
        assert!(card.detail.starts_with("1/1 endpoints ready · 1 identities · 1 nodes · 1 policies"), "{}", card.detail);
    }

    #[test]
    fn no_cilium_means_no_card() {
        let snap: Snapshot = HashMap::new();
        assert!(map(&snap).iter().all(|c| c.id != "k8s:cilium"));
    }
}
