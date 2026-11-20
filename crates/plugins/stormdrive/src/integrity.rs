//! Every RAID set's integrity: bit rot found and repaired (#368, stormraid#80).
//!
//! stormraid v0.27.0 keeps, on each RaidSet (`storage.storm.io/v1alpha1`):
//!
//! - `status.integrity`: the set's lifetime counters (`totals`: checksum
//!   mismatches by where they were found — L1 the read path, L2 verifying a
//!   fresh stripe, L3 the scrub —, blocks rewritten from parity, PI, read and
//!   write errors, rows past parity), the background scrub (`scrubbing`,
//!   `scrubPasses`, `scrubEvery`, `nextScrub`, `lastScrubPass`);
//! - `status.members[].integrity`: the same counters per member drive;
//! - the condition `BitRot`: True while a member's mismatches or repairs rose
//!   in the last scrub pass or the last 24 h, its message naming them.
//!
//! The node's RaidSet CRD keeps all of it since stormcos#1017. This reads
//! every RaidSet from the apiserver once a minute, on the console's own
//! credential (the alarm is the console's, not a viewer's), and keeps:
//!
//! - **a row per set**, each member matched to its drive by WWN (the drive's
//!   feed id, so the drive page finds its member) — [`set_row`];
//! - **the last day's samples** of every counter, one each [`SAMPLE_EVERY`],
//!   so a set and a member say what rose in the last 24 h as well as for
//!   life ([`Seen::answer`]); a console up for less than a day says since when;
//! - **an alarm** per set whose `BitRot` is on ([`alarm`]), kept for an hour
//!   after it goes off, as a drive link's is (#339). Rows past parity (data
//!   parity could not give back) in the last day make it red.

use std::collections::{BTreeMap, VecDeque};

use console_core::{ComponentSummary, Health, Metric};
use serde_json::{json, Value};

/// How often the sets are read.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// One kept sample per this many seconds.
pub const SAMPLE_EVERY: u64 = 600;
/// The window "per day" is over.
pub const DAY: u64 = 86_400;
/// How long an alarm stays after `BitRot` goes off.
pub const CLEAR_AFTER: u64 = 3600;
/// Where the sets are.
pub const RAIDSETS: &str = "/apis/storage.storm.io/v1alpha1/raidsets";

fn at<'a>(o: &'a Value, ptr: &str) -> &'a Value {
    o.pointer(ptr).unwrap_or(&Value::Null)
}

fn u(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// A WWN in one form, as stormraid compares them (`ident::norm_wwn`):
/// lower case, without `wwn-`, `0x`, `naa.`, `eui.`; and without a
/// partition member's `-partN`, so it names the disk.
pub fn norm_wwn(w: &str) -> String {
    let mut w = w.trim().to_ascii_lowercase();
    for p in ["wwn-", "nvme-"] {
        if let Some(x) = w.strip_prefix(p) {
            w = x.to_string();
        }
    }
    for p in ["0x", "naa.", "eui."] {
        if let Some(x) = w.strip_prefix(p) {
            w = x.to_string();
        }
    }
    if let Some((disk, n)) = w.rsplit_once("-part") {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            w = disk.to_string();
        }
    }
    w
}

/// A host name's first label: `forge1.g16.lo` and `forge1` are one node.
fn short(node: &str) -> String {
    node.split('.').next().unwrap_or_default().to_ascii_lowercase()
}

/// The lifetime counters of a set or a member.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub l1: u64,
    pub l2: u64,
    pub l3: u64,
    pub repaired: u64,
    pub pi: u64,
    pub read: u64,
    pub write: u64,
    pub unrecoverable: u64,
}

impl Counts {
    /// From stormraid's `IntegrityCounts`; `None` when there are none.
    pub fn of(v: &Value) -> Option<Counts> {
        if !v.is_object() {
            return None;
        }
        let m = &v["checksumMismatches"];
        Some(Counts {
            l1: u(m, "l1"),
            l2: u(m, "l2"),
            l3: u(m, "l3"),
            repaired: u(v, "blocksRepaired"),
            pi: u(v, "piErrors"),
            read: u(v, "readErrors"),
            write: u(v, "writeErrors"),
            unrecoverable: u(v, "unrecoverableRows"),
        })
    }

