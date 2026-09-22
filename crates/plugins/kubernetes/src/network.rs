//! What the network knows, put where people already look (#17).
//!
//! Cilium knows which identity a workload has, whether its datapath is
//! actually programmed, and which policies select it. All of it was in the
//! console already — as five separate lists under Networking, which is not
//! where anybody is when they are asking why a pod cannot be reached.
//!
//! So the facts are attached to the pod, the node and the machine. Every
//! one of them comes from a CRD the plugin already watches, which matters
//! for a reason the issue names: the agent is a DaemonSet, every node has
//! one, and they can disagree. A CRD is the cluster's own record and says
//! the same thing to everybody — a view that silently showed one node's
//! answer for the whole cluster would be worse than one that showed none.
//!
//! What is *not* here, and why: flows, allowed and denied, are the answer
//! to "why can nothing reach this" and they come from Hubble, which is not
//! enabled in the image yet (stormpump#11, tracked on #4). Per-module
//! agent health is the agent's own REST API on the node, the same gate.

use std::collections::HashMap;

use console_core::{ComponentSummary, Health, Metric, Relation};
use serde_json::Value;

use crate::components::Snapshot;

/// An identity number means nothing on its own — 12345 is not an answer to
/// any question. What it resolves to is: the labels Cilium decided this
/// workload *is*, which is what every policy is written against.
pub fn identity_labels(cid: &Value) -> String {
    let Some(labels) = cid.get("security-labels").and_then(Value::as_object) else {
        return String::new();
    };
    let mut parts: Vec<String> = labels
        .iter()
        .filter(|(k, _)| {
            // The three Cilium adds to every identity say nothing about
            // this one, and they crowd out the labels somebody chose.
            !matches!(
                k.as_str(),
                "k8s:io.kubernetes.pod.namespace"
                    | "k8s:io.cilium.k8s.policy.cluster"
                    | "k8s:io.cilium.k8s.policy.serviceaccount"
            )
        })
        .map(|(k, v)| {
            let k = k.strip_prefix("k8s:").unwrap_or(k);
            match v.as_str().filter(|s| !s.is_empty()) {
                Some(v) => format!("{k}={v}"),
                None => k.to_string(),
            }
        })
        .collect();
    parts.sort();
    parts.join(" ")
}

