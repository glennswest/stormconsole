//! Formatting drives, from every node (#165).
//!
//! stormdrive holds all of it: what a drive is formatted as and what it would
//! be (`status.currentFormat`, `status.plannedFormat`), the run, which it
//! persists so it outlives a restart of stormdrive or the node (`status.format`,
//! stormdrive#108), and every run before it (a drive's history). So the console
//! keeps nothing: a page reload or a console restart reads the same answer.
//!
//! A format is destructive storage (#82): `POST /api/plugins/drive/format` is
//! behind the host's gate, goes to each node **as the viewer**, and asks for a
//! phrase on top, which this file checks against the serial it read.

use serde_json::{json, Value};

/// How long an answer is reused: progress moves slowly, and every open page
/// polling should cost each node one read.
pub const FRESH: std::time::Duration = std::time::Duration::from_secs(3);

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Why stormdrive would refuse to format this drive, in its words
/// (`Drive::format_blocker`), so the page can say so before anyone asks. Only
/// a preview: stormdrive checks again, and also what it alone can see (mounted
/// partitions, a data signature on the disk, what the drive offers).
pub fn why_not(d: &Value) -> Option<String> {
    // In use comes first (#308), in the words that say what depends on it.
    // The system drive is never formatted from the console.
    let u = crate::inuse::of(d);
    if u["in_use"] == true {
        let reason = u["reason"].as_str().unwrap_or("in use");
        return Some(if u["use"] == "system" {
            format!("{reason}. The system drive is never formatted from the console")
        } else {
            format!("in use: {reason}")
        });
    }
    if text(d, "membership") == "fleet" {
        return Some("in the fleet — leave (drain) first".into());
    }
    if let Some(who) = d.get("in_use_by").and_then(Value::as_str) {
        return Some(format!("in use: holds data for {who}"));
    }
    match text(d, "activity") {
        "formatting" => return Some("a format is already running".into()),
        "idle" | "" => {}
        other => return Some(format!("activity is {other}")),
    }
    match text(d, "designation") {
        "reserved" => return Some("designated reserved".into()),
        "system" => return Some("designated system: it holds the node's own data".into()),
        _ => {}
    }
    // An NVMe namespace is formatted too (#356), as stormdrive does it: a
    // worker job's Format NVM step (stormdrive#154), not FORMAT UNIT.
    None
}

pub fn is_nvme(d: &Value) -> bool {
    text(d, "kind") == "nvme_ssd"
}

/// `SystemTime` as serde writes it, in unix seconds.
fn unix(v: Option<&Value>) -> Option<u64> {
    v.and_then(|t| t.get("secs_since_epoch")).and_then(Value::as_u64)
}

/// The drive's part of the newest worker job that formats it (#356), as the
/// page's `format` record: an NVMe namespace's Format NVM runs only as a
/// worker step, so its run is in the job, not in `status.format`. Format NVM
/// is one command the drive does not report progress on: no percent, no
/// estimate. `None` when no job kept by stormdrive (its last 64) formats it.
pub fn worker_run(jobs: &Value, id: &str, block_size_now: Option<u64>) -> Option<Value> {
    let (job, dj, step) = jobs
        .get("jobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|j| {
            let steps = j.get("steps").and_then(Value::as_array)?;
            let step = steps.iter().find(|s| s.get("op").and_then(Value::as_str) == Some("format"))?;
            let dj = j.get("drives").and_then(Value::as_array)?.iter().find(|d| d.get("drive").and_then(Value::as_str) == Some(id))?;
            Some((j, dj, step))
        })
        .max_by_key(|(j, _, _)| unix(j.get("created")).unwrap_or(0))?;
    let raw = dj.get("state").and_then(Value::as_str).unwrap_or("");
    let (state, phase) = match raw {
        "queued" => ("running", "queued"),
        "running" => ("running", "formatting"),
        other => (other, other),
    };
    let to = step.get("block_size").and_then(Value::as_u64);
    Some(json!({
        "method": "nvm",
        "job": job.get("id"),
        "state": state,
        "phase": phase,
        // The worker's own words for where the drive is: "format nvm lbaf 2 (was …)".
        "detail": dj.get("phase").and_then(Value::as_str).filter(|p| !p.is_empty() && *p != raw),
        "progressPct": Value::Null,
        "startedUnix": unix(dj.get("started")),
        "finishedUnix": unix(dj.get("finished")),
        "toBlockSize": to,
        "protection": "none",
        "error": dj.get("error"),
        "done": dj.get("done").cloned().unwrap_or(json!([])),
        // stormdrive reads the geometry back after Format NVM and keeps it as
        // the drive's block size.
        "blockSizeAfter": if raw == "done" { block_size_now.map(Value::from).unwrap_or(Value::Null) } else { Value::Null },
        "protTypeAfter": if raw == "done" { json!(0) } else { Value::Null },
    }))
}