    pub fn mismatches(&self) -> u64 {
        self.l1 + self.l2 + self.l3
    }

    fn fields(&self) -> [u64; 8] {
        [self.l1, self.l2, self.l3, self.repaired, self.pi, self.read, self.write, self.unrecoverable]
    }

    /// What rose from `base` to this; `None` if any counter went back (the
    /// counters were reset), which says nothing about a day.
    pub fn since(&self, base: &Counts) -> Option<Counts> {
        if self.fields().iter().zip(base.fields()).any(|(a, b)| *a < b) {
            return None;
        }
        Some(Counts {
            l1: self.l1 - base.l1,
            l2: self.l2 - base.l2,
            l3: self.l3 - base.l3,
            repaired: self.repaired - base.repaired,
            pi: self.pi - base.pi,
            read: self.read - base.read,
            write: self.write - base.write,
            unrecoverable: self.unrecoverable - base.unrecoverable,
        })
    }

    pub fn json(&self) -> Value {
        json!({
            "mismatches": self.mismatches(), "l1": self.l1, "l2": self.l2, "l3": self.l3,
            "repaired": self.repaired, "piErrors": self.pi, "readErrors": self.read,
            "writeErrors": self.write, "unrecoverableRows": self.unrecoverable,
        })
    }
}

/// Every drive the console reads, by WWN and by node and kernel name: its
/// feed id, as the drive page keys a drive.
#[derive(Default)]
pub struct Index {
    by_wwn: BTreeMap<String, Value>,
    by_name: BTreeMap<(String, String), Value>,
}

impl Index {
    /// Add one node's Drive objects (`prefix` is that node's id prefix).
    pub fn add(&mut self, node: &str, prefix: &str, objects: &[Value]) {
        for o in objects {
            let Some(id) = o.pointer("/metadata/name").and_then(Value::as_str) else { continue };
            let d = json!({
                "component": format!("{prefix}:drive:{id}"),
                "node": node,
                "serial": at(o, "/status/serial"),
                "bay": at(o, "/status/bay"),
                "enclosure": at(o, "/status/enclosure"),
            });
            if let Some(w) = o.pointer("/status/wwid").and_then(Value::as_str).map(norm_wwn).filter(|w| !w.is_empty()) {
                self.by_wwn.insert(w, d.clone());
            }
            if let Some(n) = o.pointer("/status/name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                self.by_name.insert((short(node), n.to_string()), d);
            }
        }
    }

    /// The drive a member is: by its WWN anywhere, else by its device's
    /// kernel name on the set's node.
    pub fn find(&self, node: &str, wwn: Option<&str>, path: Option<&str>) -> Option<&Value> {
        if let Some(d) = wwn.map(norm_wwn).filter(|w| !w.is_empty()).and_then(|w| self.by_wwn.get(&w)) {
            return Some(d);
        }
        let kname = path?.rsplit('/').next()?;
        // `/dev/sdb3` is a partition of sdb.
        let disk = if kname.starts_with("sd") { kname.trim_end_matches(|c: char| c.is_ascii_digit()) } else { kname };
        self.by_name.get(&(short(node), disk.to_string()))
    }
}

/// When an RFC 3339 time was, in seconds; `None` for none or the epoch.
pub fn epoch(t: Option<&str>) -> Option<u64> {
    let t = chrono::DateTime::parse_from_rfc3339(t?).ok()?.timestamp();
    u64::try_from(t).ok().filter(|t| *t > 0)
}

fn bitrot(o: &Value) -> Option<&Value> {
    at(o, "/status/conditions").as_array()?.iter().find(|c| c.get("type").and_then(Value::as_str) == Some("BitRot"))
}

