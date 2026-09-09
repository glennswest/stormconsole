//! KubeVirt objects → components.
//!
//! Two objects, and they answer different questions. A `VirtualMachine`
//! is the definition — what should exist, and whether it should be
//! running. A `VirtualMachineInstance` is the running machine — which
//! node, which phase, which interfaces. The console shows both, related,
//! because "defined but not running" and "running" are different states
//! an operator acts on differently.

use std::collections::HashMap;

use console_core::{Action, ComponentSummary, Health, Metric, Relation};
use serde_json::Value;

pub type Snapshot = HashMap<&'static str, HashMap<String, Value>>;

fn s<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

fn split_key(key: &str) -> (&str, &str) {
    key.split_once('/').unwrap_or(("default", key))
}

/// vCPU as the spec asks for it. KubeVirt spells the same thing three
/// ways — `cores`, `sockets`×`cores`×`threads`, or a resource request —
/// and a VM page that shows one and ignores the others is wrong for the
/// VMs that used another.
pub fn vcpus(domain: &Value) -> Option<i64> {
    let cpu = domain.get("cpu");
    if let Some(cpu) = cpu {
        let cores = cpu.get("cores").and_then(Value::as_i64);
        let sockets = cpu.get("sockets").and_then(Value::as_i64);
        let threads = cpu.get("threads").and_then(Value::as_i64);
        if cores.is_some() || sockets.is_some() || threads.is_some() {
            return Some(cores.unwrap_or(1) * sockets.unwrap_or(1) * threads.unwrap_or(1));
        }
    }
    domain
        .pointer("/resources/requests/cpu")
        .and_then(Value::as_str)
        .and_then(|c| c.trim_end_matches('m').parse::<i64>().ok())
}

/// Guest memory, printed as the spec wrote it (`4Gi`), because that is
/// what somebody typed and what they will look for.
pub fn memory(domain: &Value) -> Option<String> {
    s(domain, "/memory/guest")
        .or_else(|| s(domain, "/resources/requests/memory"))
        .map(str::to_string)
}

