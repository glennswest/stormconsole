//! Whether a drive is in use, and by what (#308, stormdrive#123).
//!
//! One answer per drive for the rack map, the list, the drive and the format
//! page, so none of them reads a free drive into the node's system drive. A
//! stormdrive with #123 says it itself (`in_use`, `use`, `slabs`, `depends_on`,
//! `reason`), and that wins. Before it, the same answer is read from what every
//! stormdrive since 0.10 already carries: the slab partitions it found on the
//! disk (`slab_parts`, with their role), whose data it is (`in_use_by`), fleet
//! membership, and last the `contents` sentence (a slab, an ESP, a mount).

use serde_json::{json, Value};

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The word a page shows for a use (`None` for a free drive).
pub fn badge(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "system" => "System",
        "data" => "Data slab",
        "slab_member" => "Slab member",
        "raid_member" => "RAID member",
        "mounted" => "Mounted",
        "free" | "" => return None,
        _ => "In use",
    })
}

fn slabs_of(d: &Value) -> Vec<Value> {
    let list = d.get("slabs").filter(|s| s.is_array()).or_else(|| d.get("slab_parts"));
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|s| json!({"partition": s.get("partition"), "name": text(s, "name"), "role": text(s, "role")}))
        .collect()
}

fn slab_words(s: &Value) -> String {
    let name = text(s, "name");
    match (s.get("partition").and_then(Value::as_u64), name.is_empty()) {
        (Some(p), false) => format!("partition {p} '{name}'"),
        (Some(p), true) => format!("partition {p}"),
        (None, _) => "the whole drive".into(),
    }
}

