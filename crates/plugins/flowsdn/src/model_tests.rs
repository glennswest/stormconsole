use super::*;
use serde_json::json;

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

/// `/v1/health/modules` as health_api.rs writes it.
fn modules() -> Vec<Module> {
    [
        json!({"ID": {"Module": ["agent"], "Component": ["api"]}, "Level": "OK",
               "Message": "initial endpoint API listening", "Error": "", "LastOK": "2026-10-10T10:00:00Z",
               "Updated": "2026-10-10T10:00:00Z", "Stopped": "0001-01-01T00:00:00Z", "Final": "", "Count": 1}),
        json!({"ID": {"Module": ["agent"], "Component": ["controllers"]}, "Level": "Degraded",
               "Message": "Kubernetes node discovery enabled; identity and policy controllers are not",
               "Error": "not implemented", "LastOK": "0001-01-01T00:00:00Z",
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
    assert_eq!(m[1].id, "agent.controllers");
    assert_eq!(m[1].last_ok, "", "0001-01-01 is the agent's never");
    assert_eq!(m[1].health, Health::Idle, "not implemented is not a fault");
}

#[test]
fn the_agent_is_as_healthy_as_its_worst_part() {
    let ok = Healthz::from_agent(&json!({"agent": {"state": "Ok", "msg": "initial endpoint API ready"},
        "kubernetes": {"state": "Ok", "msg": "node-a: 3 nodes, 12 pods", "node-name": "node-a",
                       "auto-direct-node-routes": true, "service-lb": true}}));
    assert_eq!(ok.kubernetes.as_ref().unwrap().node, "node-a");
    assert_eq!(agent_health(&ok, &modules()), (Health::Ok, "node-a: 3 nodes, 12 pods".into()));

    let mut warn = ok.clone();
    warn.kubernetes.as_mut().unwrap().state = "Warning".into();
    warn.kubernetes.as_mut().unwrap().message = "annotations: 409".into();
    assert_eq!(agent_health(&warn, &modules()).0, Health::Warn);

    let mut degraded = modules();
    degraded[1].health = Health::Warn;
    assert_eq!(agent_health(&ok, &degraded).0, Health::Warn);
    degraded[0].health = Health::Error;
    assert_eq!(agent_health(&ok, &degraded).0, Health::Error);

    let standalone = Healthz::from_agent(&json!({"agent": {"state": "Ok", "msg": "initial endpoint API ready"}}));
    assert!(standalone.kubernetes.is_none());
    assert_eq!(agent_health(&standalone, &modules()), (Health::Ok, "initial endpoint API ready".into()));
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

#[test]
fn the_edition_comes_from_the_release() {
    let p = "/etc/stormcos/release/manifest.json";
    assert!(matches!(Edition::from_manifest(r#"{"edition":"flowsdn"}"#, p), Edition::Flowsdn(_)));
    assert!(Edition::from_manifest(r#"{"network":"cilium"}"#, p).is_cilium());
    assert!(matches!(Edition::from_manifest(r#"{"version":"12.13-flowsdn","components":{}}"#, p), Edition::Flowsdn(_)));
    assert!(matches!(
        Edition::from_manifest(r#"{"version":"12.13","components":{"flowsdn":{"commit":"b02c59f"}}}"#, p),
        Edition::Flowsdn(_)
    ));
    assert!(Edition::from_manifest(r#"{"version":"12.13","components":{"stormcos-cilium":{},"stormd":{}}}"#, p).is_cilium());
    assert!(matches!(Edition::from_manifest(r#"{"version":"12.13","components":{"stormd":{}}}"#, p), Edition::Unknown(_)));
    assert!(matches!(Edition::from_manifest("not json", p), Edition::Unknown(_)));
}
