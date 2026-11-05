//! What a container or a VM disk runs on, and what it was cloned from (#75). The owner:
//! "nothing uses a golden, everything uses a CoW, but the golden is our registry".
//!
//! `GET /api/plugins/sb/runs-on` finds the volume on this node's engine by what the
//! kubelet says (its id), by its owner (a VM's disks) or by where it is mounted (a node
//! service), and follows its parents to the sealed image it reads through. A machine's
//! own sealed image is followed past, as on the Registry page (#163).
//!
//! **Last verified is this copy's**: the engine's seal record says when it last read
//! these bytes and what it found. It is only on `/digest`, which reads the whole image
//! when no digest is cached (stormblock#535), so that call gets a short budget: the
//! engine's read goes on without it and fills the cache, and the page says so. The
//! engine starts a new read for every such call, so an image that did not answer is
//! left alone for a while ([`READ_HOLD_OFF`]).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, Default)]
pub struct Query {
    /// An engine volume id, or its name.
    pub volume: Option<String>,
    /// Where a node service's volume is mounted.
    pub mounted: Option<String>,
    /// A kube object's volumes: kinds, comma separated, with `namespace` and `name`.
    pub owner: Option<String>,
    pub namespace: Option<String>,
    pub name: Option<String>,
}

/// How long an image that did not answer is left alone. Every `/digest` without a cached
/// answer starts its own whole read in the engine, and a page reloads.
pub const READ_HOLD_OFF: Duration = Duration::from_secs(300);

/// Images whose digest was asked for and did not come in time.
#[derive(Default)]
pub struct Reading(std::sync::Mutex<HashMap<String, Instant>>);

impl Reading {
    /// Whether to ask the engine about this image now.
    pub fn may_ask(&self, id: &str, now: Instant) -> bool {
        let mut held = self.0.lock().unwrap_or_else(|e| e.into_inner());
        held.retain(|_, at| now.duration_since(*at) < READ_HOLD_OFF);
        !held.contains_key(id)
    }

    /// It did not answer in its budget: its read is going on in the engine.
    pub fn started(&self, id: &str, now: Instant) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), now);
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// The listed volumes this asks about.
pub fn select<'a>(volumes: &'a [Value], q: &Query) -> Vec<&'a Value> {
    volumes
        .iter()
        .filter(|v| {
            if let Some(id) = &q.volume {
                return s(v, "id") == id || s(v, "name") == id;
            }
            if let Some(path) = &q.mounted {
                return v.get("attachments").and_then(Value::as_array).into_iter().flatten().any(|a| s(a, "mounted_at") == path);
            }
            if let (Some(kinds), Some(ns), Some(name)) = (&q.owner, &q.namespace, &q.name) {
                let o = v.get("owner").filter(|o| o.is_object()).or_else(|| v.get("consumer").filter(|o| o.is_object()));
                return o.is_some_and(|o| kinds.split(',').any(|k| k == s(o, "kind")) && s(o, "namespace") == ns && s(o, "name") == name);
            }
            false
        })
        .collect()
}

/// The sealed image a volume reads through: its nearest sealed ancestor, past a
/// machine's own (`hostgolden-…`). `None` for a volume made empty or a parent gone.
/// A node service runs on the sealed image itself, so that volume is its own answer.
pub fn image_of<'a>(volume: &'a Value, by_id: &HashMap<&str, &'a Value>) -> Option<&'a Value> {
    if volume.get("sealed").and_then(Value::as_bool) == Some(true) {
        return Some(volume);
    }
    let mut at = volume;
    for _ in 0..64 {
        let parent = by_id.get(at.get("parent").and_then(Value::as_str)?).copied()?;
        if parent.get("sealed").and_then(Value::as_bool) == Some(true) && !s(parent, "name").starts_with("hostgolden-") {
            return Some(parent);
        }
        at = parent;
    }
    None
}

/// The volume as the page shows it, from the engine's single GET, which always has
/// its placement.
pub fn runs_on(v: &Value) -> Value {
    let p = v.get("placement");
    let list = |k: &str| p.and_then(|p| p.get(k)).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut nodes: Vec<String> = list("drives").iter().map(|d| s(d, "node").to_string()).filter(|n| !n.is_empty()).collect();
    nodes.sort();
    nodes.dedup();
    let drives: Vec<Value> = list("drives")
        .iter()
        .map(|d| {
            let r = d.get("drive").cloned().unwrap_or(Value::Null);
            json!({"serial": s(&r, "serial"), "path": s(&r, "path"), "bytes": d.get("bytes")})
        })
        .collect();
    json!({
        "id": s(v, "id"),
        "name": s(v, "name"),
        "virtualBytes": v.get("virtual_size_bytes"),
        "allocatedBytes": v.get("allocated_bytes"),
        "sharedBytes": v.get("shared_bytes"),
        "physicalBytes": v.get("physical_bytes"),
        "health": s(v, "health"),
        "redundancy": s(v, "redundancy"),
        "role": s(v, "role"),
        "inUse": v.get("in_use"),
        "nodes": nodes,
        "drives": drives,
        "slabs": list("slabs").len(),
    })
}

