//! The console's apiserver role against the code (#78). `deploy/rbac/stormconsole.yaml` is what
//! stormcos ships; these fail when a new watched kind or a new write is not in it, or when the
//! role grows past "the settings of the components it manages" (the owner, stormcos#203).

use serde::Deserialize;
use serde_json::Value;

const ROLE: &str = include_str!("../../../deploy/rbac/stormconsole.yaml");
const DOC: &str = include_str!("../../../docs/rbac.md");

fn docs() -> Vec<Value> {
    serde_yaml::Deserializer::from_str(ROLE).map(|d| Value::deserialize(d).unwrap()).collect()
}

fn rules(kind: &str, name: &str) -> Vec<Value> {
    let doc = docs().into_iter().find(|d| d["kind"] == kind && d["metadata"]["name"] == name).unwrap();
    doc["rules"].as_array().unwrap().clone()
}

fn has(list: &Value, what: &str) -> bool {
    list.as_array().is_some_and(|a| a.iter().any(|x| x == what))
}

fn allows(rules: &[Value], group: &str, resource: &str, verb: &str) -> bool {
    rules.iter().any(|r| has(&r["apiGroups"], group) && has(&r["resources"], resource) && has(&r["verbs"], verb))
}

fn cluster() -> Vec<Value> {
    rules("ClusterRole", "stormcos:stormconsole")
}

/// `/api/v1/pods` → ("", "pods"); `/apis/apps/v1/deployments` → ("apps", "deployments").
fn group_resource(list_path: &str) -> (String, String) {
    let parts: Vec<&str> = list_path.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["api", _, resource] => (String::new(), (*resource).to_string()),
        ["apis", group, _, resource] => ((*group).to_string(), (*resource).to_string()),
        _ => panic!("a list path this test does not read: {list_path}"),
    }
}

/// Every write in the role, as (group, resource, verb). Adding one is a decision: say why in
/// docs/rbac.md, and add it here.
const WRITES: &[(&str, &str, &str)] = &[
    ("kubevirt.io", "virtualmachines", "patch"),
    ("kubevirt.io", "virtualmachineinstances", "delete"),
    ("snapshot.kubevirt.io", "virtualmachinesnapshots", "create"),
    ("snapshot.kubevirt.io", "virtualmachinesnapshots", "delete"),
    ("snapshot.kubevirt.io", "virtualmachinerestores", "create"),
    ("cluster.storm.io", "clusters", "create"),
    ("cluster.storm.io", "clusters", "patch"),
    ("cluster.storm.io", "clusters", "delete"),
    ("cluster.storm.io", "clustermembers", "create"),
    ("cluster.storm.io", "clustermembers", "patch"),
    ("cluster.storm.io", "clustermembers", "delete"),
    ("storm.io", "cloudimages", "delete"),
];

#[test]
fn every_kind_the_console_watches_may_be_listed_and_watched() {
    let role = cluster();
    let watched = plugin_kubernetes::cache::RESOURCES.iter().chain(plugin_vm::RESOURCES).chain(plugin_stormcluster::objects::RESOURCES);
    let mut n = 0;
    for spec in watched {
        let (group, resource) = group_resource(spec.list_path);
        for verb in ["get", "list", "watch"] {
            assert!(allows(&role, &group, &resource, verb), "{group}/{resource} {verb}: watched by the console ({})", spec.kind);
        }
        n += 1;
    }
    assert!(n >= 30, "the tables were read: {n}");
}

#[test]
fn what_the_console_reads_on_demand_on_its_own_credential() {
    let role = cluster();
    // Cluster-wide events, the runs it keeps, and a pod page's owners and Services.
    assert!(allows(&role, "", "events", "list"));
    assert!(allows(&role, "", "pods/log", "get"));
    assert!(allows(&role, "", "endpoints", "get"));
    assert!(allows(&role, "apps", "replicasets", "get"));
    assert!(allows(&role, "project.openshift.io", "projects", "list"));
}

#[test]
fn its_writes_are_exactly_the_list() {
    let mut found = Vec::new();
    for r in cluster() {
        let Some(verbs) = r["verbs"].as_array() else { continue };
        for verb in verbs.iter().filter_map(Value::as_str).filter(|v| !matches!(*v, "get" | "list" | "watch")) {
            for group in r["apiGroups"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                for resource in r["resources"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    found.push((group.to_string(), resource.to_string(), verb.to_string()));
                }
            }
        }
    }
    let mut listed: Vec<_> = WRITES.iter().map(|(g, r, v)| (g.to_string(), r.to_string(), v.to_string())).collect();
    found.sort();
    listed.sort();
    assert_eq!(found, listed);
}

#[test]
fn it_is_not_cluster_admin_by_another_name() {
    let role = cluster();
    let text = serde_json::to_string(&role).unwrap();
    assert!(!text.contains("\"*\""), "no wildcard");
    assert!(!allows(&role, "", "secrets", "get"), "no Secret outside its own namespace");
    assert!(!allows(&role, "", "secrets", "list"));
    for (group, resource, verb) in [
        ("", "namespaces", "create"),
        ("", "pods", "create"),
        ("", "pods", "delete"),
        ("apps", "deployments", "update"),
        ("rbac.authorization.k8s.io", "rolebindings", "create"),
        ("rbac.authorization.k8s.io", "clusterroles", "create"),
        ("kubevirt.io", "virtualmachines", "create"),
        ("kubevirt.io", "virtualmachines", "delete"),
    ] {
        assert!(!allows(&role, group, resource, verb), "{group}/{resource} {verb} is a signed-in user's");
    }
}

#[test]
fn the_keys_home_is_namespaced_and_the_migration_can_be_dropped() {
    let keys = rules("Role", "stormconsole-keys");
    for verb in ["get", "list", "create", "update", "delete"] {
        assert!(allows(&keys, "", "secrets", verb));
    }
    let doc = docs();
    let ns = |name: &str| doc.iter().find(|d| d["kind"] == "Role" && d["metadata"]["name"] == name).unwrap()["metadata"]["namespace"].clone();
    assert_eq!(ns("stormconsole-keys"), "storm-users");
    assert_eq!(ns("stormconsole-keys-migration"), "default");
    let migrate = rules("Role", "stormconsole-keys-migration");
    assert!(!allows(&migrate, "", "secrets", "create"), "it only takes them out");
    assert!(doc.iter().any(|d| d["kind"] == "Namespace" && d["metadata"]["name"] == "storm-users"), "stormcos ships it");
}

#[test]
fn every_binding_names_the_service_account() {
    let bindings: Vec<_> = docs().into_iter().filter(|d| matches!(d["kind"].as_str(), Some("ClusterRoleBinding" | "RoleBinding"))).collect();
    assert_eq!(bindings.len(), 3);
    for b in bindings {
        let s = &b["subjects"][0];
        assert_eq!((s["kind"].as_str(), s["name"].as_str(), s["namespace"].as_str()), (Some("ServiceAccount"), Some("stormconsole"), Some("kube-system")));
    }
}

#[test]
fn the_doc_names_every_resource_the_role_does() {
    for r in cluster() {
        for resource in r["resources"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            assert!(DOC.contains(resource), "docs/rbac.md does not mention {resource}");
        }
    }
}
