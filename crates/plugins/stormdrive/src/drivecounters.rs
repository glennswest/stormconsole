//! A drive's own error and defect counters (#384), from every node's Drive objects.
//!
//! stormdrive reads them from the drive itself and puts them in the Drive object's
//! `status.health.smart` (stormdrive#22, #64, #81): for SAS the error counter log
//! pages (Read 0x03, Write 0x02, Verify 0x05: corrected and uncorrected, over the
//! drive's life) and the grown defect list; for SATA the ATA SMART attributes
//! (reallocated, pending, offline uncorrectable, reported uncorrectable, CRC). With
//! them, what the drive says of itself: a predicted failure or an IE warning.
//!
//! This keeps each drive's counters as read with the link (once a minute, the same
//! objects), and what rose in the last day from samples the console keeps, as the
//! RAID sets' integrity does ([`crate::integrity`]).
//!
//! With them (#394), what the node's engine samples of the drive (stormblock#648) and
//! stormdrive v0.60.0 joins into `status.health.errors` (stormdrive#160): the PI guard,
//! application-tag and reference-tag checks the drive failed, the same raised by the card
//! (EEDP), the medium, recovered and hardware errors in the kernel's sense lines, the
//! retry-corrected and non-medium errors, each kind's rate per minute, the defects grown
//! per day, and the engine's level (`settling|ok|degraded|failing`) with its `since`,
//! `streak` and `why`. stormdrive sets `DriveErrorsDegraded` and `DriveErrorsFailing` from
//! the level, and those raise an alarm here as a link's conditions do ([`crate::links`]).
//!
//! The engine's counters keep a day of their own: its sense counts start again when the
//! engine does, and that must not cost the drive's own counters their day.

use std::collections::{BTreeMap, VecDeque};

use console_core::{ComponentSummary, Health, Metric, Relation};
use serde_json::{json, Value};

use crate::links::{self, Alarm};

/// A sample kept every 10 min; the day is the newest less the one a day back.
pub const SAMPLE_EVERY: u64 = 600;
pub const DAY: u64 = 86_400;

/// The counters, as the drive gives them: `(key, pointer under smart)`. A key the drive
/// does not give is left out, never a 0.
const COUNTERS: &[(&str, &str)] = &[
    ("readUncorrected", "/read_errors/uncorrected"),
    ("writeUncorrected", "/write_errors/uncorrected"),
    ("verifyUncorrected", "/verify_errors/uncorrected"),
    ("grownDefects", "/grown_defects"),
    ("readCorrected", "/read_errors/corrected"),
    ("writeCorrected", "/write_errors/corrected"),
    ("verifyCorrected", "/verify_errors/corrected"),
    ("reallocated", "/reallocated_sectors"),
    ("pending", "/pending_sectors"),
    ("offlineUncorrectable", "/offline_uncorrectable"),
    ("reportedUncorrectable", "/reported_uncorrectable"),
    ("crcErrors", "/crc_errors"),
];

/// The engine's counters (#394), as `status.health.errors` gives them: `(key, pointer
/// under errors)`. Retry-corrected is the log page's "corrected with delay": what the
/// engine counts as degrading, unlike ECC on the fly.
const ENGINE: &[(&str, &str)] = &[
    ("piGuard", "/sense/piGuard"),
    ("piAppTag", "/sense/piAppTag"),
    ("piRefTag", "/sense/piRefTag"),
    ("eedpGuard", "/sense/eedpGuard"),
    ("eedpAppTag", "/sense/eedpAppTag"),
    ("eedpRefTag", "/sense/eedpRefTag"),
    ("mediumError", "/sense/mediumError"),
    ("recoveredError", "/sense/recoveredError"),
    ("hardwareError", "/sense/hardwareError"),
    ("readRetryCorrected", "/drive/read/correctedDelayed"),
    ("writeRetryCorrected", "/drive/write/correctedDelayed"),
    ("verifyRetryCorrected", "/drive/verify/correctedDelayed"),
    ("nonMedium", "/drive/nonMedium"),
];