/// One set as the console shows it: `None` for an object with no name.
/// `integrity` is null on a set whose stormraid predates v0.27.0 (or a CRD
/// that pruned it); the page says so.
pub fn set_row(o: &Value, idx: &Index) -> Option<Value> {
    let name = o.pointer("/metadata/name").and_then(Value::as_str)?;
    let node = at(o, "/spec/node").as_str().unwrap_or_default();
    let integ = at(o, "/status/integrity");
    // Each member's counters from its own row; a member stormraid lists only
    // under `integrity.members` (none today, but the schema allows it) by path.
    let by_path: BTreeMap<&str, &Value> = integ["members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| Some((m.get("path")?.as_str()?, &m["counts"])))
        .collect();
    let members: Vec<Value> = at(o, "/status/members")
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let path = m.get("path").and_then(Value::as_str);
            let wwn = m.get("wwn").and_then(Value::as_str);
            let counts = Counts::of(&m["integrity"]).or_else(|| path.and_then(|p| by_path.get(p).copied()).and_then(Counts::of));
            let mi = &m["integrity"];
            json!({
                "slot": m.get("slot"), "path": path, "wwn": wwn, "role": m.get("role"), "state": m.get("state"),
                "bay": m.get("bay"), "enclosure": m.get("enclosure"),
                "drive": idx.find(node, wwn, path),
                "counts": counts.map(|c| c.json()),
                "last": {
                    "mismatch": mi.get("lastMismatch"), "repair": mi.get("lastRepair"), "piError": mi.get("lastPiError"),
                    "readError": mi.get("lastReadError"), "writeError": mi.get("lastWriteError"),
                },
            })
        })
        .collect();
    let totals = Counts::of(&integ["totals"]);
    let t = &integ["totals"];
    Some(json!({
        "name": name,
        "node": node,
        "parity": at(o, "/spec/parity"),
        "pi": at(o, "/spec/pi"),
        "phase": at(o, "/status/phase"),
        "device": at(o, "/status/device"),
        "integrity": if integ.is_object() {
            json!({
                "totals": totals.map(|c| c.json()),
                "last": {
                    "mismatch": t.get("lastMismatch"), "repair": t.get("lastRepair"), "piError": t.get("lastPiError"),
                    "readError": t.get("lastReadError"), "writeError": t.get("lastWriteError"),
                    "unrecoverable": t.get("lastUnrecoverable"),
                },
                "scrubbing": integ.get("scrubbing"),
                "scrubPasses": integ.get("scrubPasses"),
                "scrubEvery": integ.get("scrubEvery"),
                "nextScrub": integ.get("nextScrub"),
                "lastScrubPass": integ.get("lastScrubPass"),
            })
        } else {
            Value::Null
        },
        "lastScrub": at(o, "/status/lastScrub"),
        "bitrot": bitrot(o),
        "members": members,
    }))
}

/// A kept sample.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub at: u64,
    pub counts: Counts,
}

/// What has been read.
#[derive(Default)]
pub struct Seen {
    pub rows: Vec<Value>,
    /// Why the sets could not be read, if they could not.
    pub error: Option<String>,
    /// The apiserver serves no RaidSet kind.
    pub absent: bool,
    pub read: bool,
    pub at: u64,
    /// Each set's (`<set>`) and member's (`<set>/<path>`) samples of the last day.
    pub history: BTreeMap<String, VecDeque<Sample>>,
}

/// The keys a row's counters are kept under: the set's, then each member's.
fn keyed(r: &Value) -> Vec<(String, Option<Counts>)> {
    let name = r["name"].as_str().unwrap_or_default();
    let mut out = vec![(name.to_string(), counts_of(&r["integrity"]["totals"]))];
    for m in r["members"].as_array().into_iter().flatten() {
        let who = m["wwn"].as_str().or(m["path"].as_str()).unwrap_or_default();
        out.push((format!("{name}/{who}"), counts_of(&m["counts"])));
    }
    out
}

/// Back from [`Counts::json`].
fn counts_of(v: &Value) -> Option<Counts> {
    if !v.is_object() {
        return None;
    }
    Some(Counts {
        l1: u(v, "l1"),
        l2: u(v, "l2"),
        l3: u(v, "l3"),
        repaired: u(v, "repaired"),
        pi: u(v, "piErrors"),
        read: u(v, "readErrors"),
        write: u(v, "writeErrors"),
        unrecoverable: u(v, "unrecoverableRows"),
    })
}

