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
//! **Not here:** the drive's PI and EEDP check failures, its medium, recovered and
//! hardware errors from the kernel's sense lines, the rates and a level. stormblock#648
//! samples them, and they reach the Drive object with stormdrive#160 (#394 here).

use std::collections::{BTreeMap, VecDeque};

use serde_json::{json, Value};

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

pub type Counts = BTreeMap<String, u64>;

/// The counters a `smart` block gives.
pub fn counts(smart: &Value) -> Counts {
    COUNTERS
        .iter()
        .filter_map(|(k, p)| smart.pointer(p).and_then(Value::as_u64).map(|v| (k.to_string(), v)))
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

/// One drive's counters, or `None` for a drive whose stormdrive gives none (NVMe, whose
/// counters are its health log; or no sample yet).
pub fn row(o: &Value, node: &str, prefix: &str) -> Option<Value> {
    let id = o.pointer("/metadata/name").and_then(Value::as_str)?;
    let health = o.pointer("/status/health").filter(|h| h.is_object())?;
    let smart = health.get("smart").filter(|s| s.is_object())?;
    let c = counts(smart);
    let s = |k: &str| smart.get(k).cloned().unwrap_or(Value::Null);
    Some(json!({
        "component": format!("{prefix}:drive:{id}"),
        "node": node,
        "id": id,
        "serial": o.pointer("/status/serial").cloned().unwrap_or(Value::Null),
        "source": s("source"),
        "counts": c,
        "predictedFailure": s("predicted_failure"),
        "ieWarning": s("ie_warning"),
        "status": health.get("status").cloned().unwrap_or(Value::Null),
        "collectedAt": collected(health),
        "notCollected": health.get("not_collected").cloned().unwrap_or(Value::Null),
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
    pub history: BTreeMap<String, VecDeque<Sample>>,
}

fn counts_of(r: &Value) -> Counts {
    r["counts"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_u64()?))).collect()
}

impl Seen {
    /// Take a reading at `now`: the rows replace the old; each drive gains a sample if
    /// its last is [`SAMPLE_EVERY`] old, and loses those older than a day (keeping the
    /// one a day back, the day's base). A drive no longer reported takes its samples.
    pub fn take(&mut self, rows: Vec<Value>, now: u64) {
        let mut live = std::collections::BTreeSet::new();
        for r in &rows {
            let Some(key) = r["component"].as_str() else { continue };
            live.insert(key.to_string());
            let c = counts_of(r);
            let h = self.history.entry(key.to_string()).or_default();
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
        self.history.retain(|k, _| live.contains(k));
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

    /// The answer of `GET /counters`: each drive with its day.
    pub fn answer(&self) -> Value {
        let drives: Vec<Value> = self
            .rows
            .iter()
            .map(|r| {
                let mut r = r.clone();
                let key = r["component"].as_str().unwrap_or_default().to_string();
                r["day"] = self.day(&key, &counts_of(&r));
                r
            })
            .collect();
        json!({"drives": drives, "read": self.read, "at": self.at})
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
}