/// How long an alarm stays after the engine says `ok` again, as a link's does.
pub const CLEAR_AFTER: u64 = links::CLEAR_AFTER;

pub type Counts = BTreeMap<String, u64>;

/// The counters a `smart` block gives.
pub fn counts(smart: &Value) -> Counts {
    COUNTERS
        .iter()
        .filter_map(|(k, p)| smart.pointer(p).and_then(Value::as_u64).map(|v| (k.to_string(), v)))
        .collect()
}

/// The engine's counters an `errors` block gives.
pub fn engine_counts(errors: &Value) -> Counts {
    ENGINE
        .iter()
        .filter_map(|(k, p)| errors.pointer(p).and_then(Value::as_u64).map(|v| (k.to_string(), v)))
        .collect()
}

/// `now - base`, per counter both have; `None` when any went back (the drive was
/// replaced under the same object, or its log reset): the base says nothing then.
pub fn since(now: &Counts, base: &Counts) -> Option<Counts> {
    let mut out = Counts::new();
    for (k, v) in now {
        match base.get(k) {
            Some(b) if v < b => return None,
            Some(b) => {
                out.insert(k.clone(), v - b);
            }
            None => {}
        }
    }
    Some(out)
}

/// When stormdrive last read the drive: a `SystemTime` serialized by serde.
fn collected(health: &Value) -> Option<u64> {
    health.pointer("/collected_at/secs_since_epoch").and_then(Value::as_u64)
}

/// The conditions stormdrive sets from the engine's level (stormdrive#160).
fn error_conditions(o: &Value) -> Vec<Value> {
    o.pointer("/status/conditions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| matches!(c.get("type").and_then(Value::as_str), Some("DriveErrorsDegraded" | "DriveErrorsFailing")))
        .cloned()
        .collect()
}

/// One drive's counters, or `None` for a drive whose stormdrive gives none: no `smart`
/// (NVMe, whose counters are its health log; or no sample yet) and no engine block.
pub fn row(o: &Value, node: &str, prefix: &str) -> Option<Value> {
    let id = o.pointer("/metadata/name").and_then(Value::as_str)?;
    let health = o.pointer("/status/health").filter(|h| h.is_object())?;
    let smart = health.get("smart").filter(|s| s.is_object());
    let errors = health.get("errors").filter(|e| e.get("level").and_then(Value::as_str).is_some());
    if smart.is_none() && errors.is_none() {
        return None;
    }
    let s = |k: &str| smart.and_then(|m| m.get(k)).cloned().unwrap_or(Value::Null);
    let st = |k: &str| o.pointer(&format!("/status/{k}")).cloned().unwrap_or(Value::Null);
    Some(json!({
        "component": format!("{prefix}:drive:{id}"),
        "node": node,
        "id": id,
        "serial": st("serial"),
        "name": st("name"),
        "bay": st("bay"),
        "enclosure": st("enclosure"),
        "source": s("source"),
        "counts": smart.map(counts).unwrap_or_default(),
        "predictedFailure": s("predicted_failure"),
        "ieWarning": s("ie_warning"),
        "status": health.get("status").cloned().unwrap_or(Value::Null),
        "collectedAt": collected(health),
        "notCollected": health.get("not_collected").cloned().unwrap_or(Value::Null),
        "errors": errors.cloned().unwrap_or(Value::Null),
        "errorCounts": errors.map(engine_counts),
        "conditions": error_conditions(o),
    }))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub at: u64,
    pub counts: Counts,
}

/// What has been read.
#[derive(Default)]
pub struct Seen {
    pub rows: Vec<Value>,
    pub read: bool,
    pub at: u64,
    /// Per drive: `component` for the drive's own counters, `component` + [`ENGINE_KEY`]
    /// for the engine's.
    pub history: BTreeMap<String, VecDeque<Sample>>,
    /// The worst level each drive's errors alarm has been at (`failing`, `degraded`),
    /// until it has been `ok` for [`CLEAR_AFTER`]: what a clearing alarm says it was.
    pub was: BTreeMap<String, &'static str>,
}

/// The suffix of the engine counters' history key.
pub const ENGINE_KEY: &str = "#engine";

fn counts_at(r: &Value, field: &str) -> Counts {
    r[field].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_u64()?))).collect()
}