impl Seen {
    /// Take a reading at `now`: the rows replace the old; each counter gains
    /// a sample if the last is [`SAMPLE_EVERY`] old, and loses those older
    /// than a day (keeping the one a day back, the day's base).
    pub fn take(&mut self, rows: Vec<Value>, now: u64) {
        let mut live = std::collections::BTreeSet::new();
        for r in &rows {
            for (key, c) in keyed(r) {
                live.insert(key.clone());
                let Some(c) = c else { continue };
                let h = self.history.entry(key).or_default();
                // Counters gone back: what was kept says nothing now.
                if h.back().is_some_and(|s| c.since(&s.counts).is_none()) {
                    h.clear();
                }
                if h.back().is_none_or(|s| now >= s.at + SAMPLE_EVERY) {
                    h.push_back(Sample { at: now, counts: c });
                }
                while h.len() > 1 && h[1].at + DAY <= now {
                    h.pop_front();
                }
            }
        }
        self.history.retain(|k, _| live.contains(k));
        self.rows = rows;
        self.error = None;
        self.absent = false;
        self.read = true;
        self.at = now;
    }

    /// A read that failed: the rows are kept, as last read, and the reason said.
    pub fn failed(&mut self, error: String, absent: bool) {
        self.error = Some(error);
        self.absent = absent;
        self.read = true;
        if absent {
            self.rows.clear();
            self.history.clear();
        }
    }

    /// What rose in the last day (or since the console began keeping, if
    /// less): `{since, counts}`, or null with nothing kept.
    fn day(&self, key: &str, now: Option<Counts>) -> Value {
        let (Some(h), Some(now)) = (self.history.get(key), now) else { return Value::Null };
        // [`Seen::take`] keeps the last sample a day old (or older) first.
        let Some(base) = h.front() else { return Value::Null };
        match now.since(&base.counts) {
            Some(d) => json!({"since": base.at, "full": self.at.saturating_sub(base.at) + SAMPLE_EVERY >= DAY, "counts": d.json()}),
            None => Value::Null,
        }
    }

    /// The answer of `GET /integrity`: each set with its and its members' day.
    pub fn answer(&self) -> Value {
        let sets: Vec<Value> = self
            .rows
            .iter()
            .map(|r| {
                let mut r = r.clone();
                let name = r["name"].as_str().unwrap_or_default().to_string();
                if r["integrity"].is_object() {
                    r["integrity"]["day"] = self.day(&name, counts_of(&r["integrity"]["totals"]));
                }
                for m in r["members"].as_array_mut().into_iter().flatten() {
                    let who = m["wwn"].as_str().or(m["path"].as_str()).unwrap_or_default().to_string();
                    m["day"] = self.day(&format!("{name}/{who}"), counts_of(&m["counts"]));
                }
                r
            })
            .collect();
        json!({"sets": sets, "read": self.read, "at": self.at, "error": self.error, "absent": self.absent})
    }
}

/// An alarm on one set.
#[derive(Clone, Debug, PartialEq)]
pub struct Alarm {
    pub set: String,
    /// `lost` (rows past parity in the last day), `bitrot`, or `clearing`.
    pub level: &'static str,
    pub text: String,
}

/// The alarm a set raises now (#368):
///
/// - `BitRot` on, with rows past parity in the last day → **lost** (red:
///   parity could not give those blocks back);
/// - `BitRot` on → **bitrot** (amber: found and rewritten from parity, the
///   drive it was on named in the message);
/// - `BitRot` off for less than [`CLEAR_AFTER`] → **clearing**;
/// - otherwise none.
pub fn alarm(r: &Value, now: u64) -> Option<Alarm> {
    let c = r.get("bitrot").filter(|c| c.is_object())?;
    let set = r["name"].as_str()?.to_string();
    let node = r["node"].as_str().filter(|n| !n.is_empty());
    let on = c.get("status").and_then(Value::as_str) == Some("True");
    let place = match node {
        Some(n) => format!("RAID set {set} on {n}"),
        None => format!("RAID set {set}"),
    };
    let lost_at = epoch(r["integrity"]["last"]["unrecoverable"].as_str());
    let lost = lost_at.is_some_and(|t| t + DAY > now);
    if on {
        let mut text = format!("Bit rot on {place}");
        if let Some(m) = c.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()) {
            text.push_str(&format!(": {m}"));
        }
        if lost {
            let rows = u(&r["integrity"]["totals"], "unrecoverableRows");
            text.push_str(&format!(" — {rows} row(s) past parity: data lost"));
        }
        return Some(Alarm { set, level: if lost { "lost" } else { "bitrot" }, text });
    }
    let off = epoch(c.get("lastTransitionTime").and_then(Value::as_str))?;
    let off_for = now.saturating_sub(off);
    if off_for >= CLEAR_AFTER {
        return None;
    }
    let mins = match off_for / 60 {
        0 => "under a minute".to_string(),
        1 => "1 min".to_string(),
        m => format!("{m} min"),
    };
    Some(Alarm {
        set,
        level: "clearing",
        text: format!("No new bit rot on {place} for {mins} (the alarm clears after an hour)"),
    })
}