/// One node's answers → one record per drive. `objects` is the Drive list,
/// `None` from a stormdrive older than v0.39.0: no plan and no progress record,
/// which the node's status says rather than reading as "never formatted".
/// `jobs` is its worker jobs, where an NVMe namespace's format runs (#356).
pub fn reduce(drives: &Value, objects: Option<&Value>, jobs: Option<&Value>, node: &str, prefix: &str) -> Vec<Value> {
    let status_of = |id: &str| -> Value {
        objects
            .and_then(|o| o.get("items"))
            .and_then(Value::as_array)
            .and_then(|items| items.iter().find(|i| i.pointer("/metadata/name").and_then(Value::as_str) == Some(id)))
            .and_then(|i| i.get("status"))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let list = drives.get("drives").and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter()
        .filter_map(|d| {
            let id = crate::usage::drive_id(d)?;
            let st = status_of(&id);
            let slabs = d.pointer("/usage/slabs").and_then(Value::as_array).map_or(0, Vec::len);
            let nvme = is_nvme(d);
            let run = if nvme { jobs.and_then(|j| worker_run(j, &id, d.get("block_size").and_then(Value::as_u64))) } else { None };
            Some(json!({
                "component": format!("{prefix}:drive:{id}"),
                "node": node,
                "id": id,
                "name": text(d, "name"),
                "kind": text(d, "kind"),
                "model": text(d, "model"),
                "serial": text(d, "serial"),
                "wwn": d.get("wwid"),
                "capacity": d.get("capacity_bytes"),
                "bay": d.pointer("/location/bay"),
                "enclosure": st.get("enclosure"),
                "blockSize": d.get("block_size"),
                "current": st.get("currentFormat"),
                "planned": st.get("plannedFormat"),
                "offersPi": st.pointer("/supports/pi_types").and_then(Value::as_array).is_some_and(|t| t.contains(&json!(1))),
                "slabs": slabs,
                "use": crate::inuse::of(d),
                // What the drive can be formatted to (stormdrive pi.rs): the
                // Drive object's, else the REST answer's.
                "supports": st.get("supports").filter(|s| s.is_object()).or_else(|| d.get("supports")),
                "whyNot": why_not(d),
                "format": run.or_else(|| st.get("format").cloned()),
                // A controller reset is only on the live run, not the record.
                "resets": d.pointer("/format_run/resets").cloned().unwrap_or(json!([])),
            }))
        })
        .collect()
}

/// What a drive's history says about its formats, newest first.
pub fn runs(history: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = history
        .get("log")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| e.pointer("/action/op").and_then(Value::as_str) == Some("format"))
        .map(|e| {
            let a = &e["action"];
            json!({
                "at": e.get("at"),
                "result": a.get("result"),
                "error": a.get("error"),
                "requester": a.get("requester"),
                "params": a.get("params"),
                "before": a.get("before"),
                "after": a.get("after"),
            })
        })
        .collect();
    out.reverse();
    out
}

/// The phrase that confirms a format: a drive's serial, read off the screen,
/// or for several how many are about to lose their data.
pub fn phrase(serials: &[&str]) -> String {
    match serials {
        [one] if !one.is_empty() => (*one).to_string(),
        many => format!("format {} drives", many.len()),
    }
}

/// One drive to format, as the page asks.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Wanted {
    pub node: String,
    pub id: String,
    pub block_size: u32,
    pub protection: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct Start {
    pub drives: Vec<Wanted>,
    #[serde(default)]
    pub confirm: String,
}

/// Is this what stormdrive formats? Said here so a bad request reaches no node.
pub fn check(w: &Wanted) -> Result<(), String> {
    if !matches!(w.block_size, 512 | 4096) {
        return Err(format!("block size {}: use 512 or 4096", w.block_size));
    }
    if !matches!(w.protection.as_str(), "none" | "type1") {
        return Err(format!("protection {:?}: use none or type1", w.protection));
    }
    Ok(())
}