fn counts_of(r: &Value) -> Counts {
    counts_at(r, "counts")
}

impl Seen {
    /// One sample of one set of counters: a new base if any went back, a sample if the
    /// last is [`SAMPLE_EVERY`] old, and none older than a day but the day's base.
    fn keep(&mut self, key: String, c: Counts, now: u64) {
        let h = self.history.entry(key).or_default();
        if h.back().is_some_and(|s| since(&c, &s.counts).is_none()) {
            h.clear();
        }
        if h.back().is_none_or(|s| now >= s.at + SAMPLE_EVERY) {
            h.push_back(Sample { at: now, counts: c });
        }
        while h.len() > 1 && h[1].at + DAY <= now {
            h.pop_front();
        }
    }

    /// Take a reading at `now`: the rows replace the old; each drive's counters, and the
    /// engine's apart, gain a sample (see [`Self::keep`]). A drive no longer reported
    /// takes its samples and its alarm's memory.
    pub fn take(&mut self, rows: Vec<Value>, now: u64) {
        let mut live = std::collections::BTreeSet::new();
        for r in &rows {
            let Some(key) = r["component"].as_str() else { continue };
            live.insert(key.to_string());
            self.keep(key.to_string(), counts_of(r), now);
            if r["errorCounts"].is_object() {
                let ekey = format!("{key}{ENGINE_KEY}");
                live.insert(ekey.clone());
                self.keep(ekey, counts_at(r, "errorCounts"), now);
            }
            let conds: Vec<Value> = r["conditions"].as_array().cloned().unwrap_or_default();
            if links::on(links::cond(&conds, "DriveErrorsFailing")) {
                self.was.insert(key.to_string(), "failing");
            } else if links::on(links::cond(&conds, "DriveErrorsDegraded")) {
                self.was.entry(key.to_string()).or_insert("degraded");
            } else if ok_for(r, now).is_some_and(|t| t >= CLEAR_AFTER) {
                self.was.remove(key);
            }
        }
        self.history.retain(|k, _| live.contains(k));
        self.was.retain(|k, _| live.contains(k));
        self.rows = rows;
        self.read = true;
        self.at = now;
    }

    /// What rose since the oldest kept sample: `{since, full, counts}`, or null.
    fn day(&self, key: &str, now: &Counts) -> Value {
        let Some(base) = self.history.get(key).and_then(|h| h.front()) else { return Value::Null };
        match since(now, &base.counts) {
            Some(d) => json!({"since": base.at, "full": self.at.saturating_sub(base.at) + SAMPLE_EVERY >= DAY, "counts": d}),
            None => Value::Null,
        }
    }

    /// The answer of `GET /counters`: each drive with its day, and the engine's.
    pub fn answer(&self) -> Value {
        let drives: Vec<Value> = self
            .rows
            .iter()
            .map(|r| {
                let mut r = r.clone();
                let key = r["component"].as_str().unwrap_or_default().to_string();
                r["day"] = self.day(&key, &counts_of(&r));
                r["errorsDay"] = if r["errorCounts"].is_object() {
                    self.day(&format!("{key}{ENGINE_KEY}"), &counts_at(&r, "errorCounts"))
                } else {
                    Value::Null
                };
                r
            })
            .collect();
        json!({"drives": drives, "read": self.read, "at": self.at})
    }

    /// The errors alarms now, one per drive that raises one ([`alarm`]).
    pub fn alarms(&self, now: u64) -> Vec<Alarm> {
        self.rows
            .iter()
            .filter_map(|r| {
                let key = r["component"].as_str()?;
                alarm(r, self.was.get(key).copied(), now)
            })
            .collect()
    }
}

/// How long the engine has said `ok`, when it does.
fn ok_for(r: &Value, now: u64) -> Option<u64> {
    let e = &r["errors"];
    if e["level"].as_str() != Some("ok") {
        return None;
    }
    Some(now.saturating_sub(e["since"].as_u64()?))
}

