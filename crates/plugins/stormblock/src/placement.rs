//! The volumes on each drive (#29), from the engine's placement.
//!
//! "Which volumes am I about to lose if this drive goes" is the question a
//! drive page is opened for, and the engine answers it: since stormblock
//! v17.1.0 (#136) `GET /api/v1/volumes?placement=true` says, per volume,
//! every slab holding a leg of it, grouped by the drive the slab is on,
//! with the legs and bytes there and each slab's state (`ok`, `failed`,
//! `quarantined`, `draining`, `missing`). This turns that inside out:
//! per drive, the volumes on it, each with what it is and who uses it.
//!
//! It is a walk of every volume's extent map in the engine, so it is asked
//! for only when a page wants it, and the answer is reused for [`FRESH`].
//! A volume's *consumer* comes from the same listing (stormblock v18.1.0),
//! so a claim on a drive links to the claim.

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// How long an answer is reused: placement moves when data moves, which is
/// minutes, not seconds.
pub const FRESH: std::time::Duration = std::time::Duration::from_secs(15);

/// A drive's key: its serial, which stormdrive and the engine agree on;
/// the WWN or the path only when an engine names no serial.
fn drive_key(d: &Value) -> Option<String> {
    ["serial", "wwn", "path"]
        .iter()
        .find_map(|k| d.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()))
        .map(str::to_string)
}

