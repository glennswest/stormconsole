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

    out
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
}
