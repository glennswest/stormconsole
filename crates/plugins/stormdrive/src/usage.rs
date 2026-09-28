//! Each drive's usage, in bytes, from every node (#29).
//!
//! stormdrive's feed says how full a drive is in words ("used 1.2 TB"),
//! which is right for a row and wrong for a page that adds 1,600 of them
//! up. Since stormdrive v0.13.0 (stormdrive#12, #13) `GET /api/v1/drives`
//! carries the numbers themselves: capacity, what the engine's slabs take,
//! what is used and free in them, what lies outside any slab, what the
//! overcommit setting lets the slabs promise, and each slab on the drive.
//! A drain in progress rides on the same record.
//!
//! This is read **on demand**, not on the feed's 5 s poll: nobody needs
//! 1,600 drives' slab lists pushed to every open tab, and the one page that
//! does asks for them. Answers are kept for [`FRESH`], so a page polling
//! every few seconds costs every node one request per window, however many
//! tabs are open.

use serde_json::{json, Value};

/// How long an answer is reused.
pub const FRESH: std::time::Duration = std::time::Duration::from_secs(10);

/// A u64 field, or None when absent or null.
fn n(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

/// The drive's id as stormdrive serialises it — a bare string, or a
/// newtype some versions wrap in an object.
fn drive_id(d: &Value) -> Option<String> {
    match d.get("id") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(o)) => o.values().find_map(|v| v.as_str().map(str::to_string)),
        _ => None,
    }
}

/// One node's `GET /api/v1/drives` → one record per drive, keyed by the
/// component id the console serves that drive under (`<prefix>:drive:<id>`),
/// so the page joins without guessing.
pub fn reduce(body: &Value, prefix: &str, node: &str) -> Vec<Value> {
    let drives = body.get("drives").and_then(Value::as_array).cloned().unwrap_or_default();
    drives
        .iter()
        .filter_map(|d| {
            let id = drive_id(d)?;
            let mut r = json!({
                "component": format!("{prefix}:drive:{id}"),
                "node": node,
                "serial": d.get("serial").and_then(Value::as_str).unwrap_or(""),
                "wwn": d.get("wwid").and_then(Value::as_str),
                "capacity": n(d, "capacity_bytes"),
            });
            if let Some(u) = d.get("usage").filter(|u| u.is_object()) {
                let slabs: Vec<Value> = u
                    .get("slabs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|s| {
                        json!({
                            "id": s.get("id").and_then(Value::as_str).unwrap_or(""),
                            "role": s.get("role").and_then(Value::as_str).unwrap_or(""),
                            "tier": s.get("tier").and_then(Value::as_str).unwrap_or(""),
                            "total": n(s, "total_bytes").unwrap_or(0),
                            "used": n(s, "allocated_bytes").unwrap_or(0),
                            "free": n(s, "free_bytes").unwrap_or(0),
                            "committed": n(s, "committed_bytes"),
                        })
                    })
                    .collect();
                r["usage"] = json!({
                    "capacity": n(u, "capacity_bytes").unwrap_or(0),
                    "in_slabs": n(u, "in_slabs_bytes").unwrap_or(0),
                    "used": n(u, "used_bytes").unwrap_or(0),
                    "free_in_slabs": n(u, "free_in_slabs_bytes").unwrap_or(0),
                    "outside_slabs": n(u, "outside_slabs_bytes").unwrap_or(0),
                    "free": n(u, "free_bytes").unwrap_or(0),
                    "promisable": n(u, "promisable_bytes").unwrap_or(0),
                    "committed": n(u, "committed_bytes"),
                    "headroom": n(u, "headroom_bytes"),
                    "slabs": slabs,
                });
            }
            if let Some(o) = d.get("overcommit").filter(|o| o.is_object()) {
                r["overcommit"] = json!({
                    "enabled": o.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                    "ratio": o.get("ratio").and_then(Value::as_f64).unwrap_or(1.0),
                });
            }
            if let Some(dr) = d.get("drain").filter(|x| x.is_object()) {
                r["drain"] = json!({
                    "state": dr.get("state").and_then(Value::as_str).unwrap_or(""),
                    "moved": n(dr, "moved").unwrap_or(0),
                    "failed": n(dr, "failed").unwrap_or(0),
                    "remaining": n(dr, "remaining").unwrap_or(0),
                    "reason": dr.get("reason").and_then(Value::as_str).unwrap_or(""),
                    "then_leave": dr.get("then_leave").and_then(Value::as_bool).unwrap_or(false),
                    "errors": dr.get("errors").cloned().unwrap_or(json!([])),
                });
            }
            Some(r)
        })
        .collect()
}