/// The alarm this drive's errors raise now, if any (#394), as a link's (#339):
///
/// - `DriveErrorsFailing` on → **failing**; else `DriveErrorsDegraded` on → **degraded**,
///   with the engine's `why`;
/// - both off, the console saw one on (`was`) and the engine `ok` for less than
///   [`CLEAR_AFTER`] → **clearing**, still in the list;
/// - otherwise none. stormdrive dates the off conditions too, at the engine's `since`,
///   so it is what the console saw, not the transition time, that says one was on.
pub fn alarm(r: &Value, was: Option<&'static str>, now: u64) -> Option<Alarm> {
    let conds: Vec<Value> = r["conditions"].as_array().cloned().unwrap_or_default();
    let (failing, degraded) = (links::cond(&conds, "DriveErrorsFailing"), links::cond(&conds, "DriveErrorsDegraded"));
    let component = r["component"].as_str()?.to_string();
    let said = |level: &str, c: Option<&Value>| {
        let why = c.and_then(|c| c.get("message")).and_then(Value::as_str).filter(|m| !m.is_empty() && !m.starts_with("drive errors "));
        let why = why.map(str::to_string).or_else(|| {
            let w: Vec<&str> = r["errors"]["why"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            (!w.is_empty()).then(|| w.join("; "))
        });
        let mut t = format!("Drive errors {level}: {}", links::place(r));
        if let Some(w) = why {
            t.push_str(&format!(" — {w}"));
        }
        t
    };
    if links::on(failing) {
        return Some(Alarm { component, level: "failing", text: said("failing", failing) });
    }
    if links::on(degraded) {
        return Some(Alarm { component, level: "degraded", text: said("degraded", degraded) });
    }
    let was = was?;
    let ok = ok_for(r, now)?;
    if ok >= CLEAR_AFTER {
        return None;
    }
    Some(Alarm {
        component,
        level: "clearing",
        text: format!(
            "Drive errors ok again for {}: {} (was {was}; the alarm clears after an hour ok)",
            links::minutes(ok),
            links::place(r)
        ),
    })
}

/// The alarm as a row of the feed, which the alarm bar shows on every page, linking to
/// the drive.
pub fn component(a: &Alarm, prefix_id: &str) -> ComponentSummary {
    let health = match a.level {
        "failing" => Health::Error,
        _ => Health::Warn,
    };
    ComponentSummary {
        id: format!("{prefix_id}:alarm:drive-errors:{}", a.component),
        kind: "alarm".into(),
        label: "Drive errors".into(),
        health,
        detail: a.text.clone(),
        metrics: vec![Metric::new("level", a.level).tone(if a.level == "failing" { "error" } else { "warn" })],
        actions: Vec::new(),
        relations: vec![Relation::belongs_to("drive", a.component.clone())],
        link: Some(format!("#/drives?drive={}", a.component)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A SAS drive's Drive object as stormdrive writes it (`HealthReport`, `SmartCounters`).
    fn sas(read_unc: u64, grown: u64) -> Value {
        json!({
            "metadata": {"name": "5000cca26a819e69"},
            "status": {"serial": "NET003", "health": {
                "status": "good", "media_errors": 0, "critical_warning": 0, "messages": [],
                "collected_at": {"secs_since_epoch": 1_763_700_000u64, "nanos_since_epoch": 0},
                "smart": {"source": "log_sense",
                          "read_errors": {"corrected": 120, "uncorrected": read_unc, "bytes": 9_000_000},
                          "write_errors": {"corrected": 4, "uncorrected": 0},
                          "verify_errors": {"corrected": 0},
                          "grown_defects": grown}
            }}
        })
    }

    #[test]
    fn a_sas_drive_gives_its_error_pages_and_grown_defects() {
        let r = row(&sas(2, 5), "forge1", "drive").unwrap();
        assert_eq!(r["component"], "drive:drive:5000cca26a819e69");
        assert_eq!((r["serial"].as_str(), r["source"].as_str(), r["collectedAt"].as_u64()), (Some("NET003"), Some("log_sense"), Some(1_763_700_000)));
        let c = counts_of(&r);
        assert_eq!(c.get("readUncorrected"), Some(&2));
        assert_eq!(c.get("readCorrected"), Some(&120));
        assert_eq!(c.get("grownDefects"), Some(&5));
        assert_eq!(c.get("writeUncorrected"), Some(&0));
        assert_eq!(c.get("verifyUncorrected"), None, "not given is left out, never 0");
        assert_eq!(c.get("reallocated"), None, "an ATA attribute is not a SAS drive's");
    }

    #[test]
    fn a_sata_drive_gives_its_attributes() {
        let o = json!({"metadata": {"name": "ata-1"}, "status": {"health": {"smart": {
            "source": "ata_smart", "reallocated_sectors": 8, "pending_sectors": 1, "offline_uncorrectable": 0, "crc_errors": 3,
            "predicted_failure": "reallocated sectors past threshold"}}}});
        let r = row(&o, "storm-b", "drive:@storm-b").unwrap();
        assert_eq!(r["component"], "drive:@storm-b:drive:ata-1");
        assert_eq!(counts_of(&r), [("crcErrors", 3), ("offlineUncorrectable", 0), ("pending", 1), ("reallocated", 8)].into_iter().map(|(k, v)| (k.to_string(), v)).collect());
        assert_eq!(r["predictedFailure"], "reallocated sectors past threshold");
    }

    #[test]
    fn no_counters_is_no_row() {
        let nvme = json!({"metadata": {"name": "nvme-1"}, "status": {"health": {"status": "good", "nvme": {"power_cycles": 3}}}});
        assert!(row(&nvme, "n", "drive").is_none(), "an NVMe drive's counters are its health log");
        assert!(row(&json!({"metadata": {"name": "x"}, "status": {}}), "n", "drive").is_none());
    }

    #[test]
    fn the_day_is_what_rose_since_a_day_back() {
        let mut s = Seen::default();
        let t0 = 1_000_000;
        s.take(vec![row(&sas(2, 5), "f", "drive").unwrap()], t0);
        s.take(vec![row(&sas(3, 5), "f", "drive").unwrap()], t0 + 60);
        let a = s.answer();
        let d = &a["drives"][0]["day"];
        assert_eq!((d["since"].as_u64(), d["full"].as_bool()), (Some(t0), Some(false)));
        assert_eq!((d["counts"]["readUncorrected"].as_u64(), d["counts"]["grownDefects"].as_u64()), (Some(1), Some(0)));
        assert_eq!(s.history["drive:drive:5000cca26a819e69"].len(), 1, "one sample per 10 min");
        // A day of samples: the base moves forward, and the day is full.
        let mut t = t0;
        for i in 0..150 {
            t += SAMPLE_EVERY;
            s.take(vec![row(&sas(3 + i / 100, 5 + i / 50), "f", "drive").unwrap()], t);
        }
        let d = &s.answer()["drives"][0]["day"];
        assert_eq!(d["full"].as_bool(), Some(true));
        assert!(t - d["since"].as_u64().unwrap() <= DAY + SAMPLE_EVERY, "{d}");
        assert_eq!((d["counts"]["readUncorrected"].as_u64(), d["counts"]["grownDefects"].as_u64()), (Some(1), Some(2)), "{d}");
    }

    #[test]
    fn counters_gone_back_start_over_and_a_gone_drive_is_forgotten() {
        let mut s = Seen::default();
        s.take(vec![row(&sas(9, 5), "f", "drive").unwrap()], 1000);
        s.take(vec![row(&sas(1, 5), "f", "drive").unwrap()], 2000);
        let d = &s.answer()["drives"][0]["day"];
        assert_eq!((d["since"].as_u64(), d["counts"]["readUncorrected"].as_u64()), (Some(2000), Some(0)), "a new base, not a negative");
        s.take(vec![], 3000);
        assert!(s.history.is_empty() && s.rows.is_empty());
        assert!(s.answer()["read"].as_bool().unwrap());
    }

    /// bay 3's Drive object with stormdrive v0.60.0's `status.health.errors` (#160): the
    /// engine's block for a drive with PI guard checks two samples running.
    fn engine(level: &str, failing: &str, degraded: &str, since: u64, pi_guard: u64) -> Value {
        let mut o = sas(1, 4);
        o["status"]["name"] = json!("sdb");
        o["status"]["bay"] = json!(3);
        o["status"]["enclosure"] = json!("shelf0");
        o["status"]["health"]["errors"] = json!({
            "level": level, "since": since, "at": since + 60, "streak": 2,
            "why": ["pi_guard 0.5/min", "read_uncorrected 0.0/min"],
            "piChecks": pi_guard + 1, "eedpChecks": 1, "uncorrected": 1, "mediumErrors": 1, "grownDefects": 4,
            "defectsPerDay": 1, "pendingPerDay": null,
            "drive": {"read": {"correctedFast": 900, "correctedDelayed": 2, "retries": 2, "corrected": 902, "uncorrected": 1},
                      "write": {"correctedFast": 10, "correctedDelayed": 0, "retries": 0, "corrected": 10, "uncorrected": 0},
                      "verify": null, "nonMedium": 3, "exception": "0x00/0x00", "grownDefects": 4,
                      "reallocated": null, "pending": null, "offlineUncorrectable": null},
            "sense": {"piGuard": pi_guard, "piAppTag": 0, "piRefTag": 1, "eedpGuard": 1, "eedpAppTag": 0, "eedpRefTag": 0,
                      "mediumError": 1, "recoveredError": 0, "hardwareError": 0},
            "perMin": {"pi_guard": 0.5, "read_uncorrected": 0.03}
        });
        let t = chrono::DateTime::from_timestamp(since as i64, 0).unwrap().to_rfc3339();
        o["status"]["conditions"] = json!([
            {"type": "Ready", "status": "True"},
            {"type": "DriveErrorsDegraded", "status": degraded, "reason": if degraded == "True" { "ErrorsMoving" } else { "DriveErrorsOk" },
             "message": if degraded == "True" { "pi_guard 0.5/min; read_uncorrected 0.0/min".to_string() } else { format!("drive errors {level}") }, "lastTransitionTime": t},
            {"type": "DriveErrorsFailing", "status": failing, "reason": if failing == "True" { "ErrorsFailing" } else { "DriveErrorsOk" },
             "message": if failing == "True" { "pi_guard 0.5/min; read_uncorrected 0.0/min".to_string() } else { format!("drive errors {level}") }, "lastTransitionTime": t}
        ]);
        o
    }

    #[test]
    fn the_engine_block_rides_the_row() {
        let r = row(&engine("failing", "True", "True", 5000, 2), "forge1", "drive").unwrap();
        assert_eq!(r["errors"]["level"], "failing");
        let e = counts_at(&r, "errorCounts");
        assert_eq!((e.get("piGuard"), e.get("piRefTag"), e.get("eedpGuard")), (Some(&2), Some(&1), Some(&1)));
        assert_eq!((e.get("readRetryCorrected"), e.get("nonMedium"), e.get("mediumError")), (Some(&2), Some(&3), Some(&1)));
        assert_eq!(e.get("verifyRetryCorrected"), None, "a page the drive does not give is left out");
        assert_eq!(r["conditions"].as_array().unwrap().len(), 2, "only the errors' conditions");
        assert_eq!((r["bay"].as_u64(), r["enclosure"].as_str()), (Some(3), Some("shelf0")));
        // A drive with the engine's block and no smart still has a row.
        let mut o = engine("ok", "False", "False", 5000, 0);
        o["status"]["health"].as_object_mut().unwrap().remove("smart");
        let r = row(&o, "forge1", "drive").unwrap();
        assert!(counts_of(&r).is_empty() && r["errorCounts"].is_object());
        // Not a block: no level.
        let mut o = sas(1, 4);
        o["status"]["health"]["errors"] = json!({"drive": {}});
        assert!(row(&o, "f", "drive").unwrap()["errors"].is_null());
    }

    #[test]
    fn the_engines_day_is_its_own() {
        let mut s = Seen::default();
        s.take(vec![row(&engine("degraded", "False", "True", 5000, 2), "f", "drive").unwrap()], 1000);
        s.take(vec![row(&engine("degraded", "False", "True", 5000, 5), "f", "drive").unwrap()], 1000 + SAMPLE_EVERY);
        let a = s.answer();
        assert_eq!(a["drives"][0]["errorsDay"]["counts"]["piGuard"].as_u64(), Some(3));
        // The engine restarts: its sense counts start again. The drive's own day stands.
        let mut o = engine("settling", "False", "True", 9000, 0);
        o["status"]["health"]["smart"]["read_errors"]["uncorrected"] = json!(2);
        s.take(vec![row(&o, "f", "drive").unwrap()], 1000 + 2 * SAMPLE_EVERY);
        let d = &s.answer()["drives"][0];
        assert_eq!((d["day"]["since"].as_u64(), d["day"]["counts"]["readUncorrected"].as_u64()), (Some(1000), Some(1)), "{d}");
        assert_eq!((d["errorsDay"]["since"].as_u64(), d["errorsDay"]["counts"]["piGuard"].as_u64()), (Some(1000 + 2 * SAMPLE_EVERY), Some(0)));
        s.take(vec![], 5000);
        assert!(s.history.is_empty() && s.was.is_empty());
    }

    #[test]
    fn failing_raises_with_the_engines_why() {
        let mut s = Seen::default();
        s.take(vec![row(&engine("failing", "True", "True", 5000, 2), "forge1", "drive").unwrap()], 5060);
        let a = s.alarms(5060);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].level, "failing");
        assert_eq!(a[0].text, "Drive errors failing: bay 3 of shelf0 on forge1 (NET003) — pi_guard 0.5/min; read_uncorrected 0.0/min");
        let c = component(&a[0], "drive");
        assert!(matches!(c.health, Health::Error) && c.kind == "alarm" && c.id == "drive:alarm:drive-errors:drive:drive:5000cca26a819e69");
        assert_eq!(c.link.as_deref(), Some("#/drives?drive=drive:drive:5000cca26a819e69"));
        // Degraded is amber.
        let r = row(&engine("degraded", "False", "True", 5000, 2), "forge1", "drive").unwrap();
        let a = alarm(&r, None, 5060).unwrap();
        assert!(a.level == "degraded" && matches!(component(&a, "drive").health, Health::Warn));
    }

    /// stormdrive dates the off conditions at the engine's `since` too: a drive the console
    /// never saw degraded raises nothing, however fresh its ok.
    #[test]
    fn ok_clears_after_an_hour_and_never_bad_raises_nothing() {
        let mut s = Seen::default();
        s.take(vec![row(&engine("ok", "False", "False", 10_000, 0), "forge1", "drive").unwrap()], 10_060);
        assert!(s.alarms(10_060).is_empty(), "never seen bad: nothing");

        let mut s = Seen::default();
        s.take(vec![row(&engine("failing", "True", "True", 9_000, 2), "forge1", "drive").unwrap()], 9_060);
        s.take(vec![row(&engine("degraded", "False", "True", 9_500, 2), "forge1", "drive").unwrap()], 9_560);
        assert_eq!(s.was.get("drive:drive:5000cca26a819e69"), Some(&"failing"), "the worst it was");
        let ok = row(&engine("ok", "False", "False", 10_000, 2), "forge1", "drive").unwrap();
        s.take(vec![ok.clone()], 10_720);
        let a = s.alarms(10_720);
        assert_eq!(a[0].level, "clearing");
        assert!(a[0].text.starts_with("Drive errors ok again for 12 min: bay 3 of shelf0"), "{}", a[0].text);
        assert!(a[0].text.contains("was failing"), "{}", a[0].text);
        s.take(vec![ok], 10_000 + CLEAR_AFTER);
        assert!(s.alarms(10_000 + CLEAR_AFTER).is_empty() && s.was.is_empty(), "an hour ok: cleared and forgotten");
        // Settling raises nothing of its own.
        let r = row(&engine("settling", "False", "False", 10_000, 0), "forge1", "drive").unwrap();
        assert!(alarm(&r, Some("degraded"), 10_060).is_none());
    }
}
