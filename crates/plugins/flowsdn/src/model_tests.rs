use super::*;
use serde_json::json;

#[test]
fn a_verdict_is_joined_to_its_endpoint_and_judged_by_it() {
    // flowsdn-agent `api::verdict_list`.
    let shop = Endpoint { id: 7, namespace: "shop".into(), pod: "web".into(), ..Default::default() };
    let row = |e: Value| json!({"endpoint": e, "direction": "ingress", "verdict": "denied", "protocol": 6, "port": 5432, "packets": 3, "bytes": 180});
    let eps = [shop];
    let v = Verdict::from_agent(&row(json!(7)), &eps);
    assert_eq!((v.endpoint, v.namespace.as_str(), v.pod.as_str(), v.protocol.as_str(), v.port, v.packets), (Some(7), "shop", "web", "TCP", 5432, 3));
    let host = Verdict::from_agent(&row(json!("host")), &eps);
    assert_eq!(host.endpoint, None);
    let gone = Verdict::from_agent(&row(json!(99)), &eps);
    assert_eq!((gone.endpoint, gone.namespace.as_str()), (Some(99), ""));

    let hidden = std::collections::HashSet::from(["shop".to_string()]);
    assert!(!v.shown_to(&hidden), "another project's");
    assert!(host.shown_to(&hidden), "the node's own is no project's");
    assert!(!gone.shown_to(&hidden), "an endpoint not known could be anyone's");
    assert!(v.shown_to(&Default::default()) && !gone.shown_to(&std::collections::HashSet::from(["x".to_string()])));
    let lab = Verdict::from_agent(&row(json!(7)), &[Endpoint { id: 7, namespace: "lab".into(), ..Default::default() }]);
    assert!(lab.shown_to(&hidden));
}

/// The endpoint flowsdn `docs/agent-api.md` documents, Kubernetes mode.
pub fn endpoint_k8s() -> Value {
    json!({
      "id": 42,
      "status": {
        "state": "ready",
        "external-identifiers": {
          "k8s-pod-name": "example-pod", "k8s-namespace": "default", "k8s-uid": "pod-uid",
          "container-id": "sandbox-id", "pod-name": "default/example-pod", "cni-attachment-id": "sandbox-id:eth0"
        },
        "pod": {
          "ID": 42, "namespace": "default", "pod_name": "example-pod", "pod_uid": "pod-uid",
          "container_id": "sandbox-id", "node_name": "node-a",
          "labels": ["k8s:app=web", "k8s:io.kubernetes.pod.namespace=default"],
          "workloads": [{"name": "web", "kind": "Deployment"}],
          "containers": [{"name": "app", "container-id": "containerd://1", "init": false},
                         {"name": "setup", "container-id": "containerd://0", "init": true}]
        },
        "pod-networks": {"default": {
          "role": "primary", "interface": "eth0", "mac_address": "02:00:00:00:00:07",
          "ip_addresses": ["10.5.0.7/32", "f00d::a05:0:0:7/128"],
          "gateway_ips": ["10.5.0.1", "f00d::a05:0:0:1"],
          "host_interface": "lxc0042", "endpoint_id": 42, "sandbox": "sandbox-id", "node": "node-a"}},
        "networking": {
          "interface-name": "lxc-example", "interface-index": 12, "container-interface-name": "eth0",
          "netns-cookie": "123456",
          "addressing": [{"ipv4": "10.0.0.2", "ipv4-pool-name": "default"}, {"ipv6": "f00d::2"}]
        },
        "identity": {"id": 31337, "labels": ["k8s:app=web"]},
        "some-new-field": true
      }
    })
}

#[test]
fn an_endpoint_leads_with_its_pod_and_keeps_every_address() {
    let e = Endpoint::from_agent(&endpoint_k8s()).unwrap();
    assert_eq!(e.name(), "default/example-pod");
    assert_eq!((e.id, e.node.as_str(), e.state.as_str(), e.ready), (42, "node-a", "ready", true));
    assert_eq!(e.ipv4, vec!["10.0.0.2"]);
    assert_eq!(e.ipv6, vec!["f00d::2"]);
    assert_eq!(e.identity, Some(31337));
    assert_eq!((e.interface.as_str(), e.container_interface.as_str()), ("lxc-example", "eth0"));
    assert_eq!(e.mac, "02:00:00:00:00:07");
    assert_eq!(e.gateways, vec!["10.5.0.1", "f00d::a05:0:0:1"]);
    assert_eq!(e.workloads, vec!["Deployment web"]);
    assert_eq!(e.containers, vec!["app", "setup (init)"]);
    assert_eq!(e.attachment, "sandbox-id:eth0");
}

