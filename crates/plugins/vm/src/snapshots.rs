//! A machine's snapshots: the Backup tab (#25).
//!
//! The objects are KubeVirt's — `snapshot.kubevirt.io/v1beta1`
//! `VirtualMachineSnapshot` and `VirtualMachineRestore` — so `virtctl` and
//! `oc get vmsnapshot` see exactly what the console made. Their status is
//! the shape stormvm writes (`stormvm-spec` `snapshot.rs`): `phase`
//! (`InProgress`, `Succeeded`, `Failed`), `readyToUse`, `indications`
//! (`Online`, `GuestAgent`, `NoGuestAgent`, `QuiesceFailed`),
//! `error.message`, `creationTime`, and `virtualMachineSnapshotContentName`,
//! which here names the stormblock group snapshot itself.
//!
//! The button *schedules*: it creates the object and returns. The node
//! does the work — idle the filesystems, pause, group-clone every disk,
//! resume — and writes the status, which the watch brings back.
//!
//! What the status does not carry yet, and is read when it does
//! (stormvm#45): the step it is on (`storm.io/step`), the disks it took
//! (`snapshotVolumes.includedVolumes`, KubeVirt's field) and their size
//! (`storm.io/sizeBytes`). Absent, each is said to be unreported rather
//! than shown as nothing.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

pub const API: &str = "/apis/snapshot.kubevirt.io/v1beta1";
pub const NOTE: &str = "storm.io/note";

/// How long an object may sit with no status before the tab stops saying
/// "scheduled" and starts saying nothing has picked it up.
const UNCLAIMED_SECS: i64 = 60;

fn s<'a>(v: &'a Value, p: &str) -> Option<&'a str> {
    v.pointer(p).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Does this snapshot or restore object name `name` in its source/target?
fn names_vm(obj: &Value, field: &str, name: &str) -> bool {
    s(obj, &format!("/spec/{field}/name")) == Some(name)
}

pub fn is_for(snapshot: &Value, vm: &str) -> bool {
    names_vm(snapshot, "source", vm)
}

pub fn restore_is_for(restore: &Value, vm: &str) -> bool {
    names_vm(restore, "target", vm)
}

fn step_label(step: &str) -> String {
    match step {
        "idling" => "idling filesystems".into(),
        "freezing" => "freezing".into(),
        "cloning" => "cloning disks".into(),
        "thawing" => "thawing".into(),
        "recording" => "recording".into(),
        other => other.into(),
    }
}

fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

