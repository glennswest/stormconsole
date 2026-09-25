//! A machine's interfaces: what was asked for, and what the node did (#24).
//!
//! The two disagree today, and the disagreement is the whole story. A spec
//! saying `networks: [{name: default, pod: {}}]` asks for the pod network;
//! the node renders it as qemu's user-mode stack, a NAT inside the
//! hypervisor process that nothing outside it can route to (stormvm#16).
//! Showing only the spec reads as a working pod network. Showing only the
//! status loses what somebody meant. So each interface carries both, and a
//! verdict on whether the address it holds is one anybody can use.
//!
//! What was asked is read the way stormvm reads it (`stormvm-spec`
//! `kube.rs`): the `storm.io/bridge.<iface>` annotation, then
//! `storm.io/bridge`, win over the network; otherwise the network of the
//! same name — `pod` with the binding spelled on the interface
//! (`masquerade`, the default, `bridge`, `passt`) or a `multus` network.
//!
//! What the node did is `status.interfaces[]` as rustkube-node writes it:
//! `mac`, `ipAddress`, `ipAddresses` (from the guest agent, or the node's
//! neighbour table for a guest without one) and `storm.io/binding`, which
//! is `bridge` (a tap on a real bridge), `user` (the NAT) or `passt`.

use serde_json::{json, Value};

/// What an interface was asked to be attached to.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    /// `pod`, `bridge`, `multus`, or empty when the spec names nothing.
    pub network: String,
    /// The bridge or multus network's name; empty for the pod network.
    pub target: String,
    /// For the pod network, how the address reaches the guest.
    pub binding: String,
}

impl Asked {
    pub fn label(&self) -> String {
        match self.network.as_str() {
            "pod" => format!("pod network ({})", self.binding),
            "bridge" => format!("host bridge {}", self.target),
            "multus" => format!("network {}", self.target),
            _ => "nothing".into(),
        }
    }
}

fn annotation<'a>(metas: &[Option<&'a Value>], key: &str) -> Option<&'a str> {
    metas
        .iter()
        .flatten()
        .find_map(|m| m.pointer("/annotations").and_then(|a| a.get(key)).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
}

/// What each interface of `spec` (a VMI spec, or a VM's template spec) was
/// asked for, in the spec's order. `metas` are the metadata blocks whose
/// annotations apply, most specific first: the template's, then the
/// object's.
pub fn asked(spec: &Value, metas: &[Option<&Value>]) -> Vec<(String, Asked)> {
    let networks = spec.pointer("/networks").and_then(Value::as_array);
    let mut out = Vec::new();
    for i in spec.pointer("/domain/devices/interfaces").and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = i.get("name").and_then(Value::as_str) else { continue };
        let net = networks.and_then(|ns| ns.iter().find(|n| n.get("name").and_then(Value::as_str) == Some(name)));
        let bridge = annotation(metas, &format!("storm.io/bridge.{name}")).or_else(|| annotation(metas, "storm.io/bridge"));
        let a = if let Some(b) = bridge {
            Asked { network: "bridge".into(), target: b.into(), binding: String::new() }
        } else if net.is_some_and(|n| n.get("pod").is_some_and(|p| !p.is_null())) {
            let binding = ["bridge", "passt", "masquerade"]
                .into_iter()
                .find(|k| i.get(*k).is_some_and(|v| !v.is_null()))
                .unwrap_or("masquerade");
            Asked { network: "pod".into(), target: String::new(), binding: binding.into() }
        } else if let Some(m) = net.and_then(|n| n.pointer("/multus/networkName")).and_then(Value::as_str) {
            Asked { network: "multus".into(), target: m.into(), binding: String::new() }
        } else {
            Asked { network: String::new(), target: String::new(), binding: String::new() }
        };
        out.push((name.to_string(), a));
    }
    out
}

