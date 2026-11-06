//! A machine's metadata, the images it boots from, and what the node does
//! not report about either (#69) — the same questions the pod page
//! answers, asked of a VM.
//!
//! Read from rustkube-node (2026-10-02): the instance's status carries
//! `phase`, `nodeName`, `reason`, `message`, conditions and the
//! `storm.io/startedUnix` / `readyUnix` / `bootSeconds` marks; no image
//! digest for any disk. Interface counters are stormvm's (stormvm#48), on the
//! machine's Metrics tab (#14).

use serde_json::{json, Map, Value};

const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

fn merged(objects: &[Option<&Value>], ptr: &str) -> Map<String, Value> {
    // The definition first, the instance over it: the instance is what
    // runs, and the definition's labels are where it got most of them.
    let mut out = Map::new();
    for o in objects.iter().flatten() {
        if let Some(m) = o.pointer(ptr).and_then(Value::as_object) {
            for (k, v) in m {
                if k != LAST_APPLIED {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
    }
    out
}

/// Labels, annotations, conditions and the boot marks.
pub fn metadata(machine: Option<&Value>, instance: Option<&Value>) -> Value {
    let status = instance.and_then(|v| v.get("status"));
    let mark = |k: &str| status.and_then(|s| s.get(k)).cloned().unwrap_or(Value::Null);
    json!({
        "created": machine.or(instance).and_then(|v| v.pointer("/metadata/creationTimestamp")).cloned(),
        "uid": instance.and_then(|v| v.pointer("/metadata/uid")).cloned(),
        "labels": merged(&[machine, instance], "/metadata/labels"),
        "annotations": merged(&[machine, instance], "/metadata/annotations"),
        "conditions": status.and_then(|s| s.get("conditions")).cloned().unwrap_or(json!([])),
        "startedUnix": mark("storm.io/startedUnix"),
        "readyUnix": mark("storm.io/readyUnix"),
        "bootSeconds": mark("storm.io/bootSeconds"),
        "message": status.and_then(|s| s.get("message")).cloned().unwrap_or(Value::Null),
    })
}

/// What `virsh dominfo` would say, from the spec and the running instance: the domain
/// asked for and the state it is in, with when it entered each phase.
pub fn domain(machine: Option<&Value>, instance: Option<&Value>) -> Value {
    let spec = instance
        .and_then(|v| v.pointer("/spec/domain"))
        .or_else(|| machine.and_then(|m| m.pointer("/spec/template/spec/domain")));
    let at = |p: &str| spec.and_then(|d| d.pointer(p)).cloned().unwrap_or(Value::Null);
    let status = instance.and_then(|v| v.get("status"));
    json!({
        "printable": machine.and_then(|m| m.pointer("/status/printableStatus")).cloned().unwrap_or(Value::Null),
        "phase": status.and_then(|s| s.get("phase")).cloned().unwrap_or(Value::Null),
        "transitions": status.and_then(|s| s.get("phaseTransitionTimestamps")).cloned().unwrap_or(json!([])),
        "machineType": at("/machine/type"),
        "cpuModel": at("/cpu/model"),
        "sockets": at("/cpu/sockets"),
        "threads": at("/cpu/threads"),
        "firmware": at("/firmware"),
        "qos": status.and_then(|s| s.get("qosClass")).cloned().unwrap_or(Value::Null),
    })
}

/// The guest agent: KubeVirt's `AgentConnected` condition and `guestOSInfo`, and an
/// interface whose address came from it. A node that says none of these has told the
/// console nothing, which is not the same as there being no agent.
pub fn guest_agent(instance: Option<&Value>) -> Value {
    let status = instance.and_then(|v| v.get("status"));
    let condition = status
        .and_then(|s| s.get("conditions"))
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c.get("type").and_then(Value::as_str) == Some("AgentConnected")));
    let os = status.and_then(|s| s.get("guestOSInfo")).filter(|o| o.as_object().is_some_and(|m| !m.is_empty()));
    let from_agent = status
        .and_then(|s| s.get("interfaces"))
        .and_then(Value::as_array)
        .is_some_and(|i| i.iter().any(|n| n.get("infoSource").and_then(Value::as_str).is_some_and(|s| s.contains("guest-agent"))));
    let connected = match condition.and_then(|c| c.get("status")).and_then(Value::as_str) {
        Some(s) => Some(s == "True"),
        None if os.is_some() || from_agent => Some(true),
        None => None,
    };
    json!({
        "connected": connected,
        "since": condition.and_then(|c| c.get("lastTransitionTime")).cloned().unwrap_or(Value::Null),
        "reason": condition.and_then(|c| c.get("message").or_else(|| c.get("reason"))).cloned().unwrap_or(Value::Null),
        "os": os.cloned().unwrap_or(Value::Null),
        "addressesFromAgent": from_agent,
    })
}

/// Each disk's source image, and its digest where the reference carries
/// one.
pub fn images(spec: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for v in spec.pointer("/volumes").and_then(Value::as_array).into_iter().flatten() {
        let name = v.get("name").and_then(Value::as_str).unwrap_or("");
        let (kind, source) = if let Some(i) = v.pointer("/containerDisk/image").and_then(Value::as_str) {
            ("container disk", i)
        } else if let Some(g) = v.pointer("/dataVolume/name").and_then(Value::as_str) {
            ("instance of registry image", g)
        } else if let Some(c) = v.pointer("/persistentVolumeClaim/claimName").and_then(Value::as_str) {
            ("claim", c)
        } else {
            // cloud-init seeds, emptyDisks: no image behind them.
            continue;
        };
        out.push(json!({
            "disk": name,
            "kind": kind,
            "source": source,
            "digest": plugin_kubernetes::pod::digest_of(source),
        }));
    }
    out
}

/// What the page cannot show for a machine, each with its issue.
pub fn gaps(images: &[Value]) -> Vec<Value> {
    let mut out = Vec::new();
    if images.iter().any(|i| i["digest"].is_null()) {
        out.push(json!({
            "what": "image digest",
            "why": "the node reports no sha256 for the registry image a disk was cloned from",
            "issue": plugin_kubernetes::pod::STATUS_ISSUE,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_domain_and_the_agent_say_what_they_know() {
        let vmi = json!({"spec": {"domain": {"machine": {"type": "q35"}, "cpu": {"model": "host-passthrough", "sockets": 2}, "firmware": {"bootloader": {"efi": {}}}}},
            "status": {"phase": "Running", "qosClass": "Burstable", "phaseTransitionTimestamps": [{"phase": "Running", "phaseTransitionTimestamp": "2026-11-05T00:00:00Z"}],
              "conditions": [{"type": "AgentConnected", "status": "True", "lastTransitionTime": "2026-11-05T00:01:00Z"}],
              "guestOSInfo": {"prettyName": "Fedora 44", "kernelRelease": "6.17"}, "interfaces": []}});
        let d = domain(None, Some(&vmi));
        assert_eq!((d["machineType"].as_str(), d["cpuModel"].as_str(), d["sockets"].as_u64()), (Some("q35"), Some("host-passthrough"), Some(2)));
        assert_eq!(d["transitions"][0]["phase"], "Running");
        let g = guest_agent(Some(&vmi));
        assert_eq!((g["connected"].as_bool(), g["os"]["prettyName"].as_str()), (Some(true), Some("Fedora 44")));
        // A stopped machine still says what its definition asks for.
        let vm = json!({"spec": {"template": {"spec": {"domain": {"machine": {"type": "pc"}}}}}, "status": {"printableStatus": "Stopped"}});
        let d = domain(Some(&vm), None);
        assert_eq!((d["machineType"].as_str(), d["printable"].as_str(), d["phase"].as_str()), (Some("pc"), Some("Stopped"), None));
        // Said to be down, and not said at all, are different answers.
        let down = json!({"status": {"conditions": [{"type": "AgentConnected", "status": "False", "message": "no socket"}]}});
        assert_eq!((guest_agent(Some(&down))["connected"].as_bool(), guest_agent(Some(&down))["reason"].as_str()), (Some(false), Some("no socket")));
        assert!(guest_agent(Some(&json!({"status": {}})))["connected"].is_null());
        assert!(guest_agent(None)["connected"].is_null());
        // An address that came from the agent says it is there.
        let via = json!({"status": {"interfaces": [{"infoSource": "domain, guest-agent"}]}});
        assert_eq!(guest_agent(Some(&via))["connected"].as_bool(), Some(true));
    }


    #[test]
    fn the_instance_wins_and_last_applied_is_dropped() {
        let vm = json!({"metadata":{"creationTimestamp":"t0","labels":{"app":"web","tier":"a"},
                        "annotations":{"kubectl.kubernetes.io/last-applied-configuration":"{}","note":"x"}}});
        let vmi = json!({"metadata":{"uid":"u","labels":{"tier":"b"}},
                         "status":{"conditions":[{"type":"Ready","status":"True"}],"storm.io/bootSeconds":12}});
        let m = metadata(Some(&vm), Some(&vmi));
        assert_eq!(m["labels"], json!({"app":"web","tier":"b"}));
        assert_eq!(m["annotations"], json!({"note":"x"}));
        assert_eq!(m["bootSeconds"], 12);
        assert_eq!(m["created"], "t0");
        assert_eq!(m["conditions"][0]["type"], "Ready");
    }

    #[test]
    fn images_are_the_disks_with_a_source() {
        let hex = "d".repeat(64);
        let spec = json!({"volumes":[
            {"name":"root","dataVolume":{"name":"rocky-10"}},
            {"name":"tools","containerDisk":{"image":format!("reg/tools@sha256:{hex}")}},
            {"name":"data","persistentVolumeClaim":{"claimName":"pg"}},
            {"name":"seed","cloudInitNoCloud":{}}
        ]});
        let i = images(&spec);
        assert_eq!(i.len(), 3);
        assert_eq!(i[0]["kind"], "instance of registry image");
        assert!(i[0]["digest"].is_null());
        assert_eq!(i[1]["digest"], format!("sha256:{hex}"));
        let g = gaps(&i);
        assert_eq!(g[0]["what"], "image digest");
        assert_eq!(g.len(), 1, "a machine's traffic is on its Metrics tab (#14)");
    }
}