/// One row of the tab. `now` is passed in so the "nothing has picked it
/// up" answer is testable.
pub fn row(obj: &Value, now: DateTime<Utc>) -> Value {
    let name = s(obj, "/metadata/name").unwrap_or_default();
    let created = s(obj, "/metadata/creationTimestamp");
    let phase = s(obj, "/status/phase");
    let ready = obj.pointer("/status/readyToUse").and_then(Value::as_bool).unwrap_or(false);
    let step = s(obj, "/status/storm.io~1step");
    let error = s(obj, "/status/error/message");
    let age = created
        .and_then(|c| DateTime::parse_from_rfc3339(c).ok())
        .map(|c| (now - c.with_timezone(&Utc)).num_seconds());

    // One word for the badge, one sentence for what it means.
    let (state, say) = match (phase, ready) {
        (Some("Succeeded"), true) => ("ready", "ready to restore from".to_string()),
        (Some("Succeeded"), false) => ("failed", "finished, and not marked ready to use".into()),
        (Some("Failed"), _) => (
            "failed",
            match (step, error) {
                (Some(st), Some(e)) => format!("failed while {}: {e}", step_label(st)),
                (None, Some(e)) => format!("failed: {e}"),
                (Some(st), None) => format!("failed while {}", step_label(st)),
                (None, None) => "failed, and the node gave no reason".into(),
            },
        ),
        (Some("InProgress"), _) => (
            "progress",
            match step {
                Some(st) => format!("{}…", step_label(st)),
                None => "in progress. The node does not report which step yet (stormvm#45)".into(),
            },
        ),
        (Some(other), _) => ("progress", other.to_string()),
        // Created, and nothing has written a status. For a minute that is
        // "scheduled"; after it, the honest answer is that nothing on this
        // cluster has picked it up — which today is the case everywhere.
        (None, _) if age.is_some_and(|a| a >= UNCLAIMED_SECS) => (
            "waiting",
            format!(
                "not picked up after {}. Nothing on this node takes snapshots yet: the kubelet's \
                 snapshot controller is rustkube-node#53",
                ago(age.unwrap_or(0))
            ),
        ),
        (None, _) => ("scheduled", "scheduled; waiting for the node".into()),
    };

    let disks: Option<Vec<&str>> = obj
        .pointer("/status/snapshotVolumes/includedVolumes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect());
    let size = obj.pointer("/status/storm.io~1sizeBytes").and_then(Value::as_u64);
    json!({
        "name": name,
        "created": created,
        "taken": s(obj, "/status/creationTime"),
        "state": state,
        "say": say,
        "phase": phase,
        "step": step.map(step_label),
        "ready": ready,
        "indications": obj.pointer("/status/indications").cloned().unwrap_or(json!([])),
        "disks": disks,
        "size": size.map(human_bytes),
        "content": s(obj, "/status/virtualMachineSnapshotContentName"),
        "note": s(obj, &format!("/metadata/annotations/{}", NOTE.replace('/', "~1"))),
        "deleting": obj.pointer("/metadata/deletionTimestamp").is_some(),
    })
}

fn ago(secs: i64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{} min", s / 60),
        s => format!("{} h", s / 3600),
    }
}

/// A restore's row: which snapshot, done or not, and why not.
pub fn restore_row(obj: &Value) -> Value {
    let complete = obj.pointer("/status/complete").and_then(Value::as_bool).unwrap_or(false);
    let failed = obj
        .pointer("/status/conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|c| c["type"] == "Ready" && c["status"] == "False")
        .and_then(|c| c["reason"].as_str())
        .filter(|r| !matches!(*r, "not ready" | "restoring" | ""));
    let (state, say) = if complete {
        ("ready", "restored".to_string())
    } else if let Some(r) = failed {
        ("failed", r.to_string())
    } else if obj.get("status").is_some_and(|s| !s.is_null()) {
        ("progress", "restoring…".to_string())
    } else {
        ("scheduled", "scheduled; waiting for the node".into())
    };
    json!({
        "name": s(obj, "/metadata/name"),
        "snapshot": s(obj, "/spec/virtualMachineSnapshotName"),
        "created": s(obj, "/metadata/creationTimestamp"),
        "restored": s(obj, "/status/restoreTime"),
        "state": state,
        "say": say,
    })
}

/// What the Snapshot button sends.
#[derive(Debug, Default, Deserialize)]
pub struct Take {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub note: String,
}

/// A DNS-1123 name: what the apiserver will accept for `metadata.name`.
fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 253
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && n.starts_with(|c: char| c.is_ascii_alphanumeric())
        && n.ends_with(|c: char| c.is_ascii_alphanumeric())
}

/// The `VirtualMachineSnapshot` to create. `defined` says whether the
/// machine has a `VirtualMachine`; a bare instance is snapshotted as one,
/// which stormvm accepts, since its disks are as worth keeping.
pub fn take_body(ns: &str, vm: &str, defined: bool, t: &Take, now: DateTime<Utc>) -> Result<Value, String> {
    let name = match t.name.trim() {
        "" => format!("{vm}-{}", now.format("%Y%m%d-%H%M%S")),
        n => n.to_string(),
    };
    if !valid_name(&name) {
        return Err(format!(
            "{name:?} is not a name the apiserver accepts: lowercase letters, digits, '-' and '.', \
             starting and ending with a letter or digit"
        ));
    }
    let mut meta = json!({"name": name, "namespace": ns});
    if !t.note.trim().is_empty() {
        meta["annotations"] = json!({ NOTE: t.note.trim() });
    }
    Ok(json!({
        "apiVersion": "snapshot.kubevirt.io/v1beta1",
        "kind": "VirtualMachineSnapshot",
        "metadata": meta,
        "spec": {
            "source": {
                "apiGroup": "kubevirt.io",
                "kind": if defined { "VirtualMachine" } else { "VirtualMachineInstance" },
                "name": vm,
            }
        }
    }))
}