/// Every address an interface's status reports, `ipAddresses` first and
/// the singular upstream field folded in, without blanks or repeats.
pub fn addresses(status: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let many = status.get("ipAddresses").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
    for a in many.chain(status.get("ipAddress").and_then(Value::as_str)) {
        if !a.is_empty() && !out.iter().any(|x| x == a) {
            out.push(a.to_string());
        }
    }
    out
}

/// Whether an address anybody else can use is on this interface, and why.
fn verdict(asked: Option<&Asked>, running: bool, status: Option<&Value>) -> (&'static str, String) {
    if !running {
        return ("stopped", "not running, so no address".into());
    }
    let Some(st) = status else {
        return ("pending", "the node has not reported this interface yet".into());
    };
    let binding = st.get("storm.io/binding").and_then(Value::as_str).unwrap_or("");
    let addrs = addresses(st);
    let asked_pod = asked.is_some_and(|a| a.network == "pod");
    match binding {
        "user" => (
            "nat",
            if asked_pod {
                "asked for the pod network, and this node runs it as a NAT inside the hypervisor \
                 (stormvm#16): the guest's address is reachable from nothing outside the machine"
                    .into()
            } else {
                "behind a NAT inside the hypervisor: the guest's address is reachable from \
                 nothing outside the machine"
                    .into()
            },
        ),
        _ if addrs.is_empty() => (
            "none",
            "no address yet. It comes from the guest agent, or from the node's neighbour table \
             once the guest has spoken on the network"
                .into(),
        ),
        "bridge" => (
            "reachable",
            match asked.filter(|a| a.network == "bridge") {
                Some(a) => format!("on host bridge {}: reachable from that segment", a.target),
                None => "on a bridge: reachable from that segment".into(),
            },
        ),
        "passt" => ("reachable", "on the pod network".into()),
        _ => ("unknown", "the node did not say how this interface is attached".into()),
    }
}