/// Disks, named as the guest sees them, with the volume each is backed by.
pub fn disks(spec: &Value) -> Vec<(String, String)> {
    let volumes: HashMap<&str, &Value> = spec
        .pointer("/volumes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| Some((v.get("name")?.as_str()?, v))).collect())
        .unwrap_or_default();
    spec.pointer("/domain/devices/disks")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| {
                    let name = d.get("name")?.as_str()?;
                    Some((name.to_string(), backing(volumes.get(name).copied())))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What a volume entry actually is, in the words the operator used.
fn backing(v: Option<&Value>) -> String {
    let Some(v) = v else { return "no volume".to_string() };
    for (key, label) in [
        ("dataVolume", "golden"),
        ("persistentVolumeClaim", "pvc"),
        ("containerDisk", "container disk"),
        ("hostDisk", "host disk"),
    ] {
        if let Some(inner) = v.get(key) {
            let name = inner
                .get("name")
                .or_else(|| inner.get("claimName"))
                .or_else(|| inner.get("image"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            return format!("{label} {name}");
        }
    }
    if v.get("cloudInitNoCloud").is_some() || v.get("cloudInitConfigDrive").is_some() {
        return "cloud-init seed".to_string();
    }
    v.as_object()
        .and_then(|o| o.keys().find(|k| *k != "name"))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string())
}

/// A running machine's health. `phase` is KubeVirt's word for it and the
/// kubelet writes it through the `/status` subresource, so it is the
/// cluster's own verdict rather than one derived here.
fn instance_health(phase: &str) -> Health {
    match phase {
        "Running" => Health::Ok,
        "Scheduling" | "Scheduled" | "Pending" => Health::Warn,
        "Succeeded" => Health::Idle,
        "Failed" => Health::Error,
        _ => Health::Unknown,
    }
}

fn action(id: &str, label: &str, method: &str, path: String, enabled: bool, danger: bool) -> Action {
    Action { id: id.into(), label: label.into(), method: method.into(), path, enabled, danger }
}

pub fn map(snap: &Snapshot) -> Vec<ComponentSummary> {
    let empty = HashMap::new();
    let of = |kind: &str| snap.get(kind).unwrap_or(&empty);
    let mut out = Vec::new();

    for (key, obj) in of("vmi") {
        let (ns, name) = split_key(key);
        let phase = s(obj, "/status/phase").unwrap_or("Unknown");
        let node = s(obj, "/status/nodeName").or_else(|| s(obj, "/spec/nodeName"));
        let spec = obj.get("spec").cloned().unwrap_or(Value::Null);
        let domain = spec.get("domain").cloned().unwrap_or(Value::Null);
        let cpus = vcpus(&domain);
        let mem = memory(&domain);
        let ds = disks(&spec);

        let mut detail = vec![phase.to_string()];
        if let Some(n) = node {
            detail.push(n.to_string());
        }
        if let Some(c) = cpus {
            detail.push(format!("{c} vCPU"));
        }
        if let Some(m) = &mem {
            detail.push(m.clone());
        }
        // A VM that would not start says why here rather than in a log on
        // a node with no shell — the kubelet writes the reason.
        if let Some(msg) = s(obj, "/status/reason").or_else(|| s(obj, "/status/message")) {
            if phase != "Running" {
                detail.push(msg.to_string());
            }
        }

        let mut c = ComponentSummary {
            id: format!("vm:instance:{key}"),
            kind: "vm".into(),
            label: name.to_string(),
            health: instance_health(phase),
            detail: detail.join(" · "),
            metrics: vec![],
            actions: vec![],
            relations: vec![Relation::belongs_to("namespace", format!("k8s:ns:{ns}"))],
            link: Some(format!("#/vm/{ns}/{name}")),
        };
        if let Some(cp) = cpus {
            c.metrics.push(Metric::new("vcpu", cp.to_string()));
        }
        if let Some(m) = mem {
            c.metrics.push(Metric::new("memory", m));
        }
        c.metrics.push(Metric::new("disks", ds.len().to_string()).tone("muted"));
        if let Some(n) = node {
            c.relations.push(Relation::has_one("node", format!("k8s:node:{n}")));
        }
        if of("vm").contains_key(key) {
            c.relations.push(Relation::belongs_to("definition", format!("vm:machine:{key}")));
        }
        // Stopping a running instance is deleting the instance: without a
        // definition there is nothing to restart it, which is exactly what
        // "stop" means for a VMI applied on its own.
        c.actions.push(action(
            "stop",
            "Stop",
            "POST",
            format!("/api/plugins/vm/instances/{key}/stop"),
            true,
            true,
        ));
        out.push(c);
    }

    for (key, obj) in of("vm") {
        let (ns, name) = split_key(key);
        let running = obj
            .pointer("/spec/running")
            .and_then(Value::as_bool)
            .or_else(|| match s(obj, "/spec/runStrategy") {
                Some("Always") | Some("RerunOnFailure") => Some(true),
                Some("Halted") | Some("Manual") => Some(false),
                _ => None,
            });
        let live = of("vmi").contains_key(key);
        let (health, detail) = match (running, live) {
            (_, true) => (Health::Ok, "running".to_string()),
            (Some(true), false) => (
                Health::Warn,
                "wanted running, no instance — nothing places one yet (stormvm docs/kube.md)"
                    .to_string(),
            ),
            _ => (Health::Idle, "stopped".to_string()),
        };
        let spec = obj.pointer("/spec/template/spec").cloned().unwrap_or(Value::Null);
        let domain = spec.get("domain").cloned().unwrap_or(Value::Null);
        let mut c = ComponentSummary {
            id: format!("vm:machine:{key}"),
            kind: "vm-definition".into(),
            label: name.to_string(),
            health,
            detail,
            metrics: vec![],
            actions: vec![],
            relations: vec![Relation::belongs_to("namespace", format!("k8s:ns:{ns}"))],
            link: Some(format!("#/vm/{ns}/{name}")),
        };
        if let Some(cp) = vcpus(&domain) {
            c.metrics.push(Metric::new("vcpu", cp.to_string()));
        }
        if let Some(m) = memory(&domain) {
            c.metrics.push(Metric::new("memory", m));
        }
        if live {
            c.relations.push(Relation::has_one("instance", format!("vm:instance:{key}")));
        }
        c.actions.push(action(
            "start",
            "Start",
            "POST",
            format!("/api/plugins/vm/machines/{key}/start"),
            !live,
            false,
        ));
        c.actions.push(action(
            "stop",
            "Stop",
            "POST",
            format!("/api/plugins/vm/machines/{key}/stop"),
            live || running == Some(true),
            true,
        ));
        c.actions.push(action(
            "delete",
            "Delete",
            "DELETE",
            format!("/api/plugins/vm/machines/{key}"),
            true,
            true,
        ));
        out.push(c);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap(kind: &'static str, key: &str, obj: Value) -> Snapshot {
        HashMap::from([(kind, HashMap::from([(key.to_string(), obj)]))])
    }

    #[test]
    fn a_running_instance_says_where_it_runs_and_what_it_has() {
        let sn = snap(
            "vmi",
            "default/web-1",
            json!({
                "spec": {
                    "domain": {"cpu": {"cores": 2}, "memory": {"guest": "4Gi"},
                               "devices": {"disks": [{"name": "root"}, {"name": "seed"}]}},
                    "volumes": [
                        {"name": "root", "dataVolume": {"name": "rocky-10"}},
                        {"name": "seed", "cloudInitNoCloud": {"userData": "#cloud-config"}}
                    ]
                },
                "status": {"phase": "Running", "nodeName": "storm-2c91b3"}
            }),
        );
        let out = map(&sn);
        assert_eq!(out.len(), 1);
        let vm = &out[0];
        assert_eq!(vm.id, "vm:instance:default/web-1");
        assert_eq!(vm.health, Health::Ok);
        assert_eq!(vm.detail, "Running · storm-2c91b3 · 2 vCPU · 4Gi");
        assert_eq!(vm.link.as_deref(), Some("#/vm/default/web-1"));
        assert!(vm.relations.iter().any(|r| r.targets == vec!["k8s:node:storm-2c91b3"]));
        assert_eq!(vm.metrics.iter().find(|m| m.label == "disks").unwrap().value, "2");
    }

    #[test]
    fn a_failed_instance_carries_the_reason_it_did_not_start() {
        let sn = snap(
            "vmi",
            "default/win",
            json!({"status": {"phase": "Failed", "reason": "the hypervisor exited with 1"}}),
        );
        let out = map(&sn);
        assert_eq!(out[0].health, Health::Error);
        assert!(out[0].detail.contains("the hypervisor exited with 1"), "{}", out[0].detail);
    }

    #[test]
    fn cpu_is_read_however_the_spec_spelled_it() {
        assert_eq!(vcpus(&json!({"cpu": {"cores": 4}})), Some(4));
        assert_eq!(vcpus(&json!({"cpu": {"sockets": 2, "cores": 2, "threads": 2}})), Some(8));
        assert_eq!(vcpus(&json!({"resources": {"requests": {"cpu": "2"}}})), Some(2));
        assert_eq!(vcpus(&json!({})), None);
    }

    #[test]
    fn a_disk_names_what_backs_it() {
        let spec = json!({
            "domain": {"devices": {"disks": [{"name": "root"}, {"name": "data"}, {"name": "orphan"}]}},
            "volumes": [
                {"name": "root", "dataVolume": {"name": "rocky-10"}},
                {"name": "data", "persistentVolumeClaim": {"claimName": "pg"}}
            ]
        });
        let d = disks(&spec);
        assert_eq!(d[0], ("root".into(), "golden rocky-10".into()));
        assert_eq!(d[1], ("data".into(), "pvc pg".into()));
        assert_eq!(d[2], ("orphan".into(), "no volume".into()));
    }

    #[test]
    fn a_definition_wanting_to_run_with_no_instance_is_not_pretended_ok() {
        let sn = snap("vm", "default/web-1", json!({"spec": {"running": true}}));
        let out = map(&sn);
        assert_eq!(out[0].id, "vm:machine:default/web-1");
        assert_eq!(out[0].health, Health::Warn);
        assert!(out[0].detail.contains("nothing places one yet"), "{}", out[0].detail);
        let start = out[0].actions.iter().find(|a| a.id == "start").unwrap();
        assert!(start.enabled);
    }

    #[test]
    fn a_definition_and_its_instance_point_at_each_other() {
        let mut sn = snap("vm", "default/web-1", json!({"spec": {"running": true}}));
        sn.insert(
            "vmi",
            HashMap::from([(
                "default/web-1".to_string(),
                json!({"status": {"phase": "Running", "nodeName": "n1"}}),
            )]),
        );
        let out = map(&sn);
        let inst = out.iter().find(|c| c.id == "vm:instance:default/web-1").unwrap();
        let def = out.iter().find(|c| c.id == "vm:machine:default/web-1").unwrap();
        assert!(inst.relations.iter().any(|r| r.targets == vec!["vm:machine:default/web-1"]));
        assert!(def.relations.iter().any(|r| r.targets == vec!["vm:instance:default/web-1"]));
        assert_eq!(def.health, Health::Ok);
        assert!(!def.actions.iter().find(|a| a.id == "start").unwrap().enabled);
    }

    #[test]
    fn runstrategy_is_read_as_well_as_running() {
        let sn = snap("vm", "default/a", json!({"spec": {"runStrategy": "Halted"}}));
        assert_eq!(map(&sn)[0].health, Health::Idle);
        let sn = snap("vm", "default/b", json!({"spec": {"runStrategy": "Always"}}));
        assert_eq!(map(&sn)[0].health, Health::Warn);
    }
}