/// The engine's volume listing with placement → per drive, the volumes on
/// it. `placed` counts the volumes that carried a placement at all: an
/// engine before v17.1.0 carries none, which is not "nothing is on any
/// drive".
pub fn by_drive(volumes: &[Value]) -> Value {
    let mut drives: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut placed = 0usize;
    for v in volumes {
        let Some(p) = v.get("placement").filter(|p| p.is_object()) else { continue };
        placed += 1;
        let id = super::field(v, &["id"]).unwrap_or_default();
        let (consumer, link) = match super::consumer(v) {
            Some((w, l)) => (Some(w), l),
            None => (None, None),
        };
        // The slabs of this volume, grouped by their drive, with each
        // slab's state for this volume and any drain moving legs off it.
        let mut slabs: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for s in p.get("slabs").and_then(Value::as_array).into_iter().flatten() {
            let Some(key) = s.get("drive").and_then(drive_key) else { continue };
            let mut e = json!({
                "id": s.get("id").and_then(Value::as_str).unwrap_or(""),
                "state": s.get("state").and_then(Value::as_str).unwrap_or("ok"),
                "legs": s.get("legs").and_then(Value::as_u64).unwrap_or(0),
                "shared_legs": s.get("shared_legs").and_then(Value::as_u64).unwrap_or(0),
                "bytes": s.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            });
            if let Some(d) = s.get("drain").filter(|d| d.is_object()) {
                e["drain"] = d.clone();
            }
            slabs.entry(key).or_default().push(e);
        }
        let legs = p.get("legs");
        for d in p.get("drives").and_then(Value::as_array).into_iter().flatten() {
            let Some(key) = d.get("drive").and_then(drive_key) else { continue };
            let on_drive = slabs.remove(&key).unwrap_or_default();
            // The worst state among this volume's slabs on this drive: one
            // failed leg here is the fact that matters.
            let state = ["missing", "failed", "quarantined", "draining"]
                .into_iter()
                .find(|w| on_drive.iter().any(|s| s["state"] == *w))
                .unwrap_or("ok");
            drives.entry(key).or_default().push(json!({
                "component": format!("sb:volume:{id}"),
                "id": id,
                "name": super::field(v, &["name"]).unwrap_or_else(|| id.clone()),
                "kind": super::volume_kind(v),
                "consumer": consumer,
                "consumer_link": link,
                "bytes": d.get("bytes").and_then(Value::as_u64).unwrap_or(0),
                "legs": d.get("legs").and_then(Value::as_u64).unwrap_or(0),
                // Legs shared with another volume (a clone and its golden):
                // on this drive, but not this volume's alone to lose.
                "shared_legs": on_drive.iter().filter_map(|s| s["shared_legs"].as_u64()).sum::<u64>(),
                "state": state,
                "slabs": on_drive,
                "policy": legs.and_then(|l| l.get("policy")).and_then(Value::as_str),
                "health": legs.and_then(|l| l.get("health")).and_then(Value::as_str),
                "rebuild": p.get("rebuild").and_then(Value::as_str).unwrap_or("none"),
            }));
        }
    }
    // Largest first: the volume that loses most when this drive goes.
    for list in drives.values_mut() {
        list.sort_by(|a, b| b["bytes"].as_u64().cmp(&a["bytes"].as_u64()));
    }
    json!({"volumes": volumes.len(), "placed": placed, "drives": drives})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(id: &str, drives: Value, slabs: Value) -> Value {
        json!({"id": id, "name": id, "kind": "volume", "in_use": true,
               "consumer": {"kind": "PersistentVolumeClaim", "namespace": "shop", "name": "db"},
               "placement": {"drives": drives, "slabs": slabs, "rebuild": "none",
                             "legs": {"policy": "mirror2", "health": "healthy"}}})
    }

    #[test]
    fn each_drive_lists_the_volumes_on_it_with_their_consumer() {
        let v = vol(
            "v1",
            json!([{"drive": {"serial": "SN1", "path": "/dev/sda"}, "slabs": 1, "legs": 4, "bytes": 4096},
                   {"drive": {"serial": "SN2", "path": "/dev/sdb"}, "slabs": 1, "legs": 4, "bytes": 4096}]),
            json!([{"id": "s1", "drive": {"serial": "SN1"}, "state": "ok", "legs": 4, "shared_legs": 3, "bytes": 4096},
                   {"id": "s2", "drive": {"serial": "SN2"}, "state": "draining", "legs": 4, "bytes": 4096,
                    "drain": {"state": "running", "moved": 1, "remaining": 3, "failed": 0}}]),
        );
        let out = by_drive(&[v]);
        assert_eq!(out["placed"], 1);
        let sn1 = &out["drives"]["SN1"][0];
        assert_eq!(sn1["component"], "sb:volume:v1");
        assert_eq!(sn1["consumer"], "PersistentVolumeClaim shop/db");
        assert_eq!(sn1["consumer_link"], "k8s:pvc:shop/db");
        assert_eq!(sn1["state"], "ok");
        assert_eq!(sn1["shared_legs"], 3);
        let sn2 = &out["drives"]["SN2"][0];
        assert_eq!(sn2["state"], "draining");
        assert_eq!(sn2["slabs"][0]["drain"]["remaining"], 3);
        assert_eq!(sn2["policy"], "mirror2");
    }

    #[test]
    fn the_worst_slab_state_on_a_drive_is_the_volumes_state_there() {
        let v = vol(
            "v1",
            json!([{"drive": {"serial": "SN1"}, "legs": 2, "bytes": 10}]),
            json!([{"id": "a", "drive": {"serial": "SN1"}, "state": "ok"},
                   {"id": "b", "drive": {"serial": "SN1"}, "state": "failed"}]),
        );
        assert_eq!(by_drive(&[v])["drives"]["SN1"][0]["state"], "failed");
    }

    #[test]
    fn an_engine_without_placement_is_not_an_empty_drive() {
        let out = by_drive(&[json!({"id": "v1", "kind": "volume"})]);
        assert_eq!(out["volumes"], 1);
        assert_eq!(out["placed"], 0, "said, so the page can tell the two apart");
        assert!(out["drives"].as_object().unwrap().is_empty());
    }

    #[test]
    fn largest_first_and_a_serial_less_drive_keyed_by_wwn() {
        let a = vol("small", json!([{"drive": {"serial": "", "wwn": "naa.1"}, "bytes": 1}]), json!([]));
        let b = vol("big", json!([{"drive": {"serial": "", "wwn": "naa.1"}, "bytes": 9}]), json!([]));
        let out = by_drive(&[a, b]);
        let l = out["drives"]["naa.1"].as_array().unwrap();
        assert_eq!(l[0]["id"], "big");
        assert_eq!(l[1]["id"], "small");
    }
}
