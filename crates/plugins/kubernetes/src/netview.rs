//! The Network Policies and Network Endpoints pages (#168): one table, one adapter
//! per network. The owner: "Network Policies (No cillium) Network Endpoints (No
//! Cillium), and these should come from either cillium or flowsdn".
//!
//! Both networks keep these as objects the apiserver holds, with the same schemas
//! ("change the API group and the kind", flowsdn `docs/crds.md`), so one reader does
//! both and only the kinds differ. Kubernetes' own NetworkPolicy is either network's.
//! A flowsdn node's endpoints are also on its agent, which the flowsdn plugin turns
//! into this same row.
//!
//! No I/O in this file.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

use crate::components::Snapshot;
use crate::network;

/// Which network the objects say this cluster runs: served CRDs are the tell. An
/// unserved kind is absent from the snapshot, and an empty one present.
pub fn network_of(snap: &Snapshot) -> &'static str {
    if ["cep", "cn", "cnp", "ccnp"].iter().any(|k| snap.contains_key(k)) {
        "cilium"
    } else if ["fsep", "fsid", "fsnp", "fscnp"].iter().any(|k| snap.contains_key(k)) {
        "flowsdn"
    } else {
        ""
    }
}

/// `k=v, k=v` with the networks' own label prefixes gone.
fn selector_line(sel: &Value) -> String {
    let mut parts: Vec<String> = sel
        .get("matchLabels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(k, v)| format!("{}={}", k.strip_prefix("k8s:").unwrap_or(k), v.as_str().unwrap_or("")))
        .collect();
    parts.sort();
    for e in sel.get("matchExpressions").and_then(Value::as_array).into_iter().flatten() {
        let vals = e.get("values").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_default();
        parts.push(format!(
            "{} {} {vals}",
            e.get("key").and_then(Value::as_str).unwrap_or(""),
            e.get("operator").and_then(Value::as_str).unwrap_or("")
        ).trim().to_string());
    }
    parts.join(", ")
}

/// The specs a policy carries: `spec`, and a CRD's `specs`.
fn specs_of(policy: &Value) -> Vec<&Value> {
    policy.get("spec").into_iter().chain(policy.get("specs").and_then(Value::as_array).into_iter().flatten()).collect()
}

fn selector_of(spec: &Value) -> Option<&Value> {
    spec.get("endpointSelector").or_else(|| spec.get("podSelector")).or_else(|| spec.get("nodeSelector"))
}

fn count(spec: &Value, keys: &[&str]) -> usize {
    keys.iter().map(|k| spec.get(*k).and_then(Value::as_array).map_or(0, Vec::len)).sum()
}

/// A pod's labels as a policy sees them: its own, and its namespace under the key a
/// network's selector uses.
fn pod_labels(ns: &str, pod: &Value) -> HashMap<String, String> {
    let mut l: HashMap<String, String> = pod
        .pointer("/metadata/labels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
        .collect();
    l.insert("io.kubernetes.pod.namespace".into(), ns.to_string());
    l
}

const POLICIES: &[(&str, &str, bool)] = &[
    // kind, what it is, namespaced
    ("netpol", "standard", true),
    ("cnp", "extended", true),
    ("ccnp", "cluster-wide", false),
    ("fsnp", "extended", true),
    ("fscnp", "cluster-wide", false),
];

/// Every policy this viewer may see: whom it selects, its rules, and the pods it
/// applies to. A selector with `matchExpressions` is said so, not half evaluated.
pub fn policies(snap: &Snapshot, hidden: Option<&HashSet<String>>) -> Vec<Value> {
    let from = network_of(snap);
    let empty = HashMap::new();
    let pods = snap.get("pod").unwrap_or(&empty);
    let hide = |ns: &str| hidden.is_some_and(|h| h.contains(ns));
    let mut out = Vec::new();
    for (kind, sort, namespaced) in POLICIES {
        for (key, obj) in snap.get(kind).unwrap_or(&empty) {
            let (project, name) = match (namespaced, key.split_once('/')) {
                (true, Some((ns, n))) => (ns, n),
                _ => ("", key.as_str()),
            };
            if hide(project) {
                continue;
            }
            let specs = specs_of(obj);
            let (mut ingress, mut egress) = (0, 0);
            let (mut selects, mut unevaluated) = (Vec::new(), false);
            for sp in &specs {
                ingress += count(sp, &["ingress", "ingressDeny"]);
                egress += count(sp, &["egress", "egressDeny"]);
                if let Some(sel) = selector_of(sp) {
                    selects.push(selector_line(sel));
                    unevaluated |= sel.get("matchExpressions").is_some();
                }
            }
            let (mut applies, mut withheld) = (Vec::new(), 0);
            for (pod_key, pod) in pods {
                let Some((ns, pod_name)) = pod_key.split_once('/') else { continue };
                if *namespaced && ns != project {
                    continue;
                }
                let labels = pod_labels(ns, pod);
                let hit = specs.iter().any(|sp| network::selects(&json!({"spec": sp}), &labels));
                if hit {
                    if hide(ns) {
                        withheld += 1;
                    } else {
                        applies.push(json!({"project": ns, "name": pod_name, "href": format!("#/pod/{ns}/{pod_name}")}));
                    }
                }
            }
            applies.sort_by_key(|a| (a["project"].as_str().unwrap_or("").to_string(), a["name"].as_str().unwrap_or("").to_string()));
            out.push(json!({
                "id": format!("k8s:{kind}:{key}"),
                "name": name,
                "project": project,
                "sort": sort,
                "from": if from.is_empty() { "kubernetes" } else { from },
                "selects": selects.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>(),
                "ingress": ingress,
                "egress": egress,
                "types": obj.pointer("/spec/policyTypes").cloned().unwrap_or(json!([])),
                "appliesTo": applies,
                "withheld": withheld,
                "unevaluated": unevaluated,
            }));
        }
    }
    out.sort_by_key(|p| (p["project"].as_str().unwrap_or("").to_string(), p["name"].as_str().unwrap_or("").to_string()));
    out
}

fn enforcing(cep: &Value, dir: &str) -> Value {
    let p = cep.pointer(&format!("/status/policy/{dir}"));
    match p.and_then(|p| p.get("enforcing")).and_then(Value::as_bool) {
        Some(b) => json!(b),
        None => match p.and_then(|p| p.get("state")).and_then(Value::as_str) {
            Some(s) => json!(s == "enforcing"),
            None => Value::Null,
        },
    }
}

/// Every endpoint this viewer may see, from the objects a network writes. A pod's
/// node is its own; an endpoint with no pod behind its name is a machine's, because a
/// VM's endpoint is filed under the machine (#51).
pub fn endpoints(snap: &Snapshot, hidden: Option<&HashSet<String>>) -> Vec<Value> {
    let empty = HashMap::new();
    let pods = snap.get("pod").unwrap_or(&empty);
    let mut out = Vec::new();
    for (kind, ids, from) in [("cep", "cid", "cilium"), ("fsep", "fsid", "flowsdn")] {
        for (key, ep) in snap.get(kind).unwrap_or(&empty) {
            let Some((ns, name)) = key.split_once('/') else { continue };
            if hidden.is_some_and(|h| h.contains(ns)) {
                continue;
            }
            let mut ips = Vec::new();
            for a in ep.pointer("/status/networking/addressing").and_then(Value::as_array).into_iter().flatten() {
                for f in ["ipv4", "ipv6"] {
                    if let Some(ip) = a.get(f).and_then(Value::as_str) {
                        ips.push(ip.to_string());
                    }
                }
            }
            let pod = pods.get(key);
            let node = pod
                .and_then(|p| p.pointer("/spec/nodeName"))
                .or_else(|| ep.pointer("/status/networking/node"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let identity = ep.pointer("/status/identity/id").and_then(Value::as_i64);
            let labels = identity
                .and_then(|i| snap.get(ids).and_then(|m| m.get(&i.to_string())))
                .map(network::identity_labels)
                .unwrap_or_default();
            let state = ep.pointer("/status/state").and_then(Value::as_str).unwrap_or("");
            out.push(json!({
                "id": format!("k8s:{kind}:{key}"),
                "kind": if pod.is_some() { "Pod" } else { "VM" },
                "project": ns,
                "name": name,
                "href": if pod.is_some() { format!("#/pod/{ns}/{name}") } else { format!("#/vm/{ns}/{name}") },
                "ips": ips,
                "node": node,
                "identity": identity,
                "identityLabels": labels,
                "ingress": enforcing(ep, "ingress"),
                "egress": enforcing(ep, "egress"),
                "state": state,
                "ready": state == "ready",
                "from": from,
            }));
        }
    }
    out.sort_by_key(|e| (e["project"].as_str().unwrap_or("").to_string(), e["name"].as_str().unwrap_or("").to_string()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(kinds: &[(&'static str, Vec<(&str, Value)>)]) -> Snapshot {
        kinds.iter().map(|(k, v)| (*k, v.iter().map(|(a, b)| (a.to_string(), b.clone())).collect())).collect()
    }

    fn pods() -> (&'static str, Vec<(&'static str, Value)>) {
        ("pod", vec![
            ("shop/web-1", json!({"metadata": {"labels": {"app": "web"}}, "spec": {"nodeName": "n1"}})),
            ("shop/db-1", json!({"metadata": {"labels": {"app": "db"}}})),
            ("other/web-2", json!({"metadata": {"labels": {"app": "web"}}})),
        ])
    }

    #[test]
    fn one_table_whichever_network_wrote_it() {
        for (np, cw, ep, id, word) in [("cnp", "ccnp", "cep", "cid", "cilium"), ("fsnp", "fscnp", "fsep", "fsid", "flowsdn")] {
            let s = snap(&[
                pods(),
                ("netpol", vec![("shop/deny", json!({"spec": {"podSelector": {}, "policyTypes": ["Ingress"]}}))]),
                (np, vec![("shop/allow-web", json!({"spec": {"endpointSelector": {"matchLabels": {"k8s:app": "web"}}, "ingress": [{}, {}], "egress": [{}]}}))]),
                (cw, vec![("everyweb", json!({"specs": [{"endpointSelector": {"matchLabels": {"app": "web"}}, "egressDeny": [{}]}]}))]),
                (ep, vec![("shop/web-1", json!({"status": {"state": "ready", "identity": {"id": 7}, "networking": {"node": "10.0.0.1", "addressing": [{"ipv4": "10.1.0.5", "ipv6": "fd00::5"}]},
                    "policy": {"ingress": {"enforcing": true}, "egress": {"state": "non-enforcing"}}}}))]),
                (id, vec![("7", json!({"security-labels": {"k8s:app": "web"}}))]),
            ]);
            assert_eq!(network_of(&s), word);
            let p = policies(&s, None);
            let row = |n: &str| p.iter().find(|r| r["name"] == n).unwrap().clone();
            let allow = row("allow-web");
            assert_eq!((allow["sort"].as_str(), allow["from"].as_str()), (Some("extended"), Some(word)));
            assert_eq!(allow["selects"], json!(["app=web"]));
            assert_eq!((allow["ingress"].as_u64(), allow["egress"].as_u64()), (Some(2), Some(1)));
            assert_eq!(allow["appliesTo"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect::<Vec<_>>(), ["web-1"], "its own project only");
            let everyweb = row("everyweb");
            assert_eq!((everyweb["project"].as_str(), everyweb["sort"].as_str()), (Some(""), Some("cluster-wide")));
            assert_eq!(everyweb["appliesTo"].as_array().unwrap().len(), 2, "every project");
            assert_eq!(everyweb["egress"], 1);
            let deny = row("deny");
            assert_eq!(deny["appliesTo"].as_array().unwrap().len(), 2, "an empty selector is the whole project");
            assert_eq!(deny["types"], json!(["Ingress"]));
            let e = endpoints(&s, None);
            assert_eq!(e[0]["from"], word);
            assert_eq!((e[0]["kind"].as_str(), e[0]["href"].as_str(), e[0]["node"].as_str()), (Some("Pod"), Some("#/pod/shop/web-1"), Some("n1")));
            assert_eq!(e[0]["ips"], json!(["10.1.0.5", "fd00::5"]));
            assert_eq!((e[0]["ingress"].as_bool(), e[0]["egress"].as_bool(), e[0]["ready"].as_bool()), (Some(true), Some(false), Some(true)));
            assert_eq!(e[0]["identity"], 7);
            assert!(e[0]["identityLabels"].as_str().unwrap().contains("app=web"));
        }
    }

    #[test]
    fn a_machines_endpoint_and_what_a_viewer_may_not_see() {
        let s = snap(&[
            pods(),
            ("cep", vec![("lab/vm1", json!({"status": {"state": "regenerating", "networking": {"node": "10.0.0.2"}}})), ("shop/web-1", json!({"status": {}}))]),
            ("cnp", vec![("other/theirs", json!({"spec": {"endpointSelector": {}}}))]),
            ("ccnp", vec![("all", json!({"spec": {"endpointSelector": {}}}))]),
        ]);
        let e = endpoints(&s, None);
        let vm = e.iter().find(|r| r["name"] == "vm1").unwrap();
        assert_eq!((vm["kind"].as_str(), vm["href"].as_str(), vm["node"].as_str(), vm["ready"].as_bool()), (Some("VM"), Some("#/vm/lab/vm1"), Some("10.0.0.2"), Some(false)));
        assert!(vm["ingress"].is_null(), "not said is not false");
        let hidden: HashSet<String> = ["other".to_string(), "lab".to_string()].into();
        assert!(endpoints(&s, Some(&hidden)).iter().all(|r| r["project"] == "shop"));
        let p = policies(&s, Some(&hidden));
        assert!(p.iter().all(|r| r["name"] != "theirs"));
        let all = p.iter().find(|r| r["name"] == "all").unwrap();
        assert_eq!((all["appliesTo"].as_array().unwrap().len(), all["withheld"].as_u64()), (2, Some(1)), "counted, not named");
    }

    #[test]
    fn a_selector_it_cannot_evaluate_is_said() {
        let s = snap(&[pods(), ("netpol", vec![("shop/x", json!({"spec": {"podSelector": {"matchExpressions": [{"key": "app", "operator": "In", "values": ["web", "db"]}]}}}))])]);
        let p = policies(&s, None);
        assert_eq!(p[0]["unevaluated"], true);
        assert_eq!(p[0]["selects"], json!(["app In web,db"]));
        assert_eq!(p[0]["appliesTo"].as_array().unwrap().len(), 0);
        assert_eq!(p[0]["from"], "kubernetes", "no network CRDs served");
        assert_eq!(network_of(&Snapshot::default()), "");
    }
}