/// The `VirtualMachineRestore` to create, or the sentence that says why
/// not. The node needs the target stopped and the snapshot succeeded
/// (rustkube-node#53); asking anyway would make an object that fails, so
/// the console refuses first and says what to do.
pub fn restore_body(
    ns: &str,
    vm: &str,
    defined: bool,
    running: bool,
    snapshot: Option<&Value>,
    snap_name: &str,
    now: DateTime<Utc>,
) -> Result<Value, String> {
    let Some(snap) = snapshot.filter(|o| is_for(o, vm)) else {
        return Err(format!("{vm} has no snapshot {snap_name}"));
    };
    if !defined {
        return Err(format!(
            "{vm} is a bare instance: there is no definition to restore into, and stopping it \
             deletes it. Restore needs a VirtualMachine"
        ));
    }
    if running {
        return Err(format!("stop {vm} first: a restore replaces its disks, and it needs the machine stopped"));
    }
    if !snap.pointer("/status/readyToUse").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("{snap_name} is not ready to restore from ({})", row(snap, now)["say"].as_str().unwrap_or("")));
    }
    Ok(json!({
        "apiVersion": "snapshot.kubevirt.io/v1beta1",
        "kind": "VirtualMachineRestore",
        "metadata": {"name": format!("{snap_name}-restore-{}", now.format("%Y%m%d-%H%M%S")), "namespace": ns},
        "spec": {
            "target": {"apiGroup": "kubevirt.io", "kind": "VirtualMachine", "name": vm},
            "virtualMachineSnapshotName": snap_name,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(t).unwrap().with_timezone(&Utc)
    }

    fn snap(status: Value) -> Value {
        json!({
            "metadata": {"name": "before-upgrade", "namespace": "default",
                         "creationTimestamp": "2026-09-25T12:00:00Z",
                         "annotations": {"storm.io/note": "pre 10.1"}},
            "spec": {"source": {"apiGroup": "kubevirt.io", "kind": "VirtualMachine", "name": "web-1"}},
            "status": status
        })
    }

    #[test]
    fn the_button_schedules_a_kubevirt_snapshot_named_by_the_clock() {
        let b = take_body("default", "web-1", true, &Take::default(), at("2026-09-25T12:34:56Z")).unwrap();
        assert_eq!(b["kind"], "VirtualMachineSnapshot");
        assert_eq!(b["apiVersion"], "snapshot.kubevirt.io/v1beta1");
        assert_eq!(b["metadata"]["name"], "web-1-20260925-123456");
        assert_eq!(b["spec"]["source"]["kind"], "VirtualMachine");
        assert!(b["metadata"].get("annotations").is_none(), "no note, no annotation");

        let t = Take { name: "pre-upgrade".into(), note: " before 10.1 ".into() };
        let b = take_body("default", "web-1", false, &t, Utc::now()).unwrap();
        assert_eq!(b["metadata"]["name"], "pre-upgrade");
        assert_eq!(b["metadata"]["annotations"]["storm.io/note"], "before 10.1");
        assert_eq!(b["spec"]["source"]["kind"], "VirtualMachineInstance", "a bare instance is its own source");

        let bad = Take { name: "Pre Upgrade".into(), ..Default::default() };
        assert!(take_body("default", "web-1", true, &bad, Utc::now()).unwrap_err().contains("lowercase"));
    }

    #[test]
    fn the_row_says_what_state_it_is_in() {
        let now = at("2026-09-25T12:00:20Z");
        let r = row(&snap(Value::Null), now);
        assert_eq!(r["state"], "scheduled");
        assert_eq!(r["note"], "pre 10.1");

        let r = row(&snap(json!({"phase": "InProgress", "readyToUse": false, "storm.io/step": "cloning"})), now);
        assert_eq!(r["state"], "progress");
        assert_eq!(r["say"], "cloning disks…");

        let r = row(&snap(json!({"phase": "InProgress", "readyToUse": false})), now);
        assert!(r["say"].as_str().unwrap().contains("stormvm#45"), "a missing step is named, not blank");

        let r = row(
            &snap(json!({"phase": "Succeeded", "readyToUse": true, "creationTime": "2026-09-25T12:00:03Z",
                         "indications": ["Online", "GuestAgent"],
                         "snapshotVolumes": {"includedVolumes": ["root", "data"]},
                         "storm.io/sizeBytes": 3221225472u64,
                         "virtualMachineSnapshotContentName": "gsnap-7"})),
            now,
        );
        assert_eq!(r["state"], "ready");
        assert_eq!(r["disks"], json!(["root", "data"]));
        assert_eq!(r["size"], "3.0 GB");
        assert_eq!(r["content"], "gsnap-7");
    }

    #[test]
    fn a_failure_names_the_step_and_the_reason() {
        let r = row(
            &snap(json!({"phase": "Failed", "readyToUse": false, "storm.io/step": "freezing",
                         "error": {"message": "guest agent did not answer"}})),
            Utc::now(),
        );
        assert_eq!(r["state"], "failed");
        assert_eq!(r["say"], "failed while freezing: guest agent did not answer");
        assert_eq!(r["disks"], Value::Null, "unreported, not zero disks");
        assert_eq!(r["size"], Value::Null);
    }

    #[test]
    fn nothing_picking_it_up_is_said_after_a_minute() {
        let r = row(&snap(Value::Null), at("2026-09-25T12:05:00Z"));
        assert_eq!(r["state"], "waiting");
        assert!(r["say"].as_str().unwrap().contains("rustkube-node#53"));
        assert!(r["say"].as_str().unwrap().contains("5 min"));
    }

    #[test]
    fn restore_is_refused_until_it_could_work() {
        let now = Utc::now();
        let ready = snap(json!({"phase": "Succeeded", "readyToUse": true}));
        let pending = snap(json!({"phase": "InProgress", "readyToUse": false}));
        let r = |defined, running, s: Option<&Value>| restore_body("default", "web-1", defined, running, s, "before-upgrade", now);
        assert!(r(true, true, Some(&ready)).unwrap_err().starts_with("stop web-1 first"));
        assert!(r(false, false, Some(&ready)).unwrap_err().contains("bare instance"));
        assert!(r(true, false, Some(&pending)).unwrap_err().contains("not ready"));
        assert!(r(true, false, None).unwrap_err().contains("no snapshot"));
        // Another machine's snapshot is not this one's to restore from.
        let mut other = ready.clone();
        other["spec"]["source"]["name"] = json!("db-1");
        assert!(r(true, false, Some(&other)).unwrap_err().contains("no snapshot"));

        let b = r(true, false, Some(&ready)).unwrap();
        assert_eq!(b["kind"], "VirtualMachineRestore");
        assert_eq!(b["spec"]["target"]["name"], "web-1");
        assert_eq!(b["spec"]["virtualMachineSnapshotName"], "before-upgrade");
    }

    #[test]
    fn a_restore_row_reads_its_conditions() {
        let base = json!({"metadata": {"name": "undo"}, "spec": {"virtualMachineSnapshotName": "before-upgrade"}});
        assert_eq!(restore_row(&base)["state"], "scheduled");
        let mut done = base.clone();
        done["status"] = json!({"complete": true, "restoreTime": "2026-09-25T12:10:00Z"});
        assert_eq!(restore_row(&done)["state"], "ready");
        let mut bad = base.clone();
        bad["status"] = json!({"complete": false, "conditions": [
            {"type": "Progressing", "status": "False", "reason": "restoring"},
            {"type": "Ready", "status": "False", "reason": "target web-1 is running"}]});
        let r = restore_row(&bad);
        assert_eq!(r["state"], "failed");
        assert_eq!(r["say"], "target web-1 is running");
    }
}