/// Which policies select one workload.
///
/// Not derivable from the pod: a policy names a label selector and the
/// answer is whatever that selector matches, so it has to be evaluated
/// rather than read off. Evaluated here against the labels Cilium itself
/// decided the workload has — the identity — because those are the labels
/// policy is actually applied to, and a match computed against the pod's
/// own labels would differ from the datapath's on exactly the workloads
/// where it matters.
pub fn selecting(snap: &Snapshot, ns: &str, identity: Option<&Value>) -> Vec<String> {
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);
    let labels: HashMap<String, String> = identity
        .and_then(|c| c.get("security-labels"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    (
                        k.strip_prefix("k8s:").unwrap_or(k).to_string(),
                        v.as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    for (kind, id_prefix) in [("cnp", "k8s:cnp:"), ("ccnp", "k8s:ccnp:"), ("netpol", "k8s:netpol:")]
    {
        for (key, obj) in of(kind) {
            // A namespaced policy only ever selects in its own namespace.
            if kind != "ccnp" {
                let Some((pns, _)) = key.split_once('/') else { continue };
                if pns != ns {
                    continue;
                }
            }
            if selects(obj, &labels) {
                out.push(format!("{id_prefix}{key}"));
            }
        }
    }
    out.sort();
    out
}

/// Does this policy's endpoint selector match these labels?
///
/// `matchLabels` only. `matchExpressions` exists and is not evaluated
/// here: a half-evaluated selector would silently claim a policy applies
/// when it does not, which is worse than the honest gap — so a policy
/// carrying one is reported as selecting nothing rather than guessed at.
fn selects(policy: &Value, labels: &HashMap<String, String>) -> bool {
    let selectors = [
        policy.pointer("/spec/endpointSelector"),
        policy.pointer("/spec/podSelector"),
        policy.pointer("/spec/nodeSelector"),
    ];
    for sel in selectors.into_iter().flatten() {
        if sel.get("matchExpressions").is_some() {
            return false;
        }
        let Some(m) = sel.get("matchLabels").and_then(Value::as_object) else {
            // An empty selector selects everything in scope, which is what
            // a default-deny policy is.
            return sel.as_object().is_some_and(|o| o.is_empty());
        };
        return m.iter().all(|(k, v)| {
            let k = k.strip_prefix("k8s:").unwrap_or(k);
            labels.get(k).map(String::as_str) == v.as_str()
        });
    }
    false
}

/// Attach the network's view of one workload to its component.
///
/// `key` is `ns/name` — the same key the endpoint, the pod and the machine
/// are all filed under, which is what makes this work for a VM as well as
/// a pod without either plugin knowing about the other.
pub fn describe(snap: &Snapshot, key: &str, c: &mut ComponentSummary) {
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);
    let Some(cep) = of("cep").get(key) else { return };
    let Some((ns, _)) = key.split_once('/') else { return };

    // Whether the datapath is actually programmed for it. A pod can be
    // Running with an endpoint that is still regenerating, and during that
    // window nothing reaches it — which presents as a pod that is fine and
    // a service that is broken.
    let state = cep.pointer("/status/state").and_then(Value::as_str).unwrap_or("");
    if !state.is_empty() {
        c.metrics.push(Metric::new("datapath", state).tone(match state {
            "ready" => "ok",
            "waiting-for-identity" | "waiting-to-regenerate" | "regenerating" | "restoring"
            | "creating" => "warn",
            _ => "error",
        }));
    }

    let identity = cep.pointer("/status/identity/id").and_then(Value::as_i64);
    if let Some(id) = identity {
        let cid = of("cid").get(&id.to_string());
        // The number, and what it resolves to. The number alone is what
        // the endpoint list already showed, and it answers nothing.
        let labels = cid.map(identity_labels).unwrap_or_default();
        c.metrics.push(
            Metric::new("identity", if labels.is_empty() { id.to_string() } else { labels })
                .tone("muted"),
        );
        if cid.is_some() {
            c.relations.push(Relation::belongs_to("identity", format!("k8s:cid:{id}")));
        }
        let policies = selecting(snap, ns, cid);
        // No policy selecting a workload is an answer, not an absence:
        // on a cluster with a default-deny it means nothing reaches this,
        // and on one without it means everything does.
        c.metrics.push(
            Metric::new("policies", policies.len().to_string())
                .tone(if policies.is_empty() { "muted" } else { "accent" }),
        );
        if !policies.is_empty() {
            // Context, not containment: a pod does not contain the
            // policies that select it (#18).
            c.relations.push(Relation::belongs_to("policy", policies));
        }
    }
    c.relations.push(Relation::belongs_to("endpoint", format!("k8s:cep:{key}")));
}

/// Addresses left in a node's pod CIDR.
///
/// "No addresses" is a failure that presents as pods stuck Pending with a
/// message about nothing in particular, and the number that would have
/// predicted it is sitting in the CiliumNode the whole time.
pub fn ipam(cn: &Value) -> Option<(usize, usize)> {
    let pool = cn.pointer("/spec/ipam/pool").and_then(Value::as_object)?;
    let used = cn
        .pointer("/status/ipam/used")
        .and_then(Value::as_object)
        .map(serde_json::Map::len)
        .unwrap_or(0);
    Some((used, pool.len()))
}

/// The IPAM numbers as a metric, warning before it is too late to act.
pub fn ipam_metric(cn: &Value, c: &mut ComponentSummary) {
    let Some((used, total)) = ipam(cn) else { return };
    if total == 0 {
        return;
    }
    let free = total.saturating_sub(used);
    // A tenth left is the last point at which somebody can do something
    // about it; none left is a cluster that has already stopped
    // scheduling pods onto this node.
    let tone = if free == 0 {
        "error"
    } else if free * 10 <= total {
        "warn"
    } else {
        "ok"
    };
    c.metrics.push(Metric::new("addresses", format!("{free} free of {total}")).tone(tone));
    if free == 0 && c.health == Health::Ok {
        c.health = Health::Warn;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap(pairs: Vec<(&'static str, &str, Value)>) -> Snapshot {
        let mut m: Snapshot = HashMap::new();
        for (kind, key, obj) in pairs {
            m.entry(kind).or_default().insert(key.to_string(), obj);
        }
        m
    }

    #[test]
    fn an_identity_resolves_to_the_labels_policy_is_written_against() {
        let cid = json!({"security-labels": {
            "k8s:app": "web",
            "k8s:io.kubernetes.pod.namespace": "default",
            "k8s:io.cilium.k8s.policy.cluster": "default",
            "k8s:tier": "frontend"
        }});
        // The namespace and Cilium's own bookkeeping are dropped: they say
        // nothing about *this* identity and crowd out what somebody chose.
        assert_eq!(identity_labels(&cid), "app=web tier=frontend");
    }

    #[test]
    fn a_pod_carries_what_the_network_knows_about_it() {
        let sn = snap(vec![
            (
                "cep",
                "default/web-1",
                json!({"status": {"state": "ready", "identity": {"id": 12345}}}),
            ),
            ("cid", "12345", json!({"security-labels": {"k8s:app": "web"}})),
            (
                "cnp",
                "default/allow-web",
                json!({"spec": {"endpointSelector": {"matchLabels": {"app": "web"}}}}),
            ),
            (
                "cnp",
                "default/allow-db",
                json!({"spec": {"endpointSelector": {"matchLabels": {"app": "db"}}}}),
            ),
            // Another namespace's policy never selects here.
            (
                "cnp",
                "other/allow-web",
                json!({"spec": {"endpointSelector": {"matchLabels": {"app": "web"}}}}),
            ),
        ]);
        let mut c = ComponentSummary {
            id: "k8s:pod:default/web-1".into(),
            kind: "pod".into(),
            label: "web-1".into(),
            health: Health::Ok,
            detail: String::new(),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        };
        describe(&sn, "default/web-1", &mut c);
        let metric = |l: &str| c.metrics.iter().find(|m| m.label == l).map(|m| m.value.clone());
        assert_eq!(metric("datapath"), Some("ready".into()));
        assert_eq!(metric("identity"), Some("app=web".into()), "the number answers nothing");
        assert_eq!(metric("policies"), Some("1".into()));
        let policy = c.relations.iter().find(|r| r.name == "policy").unwrap();
        assert_eq!(policy.targets, vec!["k8s:cnp:default/allow-web"]);
        assert!(c.relations.iter().any(|r| r.name == "endpoint"));
    }

    /// A pod with no endpoint gets nothing rather than an empty network
    /// section: a cluster without Cilium is not a cluster with a broken
    /// datapath.
    #[test]
    fn no_endpoint_means_nothing_is_said() {
        let sn = snap(vec![]);
        let mut c = ComponentSummary {
            id: "k8s:pod:default/web-1".into(),
            kind: "pod".into(),
            label: "web-1".into(),
            health: Health::Ok,
            detail: String::new(),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        };
        describe(&sn, "default/web-1", &mut c);
        assert!(c.metrics.is_empty() && c.relations.is_empty());
    }

    /// A selector this code cannot evaluate reports nothing rather than a
    /// guess: claiming a policy applies when it does not is worse than an
    /// honest gap.
    #[test]
    fn a_match_expression_is_not_guessed_at() {
        let policy = json!({"spec": {"endpointSelector": {
            "matchExpressions": [{"key": "app", "operator": "In", "values": ["web"]}]
        }}});
        assert!(!selects(&policy, &HashMap::from([("app".into(), "web".into())])));
        // An empty selector is a default-deny and selects everything.
        let all = json!({"spec": {"endpointSelector": {}}});
        assert!(selects(&all, &HashMap::new()));
    }

    #[test]
    fn a_node_says_how_many_addresses_are_left() {
        let cn = json!({
            "spec": {"ipam": {"pool": {"10.0.0.1": {}, "10.0.0.2": {}, "10.0.0.3": {},
                                       "10.0.0.4": {}, "10.0.0.5": {}, "10.0.0.6": {},
                                       "10.0.0.7": {}, "10.0.0.8": {}, "10.0.0.9": {},
                                       "10.0.0.10": {}}}},
            "status": {"ipam": {"used": {"10.0.0.1": {}, "10.0.0.2": {}}}}
        });
        assert_eq!(ipam(&cn), Some((2, 10)));
        let mut c = ComponentSummary {
            id: "k8s:cn:n1".into(),
            kind: "cn".into(),
            label: "n1".into(),
            health: Health::Ok,
            detail: String::new(),
            metrics: vec![],
            actions: vec![],
            relations: vec![],
            link: None,
        };
        ipam_metric(&cn, &mut c);
        let m = c.metrics.iter().find(|m| m.label == "addresses").unwrap();
        assert_eq!(m.value, "8 free of 10");
        assert_eq!(m.tone.as_deref(), Some("ok"));
    }
}