#[test]
fn a_standalone_endpoint_names_itself_from_what_the_cni_supplied() {
    // No pod view, no node, no identity: the retained CNI facts only.
    let v = json!({"id": 7, "status": {"state": "ready",
        "external-identifiers": {"k8s-pod-name": "w", "k8s-namespace": "lab", "cni-attachment-id": "abc:eth0"},
        "pod": null,
        "pod-networks": {"default": {"ip_addresses": ["10.5.0.7/32"], "interface": "eth0"}}}});
    let e = Endpoint::from_agent(&v).unwrap();
    assert_eq!(e.name(), "lab/w");
    assert_eq!(e.node, "");
    assert_eq!(e.identity, None);
    // Addresses from pod-networks, prefix length dropped.
    assert_eq!(e.ipv4, vec!["10.5.0.7"]);

    let bare = Endpoint::from_agent(&json!({"id": 9, "status": {"state": "disconnecting",
        "external-identifiers": {"cni-attachment-id": "s:eth0"}}})).unwrap();
    assert_eq!((bare.name(), bare.ready), ("s:eth0".to_string(), false));
    assert_eq!(Endpoint::from_agent(&json!({"id": 1})).unwrap().name(), "endpoint 1");
    assert!(Endpoint::from_agent(&json!({"status": {}})).is_none(), "no id, no row");
}

#[test]
fn a_numeric_identity_reads_too() {
    let mut v = endpoint_k8s();
    v["status"]["identity"] = json!(4242);
    assert_eq!(Endpoint::from_agent(&v).unwrap().identity, Some(4242));
}

#[test]
fn pool_counts_are_strings_and_stay_exact_past_u64() {
    let p = Pool::from_agent(&json!({"pool": "default", "family": "ipv4", "cidr": "10.0.0.0/24",
        "capacity": "254", "allocated": "2", "excluded": "1", "allocated-excluded": "0", "available": "251"}));
    assert_eq!((p.capacity.as_str(), p.available.as_str(), p.allocated_excluded.as_str()), ("254", "251", "0"));
    assert_eq!(p.health(), Health::Ok);
    assert!((p.free_pct.unwrap() - 98.8).abs() < 0.1);

    // A /64: 2^64 - 2 addresses, more than a u64 or a JavaScript number holds.
    let v6 = Pool::from_agent(&json!({"pool": "default", "family": "ipv6", "cidr": "f00d::/64",
        "capacity": "18446744073709551614", "allocated": "3", "excluded": "0", "allocated-excluded": "0",
        "available": "18446744073709551611"}));
    assert_eq!(v6.available, "18446744073709551611");
    assert_eq!(v6.health(), Health::Ok);

    let low = Pool::from_agent(&json!({"capacity": "254", "available": "12"}));
    assert_eq!(low.health(), Health::Warn);
    let none = Pool::from_agent(&json!({"capacity": "254", "available": "0"}));
    assert_eq!(none.health(), Health::Error);
    assert_eq!(Pool::from_agent(&json!({})).health(), Health::Unknown);
}

/// `/v1/health/modules` as flowsdn writes it from 2ba23b6 (flowsdn#418,
/// docs/agent-api.md): `agent.api`, `agent.restore`, and with Kubernetes
/// `agent.kubernetes` (Degraded with the healthz message as Error while
/// lists are not synced) and `agent.policy` (Degraded: not enforced).
fn modules() -> Vec<Module> {
    [
        json!({"ID": {"Module": ["agent"], "Component": ["api"]}, "Level": "OK",
               "Message": "endpoint API listening: 3 endpoints", "Error": "", "LastOK": "2026-10-10T10:00:00Z",
               "Updated": "2026-10-10T10:00:00Z", "Stopped": "0001-01-01T00:00:00Z", "Final": "", "Count": 1}),
        json!({"ID": {"Module": ["agent"], "Component": ["kubernetes"]}, "Level": "Degraded",
               "Message": "Kubernetes lists not synced",
               "Error": "node-a: waiting for Pod and EndpointSlice lists", "LastOK": "0001-01-01T00:00:00Z",
               "Updated": "2026-10-10T10:00:00Z", "Stopped": "0001-01-01T00:00:00Z", "Final": "", "Count": 1}),
        json!({"ID": {"Module": ["agent"], "Component": ["policy"]}, "Level": "Degraded",
               "Message": "NetworkPolicy is not enforced", "Error": "policy maps could not be shared",
               "LastOK": "0001-01-01T00:00:00Z",
               "Updated": "2026-10-10T10:00:00Z", "Stopped": "0001-01-01T00:00:00Z", "Final": "", "Count": 1}),
    ]
    .iter()
    .map(Module::from_agent)
    .collect()
}