/// The drives a worker job refused at submit, with stormdrive's reason. The
/// job runs the others: unlike `POST /api/v1/format`, it is not all-or-nothing.
pub fn refused(job: &Value) -> Vec<Value> {
    job.get("drives")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|d| d.get("state").and_then(Value::as_str) == Some("refused"))
        .map(|d| json!({"drive": d.get("drive"), "name": d.get("name"), "error": d.get("error")}))
        .collect()
}

/// What an NVMe namespace is formatted to (#356): an LBA format of that data
/// size with no metadata. stormdrive's own refusal, said before a node is asked.
pub fn check_nvme(w: &Wanted) -> Result<(), String> {
    if w.protection != "none" {
        return Err("NVMe: PI formats are SCSI only here; NVMe end-to-end protection is not built in stormdrive".into());
    }
    Ok(())
}

/// One request to a node: FORMAT UNIT (`POST /api/v1/format`) for SCSI
/// drives, a worker job with a Format NVM step for NVMe namespaces.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub node: String,
    pub block_size: u32,
    pub protection: String,
    pub nvme: bool,
    pub ids: Vec<String>,
}

impl Batch {
    /// Where it goes on the node, and the body.
    pub fn request(&self) -> (&'static str, Value) {
        if self.nvme {
            ("/api/v1/worker/jobs", json!({"select": {"drives": self.ids}, "steps": [{"op": "format", "block_size": self.block_size}]}))
        } else {
            ("/api/v1/format", json!({"drives": self.ids, "block_size": self.block_size, "protection": self.protection}))
        }
    }
}