/// One row per interface: the spec's interfaces in order, then any the
/// status reports that the spec does not name. `spec` is what the machine
/// was asked to be; `instance` the running VMI, if there is one.
pub fn interfaces(spec: &Value, metas: &[Option<&Value>], instance: Option<&Value>) -> Vec<Value> {
    let running = instance.is_some();
    let status: Vec<&Value> = instance
        .and_then(|v| v.pointer("/status/interfaces"))
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let by_name = |n: &str| status.iter().copied().find(|s| s.get("name").and_then(Value::as_str) == Some(n));
    let asked = asked(spec, metas);
    let mut names: Vec<String> = asked.iter().map(|(n, _)| n.clone()).collect();
    for s in &status {
        if let Some(n) = s.get("name").and_then(Value::as_str) {
            if !names.iter().any(|x| x == n) {
                names.push(n.to_string());
            }
        }
    }
    names
        .into_iter()
        .map(|name| {
            let a = asked.iter().find(|(n, _)| *n == name).map(|(_, a)| a);
            let st = by_name(&name);
            let (reach, note) = verdict(a, running, st);
            json!({
                "name": name,
                "asked": a.map(Asked::label),
                "askedNetwork": a.map(|a| a.network.clone()),
                "did": st.and_then(|s| s.get("storm.io/binding")).and_then(Value::as_str),
                "mac": st.and_then(|s| s.get("mac")).and_then(Value::as_str)
                    .or_else(|| spec.pointer("/domain/devices/interfaces").and_then(Value::as_array)
                        .and_then(|is| is.iter().find(|i| i.get("name").and_then(Value::as_str) == Some(&name)))
                        .and_then(|i| i.get("macAddress")).and_then(Value::as_str)),
                "addresses": st.map(addresses).unwrap_or_default(),
                "reach": reach,
                "note": note,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vmi(annotations: Value, iface: Value, status: Value) -> Value {
        json!({
            "metadata": {"annotations": annotations},
            "spec": {
                "domain": {"devices": {"interfaces": [iface]}},
                "networks": [{"name": "default", "pod": {}}]
            },
            "status": {"phase": "Running", "interfaces": status}
        })
    }

    fn one(v: &Value) -> Value {
        interfaces(&v["spec"], &[v.get("metadata")], Some(v)).remove(0)
    }

    #[test]
    fn a_pod_spec_run_as_a_nat_never_reads_as_a_pod_network() {
        let v = vmi(
            json!({}),
            json!({"name": "default"}),
            json!([{"name": "default", "mac": "52:54:00:a1:8d:d0", "ipAddress": "10.155.0.15",
                    "ipAddresses": ["10.155.0.15"], "storm.io/binding": "user"}]),
        );
        let i = one(&v);
        assert_eq!(i["asked"], "pod network (masquerade)");
        assert_eq!(i["did"], "user");
        assert_eq!(i["reach"], "nat");
        assert!(i["note"].as_str().unwrap().contains("stormvm#16"));
        assert_eq!(i["addresses"], json!(["10.155.0.15"]), "the address is shown, and marked, not hidden");
    }

    #[test]
    fn a_bridge_annotation_wins_and_its_address_is_reachable() {
        let v = vmi(
            json!({"storm.io/bridge": "stormbr0"}),
            json!({"name": "default"}),
            json!([{"name": "default", "mac": "52:54:00:00:00:01", "ipAddress": "192.168.8.61",
                    "ipAddresses": ["192.168.8.61", "fd00::61"], "storm.io/binding": "bridge"}]),
        );
        let i = one(&v);
        assert_eq!(i["asked"], "host bridge stormbr0");
        assert_eq!(i["reach"], "reachable");
        assert_eq!(i["addresses"], json!(["192.168.8.61", "fd00::61"]), "v4 and v6, neither is 'the' address");
        assert_eq!(i["mac"], "52:54:00:00:00:01");
    }

    #[test]
    fn a_per_interface_bridge_beats_the_machine_wide_one() {
        let v = vmi(
            json!({"storm.io/bridge": "stormbr0", "storm.io/bridge.default": "vlan20"}),
            json!({"name": "default"}),
            json!([]),
        );
        assert_eq!(one(&v)["asked"], "host bridge vlan20");
    }

    #[test]
    fn running_without_an_address_says_no_address_yet() {
        let v = vmi(
            json!({"storm.io/bridge": "stormbr0"}),
            json!({"name": "default"}),
            json!([{"name": "default", "mac": "52:54:00:00:00:01", "ipAddress": "", "ipAddresses": [],
                    "storm.io/binding": "bridge"}]),
        );
        let i = one(&v);
        assert_eq!(i["reach"], "none");
        assert!(i["note"].as_str().unwrap().starts_with("no address yet"));
    }

    #[test]
    fn an_unreported_interface_is_pending_and_a_stopped_one_is_stopped() {
        let v = vmi(json!({}), json!({"name": "default", "bridge": {}}), Value::Null);
        let i = one(&v);
        assert_eq!(i["asked"], "pod network (bridge)");
        assert_eq!(i["reach"], "pending");
        let stopped = interfaces(&v["spec"], &[], None).remove(0);
        assert_eq!(stopped["reach"], "stopped");
        assert_eq!(stopped["did"], Value::Null);
    }

    #[test]
    fn an_interface_only_the_status_knows_is_still_listed() {
        let v = vmi(
            json!({}),
            json!({"name": "default"}),
            json!([{"name": "default", "storm.io/binding": "user"},
                   {"name": "net1", "ipAddress": "10.1.0.4", "storm.io/binding": "bridge"}]),
        );
        let all = interfaces(&v["spec"], &[v.get("metadata")], Some(&v));
        assert_eq!(all.len(), 2);
        assert_eq!(all[1]["name"], "net1");
        assert_eq!(all[1]["asked"], Value::Null);
        assert_eq!(all[1]["reach"], "reachable");
    }

    #[test]
    fn a_multus_network_is_named() {
        let spec = json!({
            "domain": {"devices": {"interfaces": [{"name": "data", "bridge": {}}]}},
            "networks": [{"name": "data", "multus": {"networkName": "vlan30"}}]
        });
        assert_eq!(asked(&spec, &[])[0].1.label(), "network vlan30");
    }
}