/// What one node answered, as the page is told it.
pub fn node_answer(node: &str, result: Result<Value, String>, prefix: &str) -> (Vec<Value>, Value) {
    match result {
        Ok(body) => {
            let drives = reduce(&body, prefix, node);
            // A stormdrive older than v0.13.0 serialises no usage at all:
            // said per node, so the page does not read "no slabs" into it.
            let reported = drives.iter().filter(|d| d.get("usage").is_some()).count();
            (drives.clone(), json!({"node": node, "ok": true, "drives": drives.len(), "with_usage": reported}))
        }
        Err(e) => (vec![], json!({"node": node, "ok": false, "error": e})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        json!({"drives": [
            {"id": "7f0c", "serial": "SN1", "wwid": "naa.5000c500", "capacity_bytes": 4000,
             "overcommit": {"enabled": true, "ratio": 2.0},
             "drain": {"state": "running", "moved": 3, "failed": 0, "remaining": 5, "errors": [],
                       "reason": "operator", "then_leave": true},
             "usage": {"capacity_bytes": 4000, "in_slabs_bytes": 3000, "used_bytes": 1000,
                       "free_in_slabs_bytes": 2000, "outside_slabs_bytes": 1000, "free_bytes": 3000,
                       "promisable_bytes": 6000, "committed_bytes": 2500, "headroom_bytes": 3500,
                       "collected_at": {"secs_since_epoch": 1},
                       "slabs": [{"id": "s1", "role": "data", "tier": "hot", "slot_size": 1,
                                  "total_bytes": 3000, "allocated_bytes": 1000, "free_bytes": 2000,
                                  "committed_bytes": 2500}]}},
            {"id": "9a", "serial": "SN2", "capacity_bytes": 8000},
            {"serial": "no-id"}
        ]})
    }

    #[test]
    fn a_drive_is_keyed_by_the_component_the_console_serves_it_as() {
        let r = reduce(&body(), "drive:@storm-b", "storm-b");
        assert_eq!(r.len(), 2, "a drive with no id cannot be joined, so it is not guessed at");
        assert_eq!(r[0]["component"], "drive:@storm-b:drive:7f0c");
        assert_eq!(r[0]["node"], "storm-b");
        assert_eq!(r[0]["wwn"], "naa.5000c500");
    }

    #[test]
    fn usage_overcommit_and_drain_are_carried_in_bytes() {
        let r = &reduce(&body(), "drive", "here")[0];
        let u = &r["usage"];
        assert_eq!(u["used"], 1000);
        assert_eq!(u["outside_slabs"], 1000);
        assert_eq!(u["headroom"], 3500);
        assert_eq!(u["slabs"][0]["used"], 1000, "allocated is what the slab has used");
        assert_eq!(u["slabs"][0]["committed"], 2500);
        assert_eq!(r["overcommit"]["ratio"], 2.0);
        assert_eq!(r["drain"]["remaining"], 5);
        assert_eq!(r["drain"]["then_leave"], true);
    }

    #[test]
    fn a_drive_stormdrive_has_no_usage_for_says_nothing_rather_than_zero() {
        let r = &reduce(&body(), "drive", "here")[1];
        assert!(r.get("usage").is_none());
        assert!(r.get("drain").is_none());
        assert_eq!(r["capacity"], 8000);
    }

    #[test]
    fn a_node_that_did_not_answer_is_named() {
        let (d, s) = node_answer("storm-c", Err("connection refused".into()), "drive:@storm-c");
        assert!(d.is_empty());
        assert_eq!(s["ok"], false);
        let (d, s) = node_answer("here", Ok(body()), "drive");
        assert_eq!(d.len(), 2);
        assert_eq!(s["with_usage"], 1);
    }

    #[test]
    fn an_id_wrapped_in_an_object_is_still_an_id() {
        let b = json!({"drives": [{"id": {"0": "abc"}, "serial": "S"}]});
        assert_eq!(reduce(&b, "drive", "here")[0]["component"], "drive:drive:abc");
    }
}