#[test]
fn modules_read_with_never_left_blank() {
    let m = modules();
    assert_eq!(m[0].id, "agent.api");
    assert_eq!((m[0].health, m[0].last_ok.as_str()), (Health::Ok, "2026-10-10T10:00:00Z"));
    assert_eq!(m[1].id, "agent.kubernetes");
    assert_eq!(m[1].last_ok, "", "0001-01-01 is the agent's never");
    assert_eq!(m[1].health, Health::Warn, "a degraded module is a warning: what flowsdn says, as it says it");
    assert_eq!(m[1].error, "node-a: waiting for Pod and EndpointSlice lists", "its Error as-is");
    assert_eq!((m[2].id.as_str(), m[2].health), ("agent.policy", Health::Warn));
    assert_eq!(m[2].message, "NetworkPolicy is not enforced", "its Message as-is");
}

#[test]
fn a_module_is_its_level_whatever_its_error_says() {
    // #355: the old Idle-on-"not implemented" exception is gone for good.
    let row = |level: &str, error: &str| {
        Module::from_agent(&json!({"ID": {"Module": ["agent"], "Component": ["x"]}, "Level": level, "Error": error})).health
    };
    assert_eq!(row("Degraded", "not implemented"), Health::Warn);
    assert_eq!(row("Stopped", "not implemented"), Health::Error);
    assert_eq!(row("OK", ""), Health::Ok);
    assert_eq!(row("", ""), Health::Unknown);
}

#[test]
fn the_agent_is_as_healthy_as_its_worst_part() {
    let ok = Healthz::from_agent(&json!({"agent": {"state": "Ok", "msg": "initial endpoint API ready"},
        "kubernetes": {"state": "Ok", "msg": "node-a: 3 nodes, 12 pods", "node-name": "node-a",
                       "auto-direct-node-routes": true, "service-lb": true}}));
    assert_eq!(ok.kubernetes.as_ref().unwrap().node, "node-a");
    // Every module well: the fixture's kubernetes and policy modules are degraded.
    let healthy: Vec<Module> = modules().into_iter().map(|m| Module { health: Health::Ok, ..m }).collect();
    assert_eq!(agent_health(&ok, &healthy), (Health::Ok, "node-a: 3 nodes, 12 pods".into()));

    let mut warn = ok.clone();
    warn.kubernetes.as_mut().unwrap().state = "Warning".into();
    warn.kubernetes.as_mut().unwrap().message = "annotations: 409".into();
    assert_eq!(agent_health(&warn, &healthy).0, Health::Warn);

    let mut degraded = healthy.clone();
    degraded[1].health = Health::Warn;
    assert_eq!(agent_health(&ok, &degraded).0, Health::Warn);
    degraded[0].health = Health::Error;
    assert_eq!(agent_health(&ok, &degraded).0, Health::Error);

    let standalone = Healthz::from_agent(&json!({"agent": {"state": "Ok", "msg": "initial endpoint API ready"}}));
    assert!(standalone.kubernetes.is_none());
    assert_eq!(agent_health(&standalone, &healthy), (Health::Ok, "initial endpoint API ready".into()));
}

#[test]
fn a_service_frontend_reads_as_address_and_backends() {
    let v = json!({"spec": {"id": 3,
        "frontend-address": {"ip": "10.96.0.10", "port": 53, "protocol": "UDP", "scope": "external"},
        "backend-addresses": [{"ip": "10.5.0.9", "port": 53, "protocol": "UDP", "state": "active"},
                              {"ip": "f00d::9", "port": 53, "protocol": "UDP", "state": "terminating"}],
        "flags": {"type": "ClusterIP", "name": "kube-dns", "namespace": "kube-system", "port-name": "dns"}},
        "status": {"realized": {"id": 3}}});
    let s = Service::from_agent(&v);
    assert_eq!((s.namespace.as_str(), s.name.as_str(), s.kind.as_str()), ("kube-system", "kube-dns", "ClusterIP"));
    assert_eq!(s.frontend, "10.96.0.10:53/UDP");
    assert_eq!(s.backends, vec!["10.5.0.9:53/UDP", "[f00d::9]:53/UDP (terminating)"]);
    assert!(s.realized);
    assert!(!Service::from_agent(&json!({"spec": {"id": 0}})).realized);
}