/// The volumes stormdrive says have data here (#52), by name.
fn volumes_of(d: &Value) -> Vec<String> {
    d.pointer("/usage/volumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.get("name").and_then(Value::as_str).map(|n| format!("volume {n}")))
        .collect()
}

/// A drive's use. `in_use` is what a page refuses a destructive verb on.
pub fn of(d: &Value) -> Value {
    let name = match text(d, "name") {
        "" => "this drive",
        n => n,
    };
    let slabs = slabs_of(d);
    let answer = |kind: &str, reason: String, depends: Vec<String>, source: &str| {
        json!({
            "in_use": kind != "free",
            "use": kind,
            "badge": badge(kind),
            "reason": reason,
            "slabs": slabs,
            "depends_on": depends,
            "source": source,
            // What a format would destroy on a drive nobody uses (a foreign
            // filesystem): said, never a refusal.
            "holds": d.get("contents").filter(|_| kind == "free"),
        })
    };
    // stormdrive#123's own answer.
    if let Some(kind) = d.get("use").and_then(Value::as_str) {
        let in_use = d.get("in_use").and_then(Value::as_bool).unwrap_or(kind != "free");
        let kind = if in_use && kind == "free" { "in_use" } else { kind };
        let depends = d
            .get("depends_on")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_string).or_else(|| x.get("what").and_then(Value::as_str).map(str::to_string)))
            .collect();
        let reason = match text(d, "reason") {
            "" if in_use => format!("{name} is in use ({})", badge(kind).unwrap_or("in use").to_lowercase()),
            r => r.to_string(),
        };
        return answer(kind, reason, depends, "stormdrive");
    }
    let volumes = volumes_of(d);
    let role = |r: &str| slabs.iter().find(|s| text(s, "role") == r);
    if let Some(s) = role("system") {
        let mut depends = vec!["this node's root (its system slab)".to_string()];
        if role("data").is_some() {
            depends.push("this node's data half (its data slab)".into());
        }
        depends.extend(volumes);
        return answer(
            "system",
            format!("{name} holds this node's system slab ({}); the node runs from it", slab_words(s)),
            depends,
            "read here",
        );
    }
    let holder = d.get("in_use_by").and_then(Value::as_str);
    if holder.is_some_and(|h| h.starts_with("stormraid")) {
        return answer("raid_member", format!("{name} is a member of a RAID set"), vec!["a RAID set".into()], "read here");
    }
    if let Some(s) = role("data").or_else(|| slabs.first()) {
        let mut depends = vec!["this node's data half (its data slab)".to_string()];
        depends.extend(volumes);
        return answer("data", format!("{name} holds a stormblock data slab ({})", slab_words(s)), depends, "read here");
    }
    if let Some(who) = holder {
        return answer("data", format!("{name} holds data for {who}"), volumes, "read here");
    }
    if text(d, "membership") == "fleet" {
        return answer("slab_member", format!("{name} is in the storage fleet"), volumes, "read here");
    }
    if d.get("mounted").and_then(Value::as_bool) == Some(true) {
        return answer("mounted", format!("{name} has a mounted filesystem"), vec![], "read here");
    }
    // Last, the sentence: an older stormdrive, or a layout it has no field for.
    let contents = text(d, "contents").to_lowercase();
    if contents.contains("efi") || contents.contains("vfat") {
        return answer(
            "system",
            format!("{name} holds an EFI system partition ({}): the node boots from it", text(d, "contents")),
            vec!["this node's boot".into()],
            "contents",
        );
    }
    if contents.contains("stormblock slab") {
        return answer("data", format!("{name} holds a stormblock slab ({})", text(d, "contents")), volumes, "contents");
    }
    if contents.contains("mounted") {
        return answer("mounted", format!("{name}: {}", text(d, "contents")), vec![], "contents");
    }
    answer("free", String::new(), vec![], "read here")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// forge1 (12.64), `GET :9092/api/v1/drives`, stormdrive 0.39.
    fn forge1() -> Vec<Value> {
        vec![
            json!({"name": "sda", "membership": "out", "designation": "none", "owner": "stormblock",
                   "in_use_by": "stormblock (system slab partitions 2, 3)",
                   "slab_parts": [{"partition": 2, "name": "stormblock", "role": "system", "offset_bytes": 1},
                                  {"partition": 3, "name": "stormblock-data", "role": "data", "offset_bytes": 2}],
                   "contents": "vfat in partition 1 'EFI', stormblock slab in partition 2 'stormblock', stormblock slab in partition 3 'stormblock-data'"}),
            json!({"name": "sdb", "membership": "out", "designation": "none"}),
            json!({"name": "sdc", "membership": "out", "designation": "none"}),
            json!({"name": "sdd", "membership": "out", "designation": "none"}),
        ]
    }

    #[test]
    fn forge1s_system_drive_is_system_and_the_others_are_free() {
        let d = forge1();
        let a = of(&d[0]);
        assert_eq!(a["in_use"], true);
        assert_eq!(a["use"], "system");
        assert_eq!(a["badge"], "System");
        assert_eq!(a["reason"], "sda holds this node's system slab (partition 2 'stormblock'); the node runs from it");
        assert_eq!(a["slabs"].as_array().unwrap().len(), 2);
        assert!(a["depends_on"][0].as_str().unwrap().contains("root"));
        assert!(a["depends_on"][1].as_str().unwrap().contains("data half"));
        for free in &d[1..] {
            let a = of(free);
            assert_eq!(a["in_use"], false, "{free}");
            assert!(a["badge"].is_null());
        }
    }

    #[test]
    fn the_sentence_alone_still_says_in_use() {
        let only = |c: &str| of(&json!({"name": "sda", "contents": c}));
        assert_eq!(only("vfat in partition 1 'EFI', stormblock slab in partition 2 'stormblock'")["use"], "system");
        assert_eq!(only("stormblock slab (whole drive)")["use"], "data");
        assert_eq!(only("vfat in partition 1 'EFI'")["use"], "system");
        assert_eq!(only("xfs (whole drive), mounted at /srv")["use"], "mounted");
        let foreign = only("xfs (whole drive)");
        assert_eq!(foreign["in_use"], false, "a foreign filesystem is data to lose, not a use");
        assert_eq!(foreign["holds"], "xfs (whole drive)");
    }

    #[test]
    fn raid_fleet_data_and_mounted() {
        assert_eq!(of(&json!({"in_use_by": "stormraid (a RAID set member)"}))["badge"], "RAID member");
        assert_eq!(of(&json!({"membership": "fleet"}))["badge"], "Slab member");
        let data = of(&json!({"name": "sde", "slab_parts": [{"role": "data", "offset_bytes": 0}],
                              "usage": {"volumes": [{"name": "pvc-db"}]}}));
        assert_eq!(data["badge"], "Data slab");
        assert_eq!(data["reason"], "sde holds a stormblock data slab (the whole drive)");
        assert_eq!(data["depends_on"][1], "volume pvc-db");
        assert_eq!(of(&json!({"mounted": true}))["use"], "mounted");
    }

    #[test]
    fn stormdrives_own_answer_wins() {
        let a = of(&json!({"name": "sda", "in_use": true, "use": "system", "reason": "sda holds this node's system slab; the node runs from it",
                           "slabs": [{"partition": 2, "name": "stormblock", "role": "system"}],
                           "depends_on": ["the node's root", {"what": "volume v1"}],
                           "slab_parts": []}));
        assert_eq!(a["source"], "stormdrive");
        assert_eq!(a["badge"], "System");
        assert_eq!(a["depends_on"][1], "volume v1");
        assert_eq!(a["slabs"][0]["name"], "stormblock");
        let free = of(&json!({"in_use": false, "use": "free", "slab_parts": [{"role": "system"}]}));
        assert_eq!(free["in_use"], false, "its word, even against what the fallback would read");
        assert_eq!(of(&json!({"use": "slab_member"}))["in_use"], true);
    }
}
