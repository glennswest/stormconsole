//! A node's shelves and RAID sets, as its engine holds them (#67, stormblock#252).
//!
//! A shelf is not an object: it is the name a set's and a spare's superblock carry as their pool.
//! `GET /api/v1/shelves` groups by it; a set with no pool is one consumer's array, and stays apart.
//! Volumes on a set are counted and not named here: they may be another project's.

use serde_json::{json, Value};

/// The worst set's state, as the engine ranks them. `None` when it has no set, or does not say.
pub fn worst(arrays: &[Value]) -> Option<String> {
    let rank = |s: &str| match s {
        "failed" => 3,
        "degraded" => 2,
        "rebuilding" => 1,
        _ => 0,
    };
    arrays.iter().filter_map(state).max_by_key(|s| rank(s)).map(str::to_string)
}

pub fn state(array: &Value) -> Option<&str> {
    array.pointer("/status/state").and_then(Value::as_str)
}

/// A member's id in the feed: its device, or where it was when its drive is gone.
pub fn member_id(array: &Value, member: &Value) -> String {
    match member.get("device_path").and_then(Value::as_str).filter(|p| !p.is_empty()) {
        Some(path) => format!("sb:member:{path}"),
        None => format!("sb:member:{}:{}", s(array, "id"), member["index"]),
    }
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One rung of `shelf=S/bay=3`, which is where the drive was registered.
fn label<'a>(labels: &'a str, key: &str) -> Option<&'a str> {
    labels.split('/').find_map(|part| part.strip_prefix(key)?.strip_prefix('='))
}

fn set_view(a: &Value) -> Value {
    let mut members: Vec<Value> = a["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            json!({
                "slot": m["index"],
                "state": m["state"],
                "path": m["device_path"],
                "serial": m.pointer("/drive/serial"),
                "wwn": m.pointer("/drive/wwn"),
                "model": m.pointer("/drive/model"),
                "bay": label(s(m, "labels"), "bay"),
                "labels": m["labels"],
                "rebuiltBytes": m["rebuilt_bytes"],
                "component": member_id(a, m),
            })
        })
        .collect();
    members.sort_by_key(|m| m["slot"].as_u64());
    json!({
        "id": a["id"],
        "name": a["name"],
        "shelf": a["pool"],
        "level": a["level"],
        "capacity": a["capacity_human"],
        "stripe": a["stripe_human"],
        "events": a["events"],
        "status": a["status"],
        "members": members,
        "volumes": a["volumes"].as_array().map_or(0, Vec::len),
    })
}

fn spare_view(sp: &Value) -> Value {
    json!({"drive": sp["drive_uuid"], "path": sp["path"], "serial": sp["serial"], "capacityBytes": sp["capacity_bytes"]})
}

