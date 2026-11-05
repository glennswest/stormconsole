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
    if text(d, "kind") == "nvme_ssd" {
        return Some("NVMe: namespace format is not implemented".into());
    }
    None
}

/// One node's two answers → one record per drive. `objects` is the Drive list,
/// `None` from a stormdrive older than v0.39.0: no plan and no progress record,
/// which the node's status says rather than reading as "never formatted".
pub fn reduce(drives: &Value, objects: Option<&Value>, node: &str, prefix: &str) -> Vec<Value> {
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
            Some(json!({
                "component": format!("{prefix}:drive:{id}"),
                "node": node,
                "id": id,
                "name": text(d, "name"),
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
                "whyNot": why_not(d),
                "format": st.get("format"),
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

/// The requests to make: one per node and format, because stormdrive checks
/// one request all-or-nothing and takes one format in it.
pub fn batches(wanted: &[Wanted]) -> Vec<(String, u32, String, Vec<String>)> {
    let mut out: Vec<(String, u32, String, Vec<String>)> = Vec::new();
    for w in wanted {
        match out.iter_mut().find(|b| b.0 == w.node && b.1 == w.block_size && b.2 == w.protection) {
            Some(b) => {
                if !b.3.contains(&w.id) {
                    b.3.push(w.id.clone());
                }
            }
            None => out.push((w.node.clone(), w.block_size, w.protection.clone(), vec![w.id.clone()])),
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
        let r = reduce(&drives(), Some(&objects()), "here", "drive");
        assert_eq!(r.len(), 2, "a drive with no id is not guessed at");
        assert_eq!(r[0]["component"], "drive:drive:a1");
        assert_eq!(r[0]["bay"], 2);
        assert_eq!(r[0]["planned"]["protection"], "type1");
        assert_eq!(r[0]["format"]["progressPct"], 12);
        assert_eq!(r[0]["offersPi"], true);
        assert_eq!(r[0]["resets"][0], "host3: IOC reset count 0 → 1");
        assert!(r[0]["whyNot"].is_null());
        assert_eq!(r[1]["slabs"], 1);
        assert_eq!(r[1]["whyNot"], "in the fleet — leave (drain) first");
    }

    #[test]
    fn an_older_stormdrive_has_no_plan_and_still_lists_its_drives() {
        let r = reduce(&drives(), None, "here", "drive:@b");
        assert_eq!(r[0]["component"], "drive:@b:drive:a1");
        assert!(r[0]["planned"].is_null() && r[0]["format"].is_null());
    }

    #[test]
    fn stormdrives_refusals_are_said_before_anyone_asks() {
        let why = |v: Value| why_not(&v);
        assert!(why(json!({"membership": "out", "activity": "idle", "designation": "none"})).is_none());
        assert!(why(json!({"in_use_by": "lvm"})).unwrap().contains("holds data for lvm"));
        assert!(why(json!({"activity": "formatting"})).unwrap().contains("already running"));
        assert!(why(json!({"activity": "testing"})).unwrap().contains("testing"));
        assert!(why(json!({"designation": "system"})).unwrap().contains("system"));
        assert!(why(json!({"designation": "reserved"})).unwrap().contains("reserved"));
        assert!(why(json!({"designation": "spare"})).is_none());
        assert!(why(json!({"kind": "nvme_ssd"})).unwrap().contains("NVMe"));
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
        let b = batches(&[w("a", "1", 4096, "type1"), w("b", "2", 4096, "type1"), w("a", "3", 4096, "type1"), w("a", "4", 4096, "none"), w("a", "1", 4096, "type1")]);
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].3, vec!["1".to_string(), "3".to_string()], "and a drive once");
        assert_eq!(b[2].2, "none");
        assert!(check(&w("a", "1", 520, "none")).is_err());
        assert!(check(&w("a", "1", 4096, "type2")).is_err());
        assert!(check(&w("a", "1", 512, "none")).is_ok());
    }
}