/// Records as flowsdn `docs/agent-api.md` "Flow record shape" (#485) writes them.
pub fn flow_records() -> Value {
    json!({"seen": 9, "kept": 4, "flows": [
      {"time": "2026-11-21T10:00:03Z", "verdict": "DROPPED", "drop_reason": "POLICY_DENIED", "traffic_direction": "EGRESS", "protocol": "TCP",
       "source": {"ID": 42, "identity": 31337, "namespace": "default", "pod_name": "example-pod", "node_name": "node-a",
                  "labels": ["k8s:app=web"], "workloads": [{"name": "web", "kind": "Deployment"}], "ip": "10.0.0.2", "port": 40112},
       "destination": {"identity": 4100, "namespace": "secret", "pod_name": "vault-0", "node_name": "node-b",
                       "labels": [], "workloads": [{"name": "vault", "kind": "StatefulSet"}], "ip": "10.6.0.4", "port": 8200}},
      {"time": "2026-11-21T10:00:02Z", "verdict": "FORWARDED", "traffic_direction": "INGRESS", "protocol": "TCP",
       "source": {"reserved": "world", "identity": 2, "ip": "203.0.113.9", "port": 51000},
       "destination": {"ID": 42, "identity": 31337, "namespace": "default", "pod_name": "example-pod", "node_name": "node-a",
                       "workloads": [], "ip": "10.0.0.2", "port": 8080}},
      {"time": "2026-11-21T10:00:01Z", "verdict": "AUDIT", "traffic_direction": "INGRESS", "protocol": "ICMPv4",
       "source": {"reserved": "remote-node", "node_name": "node-b", "ip": "192.168.8.11"},
       "destination": {"reserved": "host", "ip": "192.168.8.10"}},
      {"time": "2026-11-21T10:00:00Z", "verdict": "FORWARDED", "traffic_direction": "INGRESS", "protocol": "UDP",
       "source": {"identity": 16777217, "ip": "10.0.0.2", "port": 53},
       "destination": {"identity": 16777218, "ip": "198.51.100.1", "port": 53}}
    ]})
}

#[test]
fn a_flow_names_both_sides_by_pod_node_or_what_the_datapath_judged() {
    let eps = vec![Endpoint::from_agent(&endpoint_k8s()).unwrap()];
    let f: Vec<Flow> = flow_records()["flows"].as_array().unwrap().iter().map(|r| Flow::from_agent(r, &eps)).collect();
    let d = &f[0];
    assert_eq!((d.verdict.as_str(), d.drop_reason.as_str(), d.direction.as_str(), d.protocol.as_str()), ("DROPPED", "POLICY_DENIED", "EGRESS", "TCP"));
    assert_eq!((d.source.kind.as_str(), d.source.namespace.as_str(), d.source.pod.as_str(), d.source.endpoint), ("pod", "default", "example-pod", Some(42)));
    assert_eq!((d.source.ip.as_str(), d.source.port, d.source.workloads.clone()), ("10.0.0.2", Some(40112), vec!["Deployment web".to_string()]));
    assert_eq!((d.destination.namespace.as_str(), d.destination.pod.as_str(), d.destination.node.as_str()), ("secret", "vault-0", "node-b"));
    assert_eq!((f[1].source.kind.as_str(), f[1].source.reserved.as_str(), f[1].source.identity), ("reserved", "world", Some(2)));
    assert_eq!((f[2].source.kind.as_str(), f[2].source.node.as_str(), f[2].source.port), ("remote-node", "node-b", None));
    assert_eq!(f[2].destination.kind, "host");
    // A bare side whose address is this node's Pod is named by it; another stays a number.
    assert_eq!((f[3].source.kind.as_str(), f[3].source.pod.as_str()), ("pod", "example-pod"));
    assert_eq!((f[3].destination.kind.as_str(), f[3].destination.identity), ("identity", Some(16777218)));
}

#[test]
fn a_flow_is_withheld_when_either_side_is_a_hidden_project_or_could_be_anyones() {
    let eps = vec![Endpoint::from_agent(&endpoint_k8s()).unwrap()];
    let f: Vec<Flow> = flow_records()["flows"].as_array().unwrap().iter().map(|r| Flow::from_agent(r, &eps)).collect();
    let hidden: std::collections::HashSet<String> = ["secret".to_string()].into();
    let shown: Vec<bool> = f.iter().map(|x| x.shown_to(&hidden)).collect();
    assert_eq!(shown, vec![false, true, true, false], "its peer is in secret; world; nodes; a bare identity");
    let none = std::collections::HashSet::new();
    assert!(f[0].shown_to(&none));
}