/// The image, and what `/digest` said of it, if it said in time.
pub fn cloned_from(image: &Value, digest: Option<&Value>) -> Value {
    let seal = digest.and_then(|d| d.get("seal")).filter(|x| x.is_object());
    json!({
        "id": s(image, "id"),
        "name": s(image, "name"),
        "kind": s(image, "kind"),
        "virtualBytes": image.get("virtual_size_bytes"),
        "allocatedBytes": image.get("allocated_bytes"),
        // What it was sealed with; else what reading its whole capacity gave.
        "digest": seal.map(|x| s(x, "sha256")).filter(|d| !d.is_empty()).or(digest.map(|d| s(d, "sha256"))).filter(|d| !d.is_empty()),
        "verdict": seal.map(|x| s(x, "verdict")),
        "checkedAt": seal.and_then(|x| x.get("checked_at")).and_then(Value::as_u64).filter(|t| *t > 0),
        "reading": digest.is_none(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_is_found_by_what_says_so() {
        let vols = vec![
            json!({"id": "a", "name": "ctr-a"}),
            json!({"id": "b", "name": "pallet", "attachments": [{"transport": "ublk", "mounted_at": "/pallets/stormlb"}]}),
            json!({"id": "c", "name": "shop.vm1-root", "owner": {"kind": "VirtualMachineInstance", "namespace": "shop", "name": "vm1"}}),
            json!({"id": "d", "name": "shop.vm1-data", "consumer": {"kind": "VirtualMachine", "namespace": "shop", "name": "vm1"}}),
            json!({"id": "e", "name": "other", "owner": {"kind": "VirtualMachine", "namespace": "lab", "name": "vm1"}}),
        ];
        let ids = |q: Query| select(&vols, &q).iter().map(|v| s(v, "id").to_string()).collect::<Vec<_>>();
        assert_eq!(ids(Query { volume: Some("a".into()), ..Default::default() }), ["a"]);
        assert_eq!(ids(Query { volume: Some("ctr-a".into()), ..Default::default() }), ["a"]);
        assert_eq!(ids(Query { mounted: Some("/pallets/stormlb".into()), ..Default::default() }), ["b"]);
        let owner = Query { owner: Some("VirtualMachine,VirtualMachineInstance".into()), namespace: Some("shop".into()), name: Some("vm1".into()), ..Default::default() };
        assert_eq!(ids(owner), ["c", "d"], "another project's machine of the same name is not this one");
        assert!(ids(Query::default()).is_empty());
    }

    #[test]
    fn its_image_is_the_sealed_one_it_reads_through() {
        let vols = [
            json!({"id": "g", "name": "golden-a-aaaaaaaaaaaa", "sealed": true}),
            json!({"id": "h", "name": "hostgolden-r230", "sealed": true, "parent": "g"}),
            json!({"id": "boot", "name": "boothost-r230", "parent": "h"}),
            json!({"id": "c1", "name": "clone", "parent": "g"}),
            json!({"id": "c2", "name": "clone-of-clone", "parent": "c1"}),
            json!({"id": "e", "name": "empty"}),
            json!({"id": "x", "name": "lost", "parent": "gone"}),
        ];
        let by: HashMap<&str, &Value> = vols.iter().map(|v| (s(v, "id"), v)).collect();
        let of = |i: &str| image_of(by[i], &by).map(|v| s(v, "id"));
        assert_eq!(of("c1"), Some("g"));
        assert_eq!(of("c2"), Some("g"), "through a clone");
        assert_eq!(of("boot"), Some("g"), "past a machine's own");
        assert_eq!(of("g"), Some("g"), "a sealed volume is its own image");
        assert_eq!(of("e"), None);
        assert_eq!(of("x"), None);
    }

    #[test]
    fn what_a_volume_costs_and_where_it_is() {
        let r = runs_on(&json!({"id": "c", "name": "n", "virtual_size_bytes": 100, "allocated_bytes": 7, "shared_bytes": 93, "health": "healthy", "redundancy": "mirror:2", "in_use": true,
            "placement": {"slabs": [{}, {}], "drives": [{"node": "n1", "bytes": 5, "drive": {"serial": "S1", "path": "/dev/a"}}, {"node": "n1", "bytes": 2, "drive": {"serial": "S2"}}]}}));
        assert_eq!((r["allocatedBytes"].as_u64(), r["sharedBytes"].as_u64(), r["slabs"].as_u64()), (Some(7), Some(93), Some(2)));
        assert_eq!(r["nodes"], json!(["n1"]), "one node, however many drives");
        assert_eq!(r["drives"][0]["serial"], "S1");
    }

    #[test]
    fn an_image_being_read_is_not_asked_again() {
        let r = Reading::default();
        let t = Instant::now();
        assert!(r.may_ask("g", t));
        r.started("g", t);
        assert!(!r.may_ask("g", t + Duration::from_secs(60)), "a second whole read would start");
        assert!(r.may_ask("other", t + Duration::from_secs(60)));
        assert!(r.may_ask("g", t + READ_HOLD_OFF), "and it is asked again in time");
    }

    #[test]
    fn last_verified_is_what_the_engine_says() {
        let img = json!({"id": "g", "name": "golden-a", "kind": "golden"});
        let checked = cloned_from(&img, Some(&json!({"sha256": "whole", "seal": {"sha256": "sealed", "verdict": "intact", "checked_at": 99}})));
        assert_eq!((checked["digest"].as_str(), checked["verdict"].as_str(), checked["checkedAt"].as_u64()), (Some("sealed"), Some("intact"), Some(99)));
        // Sealed here, and never read since: not a date of 1970.
        let never = cloned_from(&img, Some(&json!({"sha256": "whole", "seal": {"sha256": "sealed", "verdict": "unchecked", "checked_at": 0}})));
        assert!(never["checkedAt"].is_null());
        assert_eq!(never["verdict"], "unchecked");
        // No seal record: the whole capacity's digest is what there is.
        assert_eq!(cloned_from(&img, Some(&json!({"sha256": "whole"})))["digest"], "whole");
        // Did not answer in its budget.
        let slow = cloned_from(&img, None);
        assert_eq!((slow["reading"].as_bool(), slow["digest"].is_null()), (Some(true), true));
    }
}