/// The requests to make: one per node, format and kind, because stormdrive
/// checks one request all-or-nothing and takes one format in it, and an NVMe
/// namespace is formatted by a worker job (#356). `nvme` says which drives are.
pub fn batches(wanted: &[Wanted], nvme: impl Fn(&Wanted) -> bool) -> Vec<Batch> {
    let mut out: Vec<Batch> = Vec::new();
    for w in wanted {
        let n = nvme(w);
        match out.iter_mut().find(|b| b.node == w.node && b.block_size == w.block_size && b.protection == w.protection && b.nvme == n) {
            Some(b) => {
                if !b.ids.contains(&w.id) {
                    b.ids.push(w.id.clone());
                }
            }
            None => out.push(Batch { node: w.node.clone(), block_size: w.block_size, protection: w.protection.clone(), nvme: n, ids: vec![w.id.clone()] }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drives() -> Value {
        json!({"drives": [
            {"id": "a1", "name": "sdb", "model": "H7210A521SUN010T", "serial": "SN1", "wwid": "naa.1",
             "capacity_bytes": 10, "block_size": 520, "membership": "out", "activity": "idle", "designation": "none",
             "kind": "sas_hdd", "location": {"bay": 2},
             "format_run": {"resets": ["host3: IOC reset count 0 → 1"]}},
            {"id": "a2", "name": "sdc", "serial": "SN2", "membership": "fleet", "activity": "idle",
             "usage": {"slabs": [{"id": "s"}]}},
            {"serial": "no-id"}
        ]})
    }
    fn objects() -> Value {
        json!({"items": [{"metadata": {"name": "a1"}, "status": {
            "enclosure": "shelf0",
            "currentFormat": {"blockSize": 520},
            "plannedFormat": {"blockSize": 4096, "protection": "type1", "basis": "try",
                              "fallbacks": [{"blockSize": 4096, "protection": "none"}]},
            "supports": {"pi_types": [1, 2]},
            "format": {"state": "running", "phase": "formatting", "progressPct": 12, "etaSeconds": 3600},
        }}]})
    }

    #[test]
    fn a_record_joins_the_rest_answer_and_the_drive_object() {
        let r = reduce(&drives(), Some(&objects()), None, "here", "drive");
        assert_eq!(r.len(), 2, "a drive with no id is not guessed at");
        assert_eq!(r[0]["component"], "drive:drive:a1");
        assert_eq!(r[0]["bay"], 2);
        assert_eq!(r[0]["planned"]["protection"], "type1");
        assert_eq!(r[0]["format"]["progressPct"], 12);
        assert_eq!(r[0]["offersPi"], true);
        assert_eq!(r[0]["resets"][0], "host3: IOC reset count 0 → 1");
        assert!(r[0]["whyNot"].is_null());
        assert_eq!(r[1]["slabs"], 1);
        assert_eq!(r[1]["whyNot"], "in use: sdc is in the storage fleet");
        assert_eq!(r[1]["use"]["badge"], "Slab member");
        assert_eq!(r[0]["supports"]["pi_types"][1], 2);
    }

    #[test]
    fn an_older_stormdrive_has_no_plan_and_still_lists_its_drives() {
        let r = reduce(&drives(), None, None, "here", "drive:@b");
        assert_eq!(r[0]["component"], "drive:@b:drive:a1");
        assert!(r[0]["planned"].is_null() && r[0]["format"].is_null());
    }

    #[test]
    fn stormdrives_refusals_are_said_before_anyone_asks() {
        let why = |v: Value| why_not(&v);
        assert!(why(json!({"membership": "out", "activity": "idle", "designation": "none"})).is_none());
        assert!(why(json!({"in_use_by": "lvm"})).unwrap().contains("holds data for lvm"));
        let sda = why(json!({"name": "sda", "slab_parts": [{"partition": 2, "name": "stormblock", "role": "system"}]})).unwrap();
        assert!(sda.starts_with("sda holds this node's system slab") && sda.ends_with("never formatted from the console"), "{sda}");
        assert!(why(json!({"activity": "formatting"})).unwrap().contains("already running"));
        assert!(why(json!({"activity": "testing"})).unwrap().contains("testing"));
        assert!(why(json!({"designation": "system"})).unwrap().contains("system"));
        assert!(why(json!({"designation": "reserved"})).unwrap().contains("reserved"));
        assert!(why(json!({"designation": "spare"})).is_none());
        assert!(why(json!({"kind": "nvme_ssd", "membership": "out", "activity": "idle"})).is_none(), "an NVMe namespace is formatted too (#356)");
    }

    #[test]
    fn the_history_keeps_formats_newest_first() {
        let h = json!({"log": [
            {"at": "t1", "action": {"op": "format", "result": "failed", "error": "e", "params": {"block_size": 4096}}},
            {"at": "t2", "kind": "moved"},
            {"at": "t3", "action": {"op": "test", "result": "done"}},
            {"at": "t4", "action": {"op": "format", "result": "done", "requester": "alice", "after": {"prot_type": 1}}},
        ]});
        let r = runs(&h);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["at"], "t4");
        assert_eq!(r[0]["requester"], "alice");
        assert_eq!(r[1]["error"], "e");
    }

    #[test]
    fn the_phrase_is_the_serial_or_the_count() {
        assert_eq!(phrase(&["SN1"]), "SN1");
        assert_eq!(phrase(&["SN1", "SN2", "SN3"]), "format 3 drives");
        assert_eq!(phrase(&[""]), "format 1 drives", "no serial, still a phrase");
    }

    #[test]
    fn one_request_per_node_and_format() {
        let w = |node: &str, id: &str, bs, p: &str| Wanted { node: node.into(), id: id.into(), block_size: bs, protection: p.into() };
        let b = batches(&[w("a", "1", 4096, "type1"), w("b", "2", 4096, "type1"), w("a", "3", 4096, "type1"), w("a", "4", 4096, "none"), w("a", "1", 4096, "type1")], |_| false);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].ids, vec!["1".to_string(), "3".to_string()], "and a drive once");
        assert_eq!(b[2].protection, "none");
        assert_eq!(b[0].request().0, "/api/v1/format");
        assert!(check(&w("a", "1", 520, "none")).is_err());
        assert!(check(&w("a", "1", 4096, "type2")).is_err());
        assert!(check(&w("a", "1", 512, "none")).is_ok());
    }

    #[test]
    fn an_nvme_namespace_goes_as_a_worker_job_format_nvm() {
        let w = |id: &str, bs| Wanted { node: "a".into(), id: id.into(), block_size: bs, protection: "none".into() };
        let b = batches(&[w("sas", 4096), w("nv1", 4096), w("nv2", 4096), w("nv3", 512)], |w| w.id.starts_with("nv"));
        assert_eq!(b.len(), 3, "SCSI and NVMe never share a request: {b:?}");
        assert!(!b[0].nvme && b[1].nvme && b[2].nvme);
        let (path, body) = b[1].request();
        assert_eq!(path, "/api/v1/worker/jobs");
        assert_eq!(body, json!({"select": {"drives": ["nv1", "nv2"]}, "steps": [{"op": "format", "block_size": 4096}]}), "no destroy, no iKnowInUse: stormdrive's guards stand");
        assert!(check_nvme(&w("nv1", 4096)).is_ok());
        let job = json!({"id": "j1", "drives": [{"drive": "nv1", "name": "nvme0n1", "state": "queued"},
                                                {"drive": "nv2", "name": "nvme1n1", "state": "refused", "error": "busy: testing"}]});
        assert_eq!(refused(&job), vec![json!({"drive": "nv2", "name": "nvme1n1", "error": "busy: testing"})]);
        let pi = Wanted { protection: "type1".into(), ..w("nv1", 4096) };
        assert!(check_nvme(&pi).unwrap_err().contains("SCSI only"));
    }

    fn t(secs: u64) -> Value {
        json!({"secs_since_epoch": secs, "nanos_since_epoch": 0})
    }

    #[test]
    fn the_newest_worker_job_is_the_namespaces_run() {
        let jobs = json!({"jobs": [
            {"id": "old", "created": t(100), "steps": [{"op": "format", "block_size": 512}],
             "drives": [{"drive": "n1", "state": "failed", "phase": "failed", "error": "no LBA format with 512-byte data and no metadata",
                         "started": t(101), "finished": t(102), "done": []}]},
            {"id": "new", "created": t(200), "steps": [{"op": "format", "block_size": 4096}],
             "drives": [{"drive": "n1", "state": "running", "phase": "format nvm lbaf 2 (was 512)", "progress_pct": null,
                         "started": t(201), "finished": null, "done": []}]},
            {"id": "other", "created": t(300), "steps": [{"op": "test", "kind": "smoke"}],
             "drives": [{"drive": "n1", "state": "done"}]},
        ]});
        let r = worker_run(&jobs, "n1", Some(512)).unwrap();
        assert_eq!((r["job"].as_str(), r["state"].as_str(), r["phase"].as_str()), (Some("new"), Some("running"), Some("formatting")));
        assert_eq!(r["detail"], "format nvm lbaf 2 (was 512)", "the worker's words");
        assert_eq!((r["toBlockSize"].as_u64(), r["startedUnix"].as_u64()), (Some(4096), Some(201)));
        assert!(r["progressPct"].is_null() && r["blockSizeAfter"].is_null(), "Format NVM gives no percent; nothing read back yet");
        assert!(worker_run(&jobs, "n2", None).is_none());

        let done = json!({"jobs": [{"id": "j", "created": t(1), "steps": [{"op": "format", "block_size": 4096}],
            "drives": [{"drive": "n1", "state": "done", "phase": "done", "started": t(2), "finished": t(9), "done": ["Format NVM to 4096-byte blocks"]}]}]});
        let r = worker_run(&done, "n1", Some(4096)).unwrap();
        assert_eq!((r["state"].as_str(), r["finishedUnix"].as_u64(), r["blockSizeAfter"].as_u64()), (Some("done"), Some(9), Some(4096)));
        assert_eq!(r["done"][0], "Format NVM to 4096-byte blocks");
        assert!(r["detail"].is_null(), "a phase that only repeats the state is not said twice");

        let q = json!({"jobs": [{"id": "j", "created": t(1), "steps": [{"op": "format", "block_size": 512}], "drives": [{"drive": "n1", "state": "queued", "phase": ""}]}]});
        assert_eq!(worker_run(&q, "n1", None).unwrap()["phase"], "queued", "queued behind its controller's lane: running, not started");
    }

    #[test]
    fn an_nvme_record_carries_its_job_and_a_scsi_one_its_object() {
        let drives = json!({"drives": [{"id": "n1", "name": "nvme0n1", "kind": "nvme_ssd", "serial": "NV1", "block_size": 512, "membership": "out", "activity": "idle"}]});
        let jobs = json!({"jobs": [{"id": "j", "created": t(1), "steps": [{"op": "format", "block_size": 4096}], "drives": [{"drive": "n1", "state": "running", "phase": "x"}]}]});
        let r = reduce(&drives, None, Some(&jobs), "here", "drive");
        assert_eq!((r[0]["kind"].as_str(), r[0]["format"]["method"].as_str()), (Some("nvme_ssd"), Some("nvm")));
        assert!(r[0]["whyNot"].is_null());
        let sas = reduce(&self::drives(), Some(&objects()), Some(&jobs), "here", "drive");
        assert_eq!(sas[0]["format"]["progressPct"], 12, "a SCSI drive's run stays its Drive object's");
    }
}
