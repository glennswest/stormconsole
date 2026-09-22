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
    Action { id: id.into(), label: label.into(), method: method.into(), path, enabled, danger, tone: None }
}

/// What stormvm says about the machines it is running, keyed `ns/name`.
/// Empty when there is no stormvm, or it is not answering, or it is not
/// running this machine — all of which mean the same thing here: no verb
/// is offered that cannot be served.
pub type Running = HashMap<String, Value>;

pub fn map(snap: &Snapshot) -> Vec<ComponentSummary> {
    map_with(snap, &Running::new())
}

pub fn map_with(snap: &Snapshot, running: &Running) -> Vec<ComponentSummary> {
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

        // The node, the vCPU and the memory each have a column of their
        // own now — the first from the placement edge, the other two from
        // metrics — so the detail line stops repeating them. It says the
        // one thing nothing else says.
        let mut detail = vec![phase.to_string()];
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
        // The address, and how it is reached — the two facts somebody scanning
        // a list of machines is actually looking for.
        //
        // `status.interfaces[]` carries both: the address from the guest
        // agent, the binding from what the node built. A machine on
        // `masquerade` is behind a NAT inside the hypervisor process and
        // nothing outside the node can route to it, which is worth seeing on
        // the row rather than discovering by trying.
        if let Some(ifs) = v.pointer("/status/interfaces").and_then(Value::as_array) {
            if let Some(ip) = ifs
                .iter()
                .filter_map(|i| i.get("ipAddress").and_then(Value::as_str))
                .find(|s| !s.is_empty())
            {
                c.metrics.push(Metric::new("ip", ip.to_string()));
            }
            if let Some(b) = ifs.first().and_then(|i| i.get("storm.io/binding")).and_then(Value::as_str) {
                c.metrics.push(
                    Metric::new("network", b.to_string()).tone(match b {
                        // Reachable.
                        "bridge" => "ok",
                        // A NAT inside the hypervisor: the guest has an
                        // address and nothing outside can use it.
                        "user" => "warn",
                        _ => "muted",
                    }),
                );
            }
        }
        if let Some(n) = node {
            // Where the machine *is*, which is a placement and not
            // something the machine contains — so `belongs_to`, the
            // direction that says so. It was `has_one`, which the table
            // read as containment and the card as where the row leads, so
            // opening a VM landed in node details (#18).
            //
            // 203d5b8 published the node as a metric as well, to get it
            // into the list at all. That was the workaround: the table now
            // gives every `belongs_to` whose values differ down the list a
            // column of its own, so the node is a sortable column here
            // without this plugin asking for one, and the metric would be
            // the same fact printed twice.
            c.relations.push(Relation::belongs_to("node", format!("k8s:node:{n}")));
        }
        let defined = of("vm").contains_key(key);
        if defined {
            c.relations.push(Relation::belongs_to("definition", format!("vm:machine:{key}")));
        }
        // On the row, so the common things do not need a detail view first.
        //
        // Stopping a running instance is deleting the instance: there is no
        // other verb, and nothing about a VMI survives being stopped except
        // its definition, if it has one. So "stop" is destructive for an
        // instance applied on its own — there is nothing left to start it
        // again — and ordinary for one a VirtualMachine defines.
        //
        // Restart is the same delete with the definition present to put the
        // machine back; without one it is a delete wearing a reassuring
        // name, so it is offered disabled rather than not at all, because
        // "why can I not restart this" is a question the row should answer.
        c.actions = vec![
            action(
                "restart",
                "Restart",
                "POST",
                format!("/api/plugins/vm/machines/{ns}/{name}/restart"),
                defined && phase == "Running",
                false,
            ),
            action(
                "stop",
                "Stop",
                "POST",
                format!("/api/plugins/vm/instances/{ns}/{name}/stop"),
                phase == "Running",
                !defined,
            ),
        ];
        // The verbs the hypervisor itself serves, offered only where
        // stormvm reports the machine can take them.
        //
        // `control.lifecycle` is whether the control socket was bound and
        // `control.freeze` whether the guest has its own agent — a spec
        // that did not ask for the channel can never have one. Offering a
        // button that 404s makes a client report that *the VM* refused,
        // which sends whoever pressed it looking at the guest.
        //
        // The ordering is deliberate: what a guest survives, first. A soft
        // reboot is a request the guest can honour; a reset is the button
        // on the front of the box and is marked as such.
        if let Some(seen) = running.get(key) {
            let can = |k: &str| {
                seen.pointer(&format!("/control/{k}")).and_then(Value::as_bool).unwrap_or(false)
            };
            let verb = |id: &str, label: &str, danger: bool| {
                action(
                    id,
                    label,
                    "POST",
                    format!("/api/plugins/vm/machines/{ns}/{name}/verb/{id}"),
                    true,
                    danger,
                )
            };
            if can("lifecycle") {
                c.actions.push(verb("softreboot", "Soft reboot", false));
                c.actions.push(verb("pause", "Pause", false));
                c.actions.push(verb("unpause", "Resume", false));
                // No warning to the guest, so it is shelved with the
                // destructive ones rather than sitting beside Pause.
                c.actions.push(verb("reset", "Reset", true));
            }
            if can("freeze") {
                // Quiescing is what makes a disk copy trustworthy, and a
                // guest left frozen has every write blocked — which from
                // inside looks like a machine that has hung. Both halves
                // are offered together so the way back is never further
                // away than the way in.
                c.actions.push(verb("freeze", "Freeze filesystems", true));
                c.actions.push(verb("thaw", "Thaw filesystems", false));
            }
            // What the doors can actually do, as a fact about the machine
            // rather than something to find out by opening a tab.
            let door = |k: &str| {
                seen.pointer(&format!("/console/{k}")).and_then(Value::as_bool).unwrap_or(false)
            };
            let doors = match (door("serial"), door("vnc")) {
                (true, true) => "serial + screen",
                (true, false) => "serial",
                (false, true) => "screen",
                (false, false) => "none",
            };
            c.metrics.push(Metric::new("console", doors).tone(if doors == "none" {
                "muted"
            } else {
                "accent"
            }));
        }
        c.actions.push(action(
            "delete",
            "Delete",
            "DELETE",
            format!("/api/plugins/vm/machines/{ns}/{name}"),
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
            "restart",
            "Restart",
            "POST",
            format!("/api/plugins/vm/machines/{key}/restart"),
            live,
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
    use console_core::RelationKind;
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
        // The node, the vCPU and the memory are columns, so the detail
        // line does not print them a second time.
        assert_eq!(vm.detail, "Running");
        assert_eq!(vm.metrics.iter().find(|m| m.label == "vcpu").unwrap().value, "2");
        assert_eq!(vm.metrics.iter().find(|m| m.label == "memory").unwrap().value, "4Gi");
        assert_eq!(vm.link.as_deref(), Some("#/vm/default/web-1"));
        assert!(vm.relations.iter().any(|r| r.targets == vec!["k8s:node:storm-2c91b3"]));
        assert_eq!(vm.metrics.iter().find(|m| m.label == "disks").unwrap().value, "2");
    }

    /// The node is where the machine *is*, and the direction of that edge
    /// is what stops the table nesting a node inside a VM and the card
    /// treating it as where the row leads (#18).
    #[test]
    fn the_node_a_machine_runs_on_is_a_placement_not_a_destination() {
        let sn = snap(
            "vmi",
            "default/web-1",
            json!({"status": {"phase": "Running", "nodeName": "storm-2c91b3"}}),
        );
        let vm = &map(&sn)[0];
        let node = vm
            .relations
            .iter()
            .find(|r| r.name == "node")
            .expect("a running instance says which node");
        assert_eq!(node.kind, RelationKind::BelongsTo);
        assert_eq!(node.targets, vec!["k8s:node:storm-2c91b3"]);
        // And not the same fact a second time: the column comes off the
        // edge, so the metric 203d5b8 added as a workaround is gone.
        assert!(!vm.metrics.iter().any(|m| m.label == "node"), "{:?}", vm.metrics);
    }

    /// Stop was published twice — once on the row and once in the menu,
    /// pointing at the same path with different danger — so a machine
    /// offered two Stops and one of them asked for confirmation.
    #[test]
    fn an_instance_offers_each_verb_once() {
        let sn = snap("vmi", "default/web-1", json!({"status": {"phase": "Running"}}));
        let acts = &map(&sn)[0].actions;
        let ids: Vec<&str> = acts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["restart", "stop", "delete"]);
        assert_eq!(ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len());
    }

    /// Restart is the instance deleted with something there to put it
    /// back. Without a definition that is a delete, so it is offered
    /// disabled rather than performed under a reassuring name.
    #[test]
    fn restart_needs_a_definition_to_restart_from() {
        let alone = snap("vmi", "default/web-1", json!({"status": {"phase": "Running"}}));
        let inst = &map(&alone)[0];
        assert!(!inst.actions.iter().find(|a| a.id == "restart").unwrap().enabled);
        // Stopping a machine nothing will restart is destructive, and says so.
        assert!(inst.actions.iter().find(|a| a.id == "stop").unwrap().danger);

        let mut defined = snap("vm", "default/web-1", json!({"spec": {"running": true}}));
        defined.insert(
            "vmi",
            HashMap::from([(
                "default/web-1".to_string(),
                json!({"status": {"phase": "Running"}}),
            )]),
        );
        let out = map(&defined);
        let inst = out.iter().find(|c| c.id == "vm:instance:default/web-1").unwrap();
        assert!(inst.actions.iter().find(|a| a.id == "restart").unwrap().enabled);
        assert!(!inst.actions.iter().find(|a| a.id == "stop").unwrap().danger);
        let def = out.iter().find(|c| c.id == "vm:machine:default/web-1").unwrap();
        assert!(def.actions.iter().find(|a| a.id == "restart").unwrap().enabled);
    }

    /// stormvm reports per machine what it can be asked to do; a verb is
    /// offered only where it can actually be served. A button that 404s
    /// makes a client report that *the VM* refused, which sends whoever
    /// pressed it looking at the guest (stormvm#9).
    #[test]
    fn a_verb_is_offered_only_where_the_hypervisor_serves_it() {
        let sn = snap("vmi", "default/web-1", json!({"status": {"phase": "Running"}}));

        // No stormvm, or not running this machine: kube verbs only.
        let ids: Vec<String> =
            map(&sn)[0].actions.iter().map(|a| a.id.clone()).collect();
        assert_eq!(ids, vec!["restart", "stop", "delete"]);

        // A control socket, no guest agent.
        let running = Running::from([(
            "default/web-1".to_string(),
            json!({"control": {"lifecycle": true, "freeze": false},
                   "console": {"serial": true, "vnc": false}}),
        )]);
        let c = &map_with(&sn, &running)[0];
        let ids: Vec<&str> = c.actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["restart", "stop", "softreboot", "pause", "unpause", "reset", "delete"]);
        // Reset is the button on the front of the box; a soft reboot is a
        // request the guest can honour. Only one of them says so.
        let danger = |id: &str| c.actions.iter().find(|a| a.id == id).unwrap().danger;
        assert!(danger("reset") && !danger("softreboot"));
        assert_eq!(c.metrics.iter().find(|m| m.label == "console").unwrap().value, "serial");

        // With an agent, both halves of freeze — the way back is never
        // further away than the way in.
        let running = Running::from([(
            "default/web-1".to_string(),
            json!({"control": {"lifecycle": true, "freeze": true}}),
        )]);
        let c = &map_with(&sn, &running)[0];
        assert!(c.actions.iter().any(|a| a.id == "freeze"));
        assert!(c.actions.iter().any(|a| a.id == "thaw"));
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
