//! A machine's metadata, the images it boots from, and what the node does
//! not report about either (#69) — the same questions the pod page
//! answers, asked of a VM.
//!
//! Read from rustkube-node (2026-10-02): the instance's status carries
//! `phase`, `nodeName`, `reason`, `message`, conditions and the
//! `storm.io/startedUnix` / `readyUnix` / `bootSeconds` marks; no image
//! digest for any disk and no interface counters. The counters are
//! stormvm#48 (in the guest) and rustkube-node#131 (the tap, as the
//! kubelet already counts a pod's interfaces).

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

/// Each disk's source image, and its digest where the reference carries
/// one.
pub fn images(spec: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    for v in spec.pointer("/volumes").and_then(Value::as_array).into_iter().flatten() {
        let name = v.get("name").and_then(Value::as_str).unwrap_or("");
        let (kind, source) = if let Some(i) = v.pointer("/containerDisk/image").and_then(Value::as_str) {
            ("container disk", i)
        } else if let Some(g) = v.pointer("/dataVolume/name").and_then(Value::as_str) {
            ("golden clone", g)
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
            "why": "the node reports no sha256 for the image or golden a disk was made from",
            "issue": plugin_kubernetes::pod::STATUS_ISSUE,
        }));
    }
    out.push(json!({
        "what": "traffic counters",
        "why": "nothing counts a machine's interfaces yet — neither the guest's (stormvm#48) nor the tap on the node",
        "issue": plugin_kubernetes::pod::NETWORK_ISSUE,
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(i[0]["kind"], "golden clone");
        assert!(i[0]["digest"].is_null());
        assert_eq!(i[1]["digest"], format!("sha256:{hex}"));
        let g = gaps(&i);
        assert_eq!(g[0]["what"], "image digest");
        assert_eq!(g[1]["what"], "traffic counters");
    }
}