/// What the page shows. `free` is what a spare or a replacement can be: drives offered blank that no set or
/// spare holds. A drive opened at its start says nothing about what it held, so it is not offered. A drive a set
/// has since replaced looks the same here (stormblock#537); the engine refuses it, in words, without `force`, which is not offered.
pub fn view(shelves: &[Value], arrays: &[Value], spares: &[Value], drives: &[Value]) -> Value {
    let used = |uuid: &str| {
        spares.iter().any(|x| s(x, "drive_uuid") == uuid)
            || arrays.iter().flat_map(|a| a["members"].as_array().into_iter().flatten()).any(|m| m.pointer("/drive/uuid").and_then(Value::as_str) == Some(uuid))
    };
    let free: Vec<Value> = drives
        .iter()
        .filter(|d| d.pointer("/offer/state").and_then(Value::as_str) == Some("blank") && !used(s(d, "uuid")))
        .map(|d| json!({"uuid": d["uuid"], "path": d["path"], "serial": d["serial"], "capacity": d["capacity_human"], "labels": d["labels"]}))
        .collect();
    json!({
        "worst": worst(arrays),
        "shelves": shelves.iter().map(|sh| json!({
            "name": sh["name"],
            "state": sh["state"],
            "usable": sh["usable_human"],
            "sets": sh["sets"].as_array().into_iter().flatten().map(set_view).collect::<Vec<_>>(),
            "spares": sh["spares"].as_array().into_iter().flatten().map(spare_view).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "arrays": arrays.iter().filter(|a| s(a, "pool").is_empty()).map(set_view).collect::<Vec<_>>(),
        "globalSpares": spares.iter().filter(|x| s(x, "pool").is_empty()).map(spare_view).collect::<Vec<_>>(),
        "free": free,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shapes from stormblock `src/mgmt/api/arrays.rs` and `shelves.rs`.
    fn set() -> Value {
        json!({
            "id": "a1", "name": "ds0-a", "pool": "ds0", "level": "raid6", "capacity_human": "4 GiB", "stripe_human": "64 KiB", "events": 9,
            "status": {"state": "rebuilding", "failed": 1, "rebuilding": 1, "tolerated": 2, "dirty_chunks": 0,
                       "rebuild": {"slots": [1], "done_bytes": 5, "total_bytes": 10, "percent": 50.0, "running": true, "rate_bytes_per_sec": 7}},
            "members": [
                {"index": 1, "uuid": "m1", "state": "failed", "device_path": "", "labels": ""},
                {"index": 0, "uuid": "m0", "state": "active", "device_path": "/dev/sdb", "labels": "shelf=ds0/bay=3",
                 "drive": {"uuid": "d0", "serial": "ZC1", "model": "m", "path": "/dev/sdb"}},
            ],
            "slab": {"id": "s"},
            "volumes": [{"id": "v", "name": "someones-secret-name", "pinned": false}],
        })
    }

    #[test]
    fn the_worst_set_decides_and_an_older_engine_says_nothing() {
        let with = |st: &str| json!({"status": {"state": st}});
        assert_eq!(worst(&[with("clean"), with("failed"), with("rebuilding")]).as_deref(), Some("failed"));
        assert_eq!(worst(&[with("clean"), with("degraded"), with("rebuilding")]).as_deref(), Some("degraded"));
        assert_eq!(worst(&[json!({"id": "old"})]), None);
        assert_eq!(worst(&[]), None);
    }

    #[test]
    fn a_member_says_its_bay_and_one_whose_drive_is_gone_still_has_a_row() {
        let v = set_view(&set());
        assert_eq!(v["members"][0]["slot"], 0, "in slot order");
        assert_eq!(v["members"][0]["bay"], "3");
        assert_eq!(v["members"][0]["serial"], "ZC1");
        assert_eq!(v["members"][0]["component"], "sb:member:/dev/sdb");
        assert_eq!(v["members"][1]["state"], "failed");
        assert_eq!(v["members"][1]["component"], "sb:member:a1:1");
        assert_eq!(v["members"][1]["bay"], Value::Null);
        assert_eq!(v["status"]["rebuild"]["percent"], 50.0);
    }

    #[test]
    fn a_volume_on_a_set_is_counted_and_not_named() {
        let v = view(&[], &[set()], &[], &[]);
        assert!(!v.to_string().contains("someones-secret-name"));
    }

    #[test]
    fn only_a_blank_drive_nothing_holds_is_free() {
        let drive = |uuid: &str, offer: Value| json!({"uuid": uuid, "path": format!("/dev/{uuid}"), "serial": uuid, "capacity_human": "1 GiB", "offer": offer});
        let drives = [
            drive("blank", json!({"state": "blank"})),
            drive("d0", json!({"state": "blank"})),
            drive("sp", json!({"state": "blank"})),
            drive("ours", json!({"state": "ours"})),
            drive("foreign", json!({"state": "foreign"})),
            json!({"uuid": "boot", "path": "/dev/boot"}),
        ];
        let spares = [json!({"drive_uuid": "sp", "pool": "", "path": "/dev/sp", "serial": "sp"})];
        let v = view(&[], &[set()], &spares, &drives);
        let free: Vec<&str> = v["free"].as_array().unwrap().iter().map(|d| d["uuid"].as_str().unwrap()).collect();
        assert_eq!(free, ["blank"]);
        assert_eq!(v["globalSpares"][0]["serial"], "sp");
    }

    #[test]
    fn a_set_on_a_shelf_is_under_it_and_an_array_of_ones_own_is_apart() {
        let own = json!({"id": "a2", "pool": "", "members": []});
        let shelf = json!({"name": "ds0", "state": "rebuilding", "usable_human": "4 GiB", "sets": [set()], "spares": []});
        let v = view(&[shelf], &[set(), own], &[], &[]);
        assert_eq!(v["shelves"][0]["sets"][0]["name"], "ds0-a");
        assert_eq!(v["arrays"].as_array().unwrap().len(), 1);
        assert_eq!(v["arrays"][0]["id"], "a2");
        assert_eq!(v["worst"], "rebuilding");
    }
}