/// The alarm as a row of the feed, shown on every page's alarm bar and
/// linking to the set.
pub fn component(a: &Alarm, prefix_id: &str) -> ComponentSummary {
    let (health, tone) = match a.level {
        "lost" => (Health::Error, "error"),
        _ => (Health::Warn, "warn"),
    };
    ComponentSummary {
        id: format!("{prefix_id}:alarm:bitrot:{}", a.set),
        kind: "alarm".into(),
        label: "Bit rot".into(),
        health,
        detail: a.text.clone(),
        metrics: vec![Metric::new("level", a.level).tone(tone)],
        actions: Vec::new(),
        relations: Vec::new(),
        link: Some(format!("#/drives/integrity?set={}", a.set)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(l3: u64, repaired: u64, unrecoverable: u64) -> Value {
        json!({"blocksRepaired": repaired, "checksumMismatches": {"l1": 1, "l2": 0, "l3": l3},
               "piErrors": 2, "readErrors": 3, "writeErrors": 0, "unrecoverableRows": unrecoverable,
               "lastMismatch": "2026-11-20T10:00:00Z", "lastRepair": "2026-11-20T10:00:00Z",
               "lastUnrecoverable": if unrecoverable > 0 { json!("2026-11-20T10:00:00Z") } else { Value::Null }})
    }

    /// md0 on forge1, as stormraid v0.27.0 writes it: slot 2 (WWN
    /// 5000cca26a819e69) found 7 mismatches in the scrub, all repaired.
    fn md0(rot: &str, since: &str, unrecoverable: u64) -> Value {
        json!({
            "metadata": {"name": "md0"},
            "spec": {"node": "forge1.g16.lo", "parity": 2, "pi": "type1"},
            "status": {
                "phase": "Ready", "device": "/dev/ublkb0",
                "integrity": {
                    "totals": counts(7, 7, unrecoverable), "scrubbing": false, "scrubPasses": 4, "scrubEvery": "7d",
                    "nextScrub": "2026-11-27T10:00:00Z",
                    "lastScrubPass": {"startedAt": "2026-11-20T09:00:00Z", "completedAt": "2026-11-20T10:00:00Z", "stripes": 1000, "mismatches": 7, "repaired": 7, "unrecoverableRows": 0},
                    "members": []
                },
                "members": [
                    {"path": "/dev/sdb", "role": "data", "state": "ok", "slot": 0, "wwn": "naa.5000cca26a819e61", "integrity": counts(0, 0, 0)},
                    {"path": "/dev/sdc", "role": "data", "state": "ok", "slot": 2, "wwn": "0x5000CCA26A819E69", "integrity": counts(7, 7, 0)},
                    {"path": "/dev/sdq3", "role": "data", "state": "ok", "slot": 3}
                ],
                "conditions": [
                    {"type": "Ready", "status": "True", "reason": "Ready"},
                    {"type": "BitRot", "status": rot, "reason": if rot == "True" { "MismatchesRose" } else { "Clean" },
                     "message": if rot == "True" { json!("slot 2 (/dev/sdc): 8 mismatch(es) (L1 1, L2 0, L3 7), 7 repaired, last 2026-11-20T10:00:00Z") } else { Value::Null },
                     "lastTransitionTime": since}
                ]
            }
        })
    }

    fn index() -> Index {
        let mut i = Index::default();
        i.add("forge1", "drive", &[
            json!({"metadata": {"name": "5000cca26a819e61"}, "status": {"name": "sdb", "serial": "NET001", "bay": 0, "wwid": "naa.5000cca26a819e61"}}),
            json!({"metadata": {"name": "5000cca26a819e69"}, "status": {"name": "sdc", "serial": "NET003", "bay": 3, "wwid": "naa.5000cca26a819e69"}}),
            json!({"metadata": {"name": "nvme-x"}, "status": {"name": "sdq", "serial": "NET017", "bay": 17}}),
        ]);
        i.add("storm-b", "drive:@storm-b", &[json!({"metadata": {"name": "b1"}, "status": {"name": "sdb", "serial": "BEE001", "wwid": "naa.5000c500aaaa0001"}})]);
        i
    }

    #[test]
    fn wwns_compare_however_they_are_written() {
        for w in ["naa.5000CCA26A819E69", "0x5000cca26a819e69", "wwn-0x5000cca26a819e69", "5000cca26a819e69-part2"] {
            assert_eq!(norm_wwn(w), "5000cca26a819e69", "{w}");
        }
    }

    #[test]
    fn a_member_is_its_drive_by_wwn_or_by_name_on_its_node() {
        let r = set_row(&md0("True", "2026-11-20T10:00:00Z", 0), &index()).unwrap();
        let m = r["members"].as_array().unwrap();
        assert_eq!(m[0]["drive"]["component"], "drive:drive:5000cca26a819e61");
        assert_eq!(m[1]["drive"]["component"], "drive:drive:5000cca26a819e69", "upper case 0x… is the same WWN");
        assert_eq!(m[1]["drive"]["serial"], "NET003");
        assert_eq!(m[2]["drive"]["component"], "drive:drive:nvme-x", "no WWN: sdq3 is a partition of sdq on forge1");
        assert_eq!(m[1]["counts"]["mismatches"], 8);
        assert_eq!(m[1]["counts"]["l3"], 7);
        assert!(m[2]["counts"].is_null(), "a member with no counters says none");
        assert_eq!(r["integrity"]["totals"]["repaired"], 7);
        assert_eq!(r["integrity"]["scrubPasses"], 4);
        assert_eq!(r["integrity"]["nextScrub"], "2026-11-27T10:00:00Z");
        // storm-b's sdb is not forge1's.
        assert!(index().find("forge1", None, Some("/dev/sdx")).is_none());
        assert_eq!(index().find("storm-b", None, Some("/dev/sdb")).unwrap()["serial"], "BEE001");
    }

    #[test]
    fn a_set_without_integrity_says_none() {
        let mut o = md0("False", "1970-01-01T00:00:00Z", 0);
        o["status"].as_object_mut().unwrap().remove("integrity");
        o["status"].as_object_mut().unwrap().remove("conditions");
        let r = set_row(&o, &index()).unwrap();
        assert!(r["integrity"].is_null() && r["bitrot"].is_null());
        assert!(alarm(&r, 0).is_none());
    }

    #[test]
    fn bitrot_on_is_amber_with_the_members_named() {
        let r = set_row(&md0("True", "2026-11-20T10:00:00Z", 0), &index()).unwrap();
        let now = epoch(Some("2026-11-20T11:00:00Z")).unwrap();
        let a = alarm(&r, now).unwrap();
        assert_eq!(a.level, "bitrot");
        assert_eq!(a.text, "Bit rot on RAID set md0 on forge1.g16.lo: slot 2 (/dev/sdc): 8 mismatch(es) (L1 1, L2 0, L3 7), 7 repaired, last 2026-11-20T10:00:00Z");
        let c = component(&a, "drive");
        assert!(matches!(c.health, Health::Warn) && c.kind == "alarm");
        assert_eq!(c.id, "drive:alarm:bitrot:md0");
        assert_eq!(c.link.as_deref(), Some("#/drives/integrity?set=md0"));
    }

    #[test]
    fn rows_past_parity_in_the_last_day_are_red() {
        let r = set_row(&md0("True", "2026-11-20T10:00:00Z", 2), &index()).unwrap();
        let now = epoch(Some("2026-11-20T11:00:00Z")).unwrap();
        let a = alarm(&r, now).unwrap();
        assert_eq!(a.level, "lost");
        assert!(a.text.ends_with(" — 2 row(s) past parity: data lost"), "{}", a.text);
        assert!(matches!(component(&a, "drive").health, Health::Error));
        // A day later the rows are old news; BitRot alone is amber.
        assert_eq!(alarm(&r, now + DAY).unwrap().level, "bitrot");
    }

    #[test]
    fn off_stays_an_hour_then_goes() {
        let r = set_row(&md0("False", "2026-11-20T10:00:00Z", 0), &index()).unwrap();
        let off = epoch(Some("2026-11-20T10:00:00Z")).unwrap();
        let a = alarm(&r, off + 720).unwrap();
        assert_eq!(a.level, "clearing");
        assert_eq!(a.text, "No new bit rot on RAID set md0 on forge1.g16.lo for 12 min (the alarm clears after an hour)");
        assert!(alarm(&r, off + CLEAR_AFTER).is_none());
        let never = set_row(&md0("False", "1970-01-01T00:00:00Z", 0), &index()).unwrap();
        assert!(alarm(&never, off).is_none(), "a set that never rotted raises nothing");
    }

    #[test]
    fn the_day_is_what_rose_since_a_day_ago() {
        let mut s = Seen::default();
        let t0 = 1_000_000u64;
        // Every minute for 30 h; slot 2's L3 rises by one each hour.
        for min in 0..=(30 * 60u64) {
            let now = t0 + min * 60;
            let mut o = md0("True", "2026-11-20T10:00:00Z", 0);
            let n = min / 60;
            o["status"]["members"][1]["integrity"]["checksumMismatches"]["l3"] = json!(n);
            o["status"]["integrity"]["totals"]["checksumMismatches"]["l3"] = json!(n);
            s.take(vec![set_row(&o, &index()).unwrap()], now);
        }
        let h = &s.history["md0/0x5000CCA26A819E69"];
        assert!(h.len() <= (DAY / SAMPLE_EVERY) as usize + 2, "a day of samples, not more: {}", h.len());
        let a = s.answer();
        let day = &a["sets"][0]["members"][1]["day"];
        assert_eq!(day["full"], true);
        let rose = day["counts"]["l3"].as_u64().unwrap();
        assert!((24..=25).contains(&rose), "about a day's rise: {rose}");
        assert_eq!(a["sets"][0]["integrity"]["day"]["counts"]["l3"], day["counts"]["l3"]);
        assert_eq!(a["sets"][0]["members"][0]["day"]["counts"]["mismatches"], 0, "slot 0 found nothing");
    }

    #[test]
    fn a_young_console_says_since_when_and_a_reset_starts_over() {
        let mut s = Seen::default();
        let o = md0("True", "2026-11-20T10:00:00Z", 0);
        s.take(vec![set_row(&o, &index()).unwrap()], 5000);
        let mut o2 = o.clone();
        o2["status"]["integrity"]["totals"]["blocksRepaired"] = json!(9);
        s.take(vec![set_row(&o2, &index()).unwrap()], 5000 + 3600);
        let d = &s.answer()["sets"][0]["integrity"]["day"];
        assert_eq!((d["since"].as_u64(), d["full"].as_bool(), d["counts"]["repaired"].as_u64()), (Some(5000), Some(false), Some(2)));
        // Counters back to zero (a set made anew under the name): start over.
        let mut o3 = o.clone();
        o3["status"]["integrity"]["totals"]["blocksRepaired"] = json!(0);
        s.take(vec![set_row(&o3, &index()).unwrap()], 5000 + 7200);
        let d = &s.answer()["sets"][0]["integrity"]["day"];
        assert_eq!((d["since"].as_u64(), d["counts"]["repaired"].as_u64()), (Some(5000 + 7200), Some(0)));
        // A set gone takes its samples.
        s.take(vec![], 9000);
        assert!(s.history.is_empty());
        s.failed("the apiserver serves no raidsets".into(), true);
        assert_eq!(s.answer()["absent"], true);
    }
}
