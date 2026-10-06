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

use console_core::{ComponentSummary, Health, Metric, Relation, RelationKind};
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
    selecting_labels(snap, ns, &labels)
}

/// Which policies select a workload with these labels — for something
/// Cilium has no identity for, such as a machine outside the pod network
/// (#51), where the question is which policies *would* apply once it is on
/// it. Keys without the `k8s:` prefix.
pub fn selecting_labels(snap: &Snapshot, ns: &str, labels: &HashMap<String, String>) -> Vec<String> {
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);
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
            if selects(obj, labels) {
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
            // policies that select it (#18) — but there are several of
            // them, which no `Relation` constructor builds, since a
            // reference edge is usually to one thing.
            c.relations.push(Relation {
                name: "policy".into(),
                kind: RelationKind::BelongsTo,
                targets: policies,
                href: None,
            });
        }
    }
    c.relations.push(Relation::belongs_to("endpoint", format!("k8s:cep:{key}")));
}

/// Why a running machine is outside network policy, or `None` when it is
/// inside it (#51).
///
/// Policy — every NetworkPolicy, every CiliumNetworkPolicy, and so a
/// project's isolation — is enforced by Cilium on its endpoints. A machine
/// the node runs as a NAT inside the hypervisor (`storm.io/binding: user`,
/// what a pod-network spec gets today, stormvm#16) or as a tap on a host
/// bridge is not one: its packets never pass through Cilium, and a page
/// listing the policies that "select" it is describing a fence that is not
/// there. So the binding the node reported decides first; and where
/// Cilium's endpoints are watched (`ceps` is `Some`), a machine with no
/// endpoint under its `ns/name` is outside too, whatever its binding.
///
/// One interface outside is enough: traffic leaves by that one.
pub fn outside_policy(vmi: &Value, ceps: Option<&HashMap<String, Value>>) -> Option<Outside> {
    let ifs = vmi.pointer("/status/interfaces").and_then(Value::as_array);
    for i in ifs.into_iter().flatten() {
        let name = i.get("name").and_then(Value::as_str).unwrap_or("?");
        match i.get("storm.io/binding").and_then(Value::as_str) {
            Some("user") => {
                return Some(Outside {
                    why: "nat",
                    sentence: format!(
                        "interface {name} is a NAT inside the hypervisor, not a Cilium endpoint \
                         (stormvm#16): no network policy and no project isolation reaches it"
                    ),
                })
            }
            Some("bridge") => {
                return Some(Outside {
                    why: "bridge",
                    sentence: format!(
                        "interface {name} is on a host bridge, outside the pod network: no network \
                         policy and no project isolation reaches it"
                    ),
                })
            }
            _ => {}
        }
    }
    let ns = vmi.pointer("/metadata/namespace").and_then(Value::as_str)?;
    let name = vmi.pointer("/metadata/name").and_then(Value::as_str)?;
    if let Some(ceps) = ceps {
        if !ceps.contains_key(&format!("{ns}/{name}")) {
            return Some(Outside {
                why: "no-endpoint",
                sentence: "Cilium has no endpoint for it, so no network policy and no project \
                           isolation is applied to its traffic"
                    .into(),
            });
        }
    }
    None
}

/// A machine outside policy: `why` is `nat`, `bridge` or `no-endpoint`.
#[derive(Debug, Clone, PartialEq)]
pub struct Outside {
    pub why: &'static str,
    pub sentence: String,
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

    fn vmi(ifs: Value) -> Value {
        json!({"metadata": {"namespace": "shop", "name": "vm1"},
               "status": {"phase": "Running", "interfaces": ifs}})
    }

    /// The security claim (#51): a NAT'd machine is outside every policy,
    /// whether or not anything selects its labels.
    #[test]
    fn a_machine_behind_the_hypervisors_nat_is_outside_policy() {
        let o = outside_policy(&vmi(json!([{"name": "default", "storm.io/binding": "user"}])), None).unwrap();
        assert_eq!(o.why, "nat");
        assert!(o.sentence.contains("stormvm#16"), "{}", o.sentence);
        // Even with an endpoint recorded under its name: the binding wins.
        let ceps = HashMap::from([("shop/vm1".to_string(), json!({}))]);
        assert_eq!(outside_policy(&vmi(json!([{"name": "default", "storm.io/binding": "user"}])), Some(&ceps)).unwrap().why, "nat");
        let o = outside_policy(&vmi(json!([{"name": "lan", "storm.io/binding": "bridge"}])), None).unwrap();
        assert_eq!(o.why, "bridge");
    }

    #[test]
    fn a_machine_on_the_pod_network_is_inside_only_with_an_endpoint() {
        let passt = vmi(json!([{"name": "default", "storm.io/binding": "passt"}]));
        // Nothing watched to say otherwise: no claim either way is invented.
        assert!(outside_policy(&passt, None).is_none());
        let ceps = HashMap::from([("shop/vm1".to_string(), json!({}))]);
        assert!(outside_policy(&passt, Some(&ceps)).is_none());
        let none = HashMap::new();
        assert_eq!(outside_policy(&passt, Some(&none)).unwrap().why, "no-endpoint");
    }

    #[test]
    fn policies_that_would_select_a_machine_are_found_from_its_labels() {
        let sn = snap(vec![
            ("netpol", "shop/storm-isolate", json!({"spec": {"podSelector": {}}})),
            ("cnp", "shop/web", json!({"spec": {"endpointSelector": {"matchLabels": {"app": "web"}}}})),
            ("cnp", "shop/db", json!({"spec": {"endpointSelector": {"matchLabels": {"app": "db"}}}})),
        ]);
        let labels = HashMap::from([("app".to_string(), "web".to_string())]);
        assert_eq!(
            selecting_labels(&sn, "shop", &labels),
            vec!["k8s:cnp:shop/web", "k8s:netpol:shop/storm-isolate"]
        );
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
