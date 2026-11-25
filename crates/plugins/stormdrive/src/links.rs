//! Every drive's SAS link (#339, stormblock#642, stormdrive#149).
//!
//! stormdrive v0.49.0 samples each SAS drive's link every minute, both sides:
//! the drive's own phy counters (log page 0x18) and the host phy facing it
//! (the HBA's, or a shelf expander's), with a rate per minute and a level
//! (`settling`, `ok`, `degraded`: anything moving, `failing`: loss of sync or
//! phy resets moving two samples running). It puts that in the Drive object's
//! `status.link`, with the conditions `LinkDegraded` and `LinkFailing`.
//!
//! This reads those objects from every node once a minute and keeps:
//!
//! - **a row per drive** for the drive page's Link panel and the level shown
//!   everywhere the drive appears ([`row`]);
//! - **the last [`HISTORY`] samples** of each drive's loss-of-sync rate, the
//!   trend when ironprom cannot draw one (another node's drive, or ironprom
//!   down);
//! - **an alarm** per drive whose condition is on ([`alarm`]): raised from the
//!   Drive's condition, never from a failed write, and kept for an hour after
//!   the link is `ok` again ([`CLEAR_AFTER`]).
//!
//! Nothing of it needs forge, stormcentral or a person at the console: it is
//! read on the node, from the node's own stormdrives (the owner: host first).

use std::collections::{BTreeMap, VecDeque};

use console_core::{ComponentSummary, Health, Metric, Relation};
use serde_json::{json, Value};

/// Samples kept per drive: an hour at stormdrive's one a minute.
pub const HISTORY: usize = 60;
/// How long an alarm stays after the link is `ok` again.
pub const CLEAR_AFTER: u64 = 3600;
/// How often the objects are read: stormdrive samples once a minute.
pub const EVERY: std::time::Duration = std::time::Duration::from_secs(60);

fn at<'a>(o: &'a Value, ptr: &str) -> &'a Value {
    o.pointer(ptr).unwrap_or(&Value::Null)
}

/// The conditions stormdrive sets for the link.
fn link_conditions(o: &Value) -> Vec<Value> {
    at(o, "/status/conditions")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| matches!(c.get("type").and_then(Value::as_str), Some("LinkDegraded" | "LinkFailing")))
        .cloned()
        .collect()
}

/// Which side the host phy is: `phy-1:2` faces an HBA, `phy-1:0:5` (host,
/// expander, phy) an expander's port.
pub fn host_side(host_phy: Option<&str>) -> &'static str {
    match host_phy {
        Some(p) if p.trim_start_matches("phy-").matches(':').count() >= 2 => "expander",
        _ => "host",
    }
}

/// One drive's link as the console shows it, or `None` for a drive with no
/// link reported (not SAS, or a stormdrive older than v0.49.0).
pub fn row(o: &Value, node: &str, prefix: &str) -> Option<Value> {
    let link = o.pointer("/status/link").filter(|l| l.is_object())?;
    let id = o.pointer("/metadata/name").and_then(Value::as_str)?;
    Some(json!({
        "component": format!("{prefix}:drive:{id}"),
        "node": node,
        "id": id,
        "name": at(o, "/status/name"),
        "serial": at(o, "/status/serial"),
        "bay": at(o, "/status/bay"),
        "enclosure": at(o, "/status/enclosure"),
        "hostSide": host_side(link.get("hostPhy").and_then(Value::as_str)),
        "link": link,
        "conditions": link_conditions(o),
    }))
}

/// A side's loss of DWORD sync per minute, summed over its phys; `None` when
/// the side gave no rate this sample (the first one, a reset counter).
pub fn sync_rate(side: &Value) -> Option<f64> {
    let rates = side.get("rates")?.as_array()?;
    if rates.is_empty() {
        return None;
    }
    Some(rates.iter().filter_map(|r| r.get("lossOfSync").and_then(Value::as_f64)).sum())
}

/// One point of the trend.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub at: u64,
    pub drive: Option<f64>,
    pub host: Option<f64>,
}

/// What has been read: the rows, which nodes answered, and each drive's trend.
#[derive(Default)]
pub struct Seen {
    pub rows: Vec<Value>,
    pub nodes: Vec<Value>,
    pub read: bool,
    pub history: BTreeMap<String, VecDeque<Point>>,
}

impl Seen {
    /// Take a new reading: the rows replace the old, and each drive's trend
    /// gains this sample unless stormdrive has not sampled since.
    pub fn take(&mut self, rows: Vec<Value>, nodes: Vec<Value>) {
        for r in &rows {
            let Some(key) = r.get("component").and_then(Value::as_str) else { continue };
            let link = &r["link"];
            let Some(t) = link.get("at").and_then(Value::as_u64) else { continue };
            let h = self.history.entry(key.to_string()).or_default();
            if h.back().is_some_and(|p| p.at >= t) {
                continue;
            }
            h.push_back(Point { at: t, drive: sync_rate(&link["drive"]), host: sync_rate(&link["host"]) });
            while h.len() > HISTORY {
                h.pop_front();
            }
        }
        // A drive no longer reported takes its trend with it.
        let live: std::collections::BTreeSet<&str> = rows.iter().filter_map(|r| r["component"].as_str()).collect();
        self.history.retain(|k, _| live.contains(k.as_str()));
        self.rows = rows;
        self.nodes = nodes;
        self.read = true;
    }

    /// The answer of `GET /links`: each row with its trend.
    pub fn answer(&self) -> Value {
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|r| {
                let mut r = r.clone();
                let key = r["component"].as_str().unwrap_or_default().to_string();
                let points: Vec<Value> = self
                    .history
                    .get(&key)
                    .into_iter()
                    .flatten()
                    .map(|p| json!({"at": p.at, "drive": p.drive, "host": p.host}))
                    .collect();
                r["history"] = json!(points);
                r
            })
            .collect();
        json!({"drives": rows, "nodes": self.nodes, "read": self.read})
    }
}

/// An alarm on one drive's link.
#[derive(Clone, Debug, PartialEq)]
pub struct Alarm {
    pub component: String,
    /// `failing`, `degraded`, or `clearing` (ok again, for less than an hour).
    pub level: &'static str,
    pub text: String,
}

pub(crate) fn cond<'a>(conds: &'a [Value], kind: &str) -> Option<&'a Value> {
    conds.iter().find(|c| c.get("type").and_then(Value::as_str) == Some(kind))
}

pub(crate) fn on(c: Option<&Value>) -> bool {
    c.and_then(|c| c.get("status")).and_then(Value::as_str) == Some("True")
}

/// When a condition last changed, if it ever has: stormdrive writes the
/// epoch for one that never did.
fn changed(c: Option<&Value>) -> Option<&str> {
    c.and_then(|c| c.get("lastTransitionTime")).and_then(Value::as_str).filter(|t| !t.starts_with("1970-"))
}

/// Where the drive is, in the words of the room.
pub(crate) fn place(r: &Value) -> String {
    let name = r["name"].as_str().filter(|s| !s.is_empty());
    let serial = r["serial"].as_str().filter(|s| !s.is_empty());
    let mut s = String::new();
    match r["bay"].as_u64() {
        Some(b) => s.push_str(&format!("bay {b}")),
        None => s.push_str(name.unwrap_or("a drive")),
    }
    if let Some(e) = r["enclosure"].as_str().filter(|e| !e.is_empty()) {
        s.push_str(&format!(" of {e}"));
    }
    if let Some(n) = r["node"].as_str() {
        s.push_str(&format!(" on {n}"));
    }
    if let Some(serial) = serial {
        s.push_str(&format!(" ({serial})"));
    }
    s
}

pub(crate) fn minutes(secs: u64) -> String {
    match secs / 60 {
        0 => "under a minute".into(),
        1 => "1 min".into(),
        m => format!("{m} min"),
    }
}

/// The alarm this drive's link raises now, if any (#339):
///
/// - `LinkFailing` on → **failing**; else `LinkDegraded` on → **degraded**;
/// - both off, one of them changed and the link `ok` for less than
///   [`CLEAR_AFTER`] → **clearing**, still in the list;
/// - otherwise none. A drive whose conditions never changed raises nothing.
pub fn alarm(r: &Value, now: u64) -> Option<Alarm> {
    let conds: Vec<Value> = r["conditions"].as_array().cloned().unwrap_or_default();
    let (failing, degraded) = (cond(&conds, "LinkFailing"), cond(&conds, "LinkDegraded"));
    let link = &r["link"];
    let component = r["component"].as_str()?.to_string();
    let message = link["message"].as_str().unwrap_or_default();
    let rec = link["recommendation"].as_str().filter(|s| !s.is_empty());
    let rate = link["negotiatedMbps"].as_u64().map(|m| format!(" at {}", gbit(m))).unwrap_or_default();
    let said = |level: &str| {
        let mut t = format!("Drive link {level}: {}{rate}", place(r));
        if !message.is_empty() {
            t.push_str(&format!(" — {message}"));
        }
        if let Some(rec) = rec {
            t.push_str(&format!(" — {rec}"));
        }
        t
    };
    if on(failing) {
        return Some(Alarm { component, level: "failing", text: said("failing") });
    }
    if on(degraded) {
        return Some(Alarm { component, level: "degraded", text: said("degraded") });
    }
    let last = match (changed(failing), changed(degraded)) {
        (Some(f), Some(d)) => Some(if f >= d { ("failing", f) } else { ("degraded", d) }),
        (Some(f), None) => Some(("failing", f)),
        (None, Some(d)) => Some(("degraded", d)),
        (None, None) => None,
    };
    let (was, _) = last?;
    if link["level"].as_str() != Some("ok") {
        return None;
    }
    let since = link["since"].as_u64()?;
    let ok_for = now.saturating_sub(since);
    if ok_for >= CLEAR_AFTER {
        return None;
    }
    Some(Alarm {
        component,
        level: "clearing",
        text: format!(
            "Drive link ok again for {}: {} (was {was}; the alarm clears after an hour ok)",
            minutes(ok_for),
            place(r)
        ),
    })
}

/// 12000 Mb/s → "12 Gbit", 1500 → "1.5 Gbit".
pub fn gbit(mbps: u64) -> String {
    if mbps.is_multiple_of(1000) {
        format!("{} Gbit", mbps / 1000)
    } else {
        format!("{:.1} Gbit", mbps as f64 / 1000.0)
    }
}

/// The alarm as a row of the feed, which the console's alarm bar shows on
/// every page, linking to the drive.
pub fn component(a: &Alarm, prefix_id: &str) -> ComponentSummary {
    let health = match a.level {
        "failing" => Health::Error,
        _ => Health::Warn,
    };
    ComponentSummary {
        id: format!("{prefix_id}:alarm:link:{}", a.component),
        kind: "alarm".into(),
        label: "Drive link".into(),
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

    /// forge1's bay 3 as stormdrive v0.49.0 writes it: the drive side's loss
    /// of sync 500 a minute, the HBA's 0, failing two samples running.
    fn bay3(level: &str, failing: &str, degraded: &str, since: u64) -> Value {
        json!({
            "metadata": {"name": "5000cca26a819e69"},
            "status": {
                "name": "sdb", "serial": "2EG3K1XJ", "bay": 3, "enclosure": "shelf0",
                "link": {
                    "at": since + 120, "level": level, "since": since, "streak": 2, "syncStreak": 2,
                    "hostPhy": "phy-1:2", "negotiatedMbps": 12000, "hostMaximumMbps": 12000, "hostMaximumHwMbps": 12000,
                    "drive": {"phys": [{"phy": "0", "rateMbps": 12000, "invalidDwords": 4294967295u64, "disparityErrors": 4294967295u64, "lossOfSync": 73506628, "phyResets": 0}],
                              "rates": [{"phy": "0", "invalidDwords": 0.0, "disparityErrors": 0.0, "lossOfSync": 500.0, "phyResets": 0.0}]},
                    "host": {"phys": [{"phy": "phy-1:2", "rateMbps": 12000, "invalidDwords": 0, "disparityErrors": 0, "lossOfSync": 0, "phyResets": 0}],
                             "rates": [{"phy": "phy-1:2", "invalidDwords": 0.0, "disparityErrors": 0.0, "lossOfSync": 0.0, "phyResets": 0.0}]},
                    "message": "drive phy 0 loss of sync 500/min",
                    "recommendation": "the drive's receiver or its slot: reseat; if it continues, swap bays or replace",
                    "policy": null, "maximum": "maximum 12 Gbit (hardware)"
                },
                "conditions": [
                    {"type": "Ready", "status": "True"},
                    {"type": "LinkDegraded", "status": degraded, "reason": "ErrorsMoving", "lastTransitionTime": "2026-11-17T10:00:00Z"},
                    {"type": "LinkFailing", "status": failing, "reason": "SyncLost", "lastTransitionTime": "2026-11-17T10:01:00Z"}
                ]
            }
        })
    }

    #[test]
    fn a_row_is_the_link_with_where_the_drive_is() {
        let r = row(&bay3("failing", "True", "True", 1000), "forge1", "drive").unwrap();
        assert_eq!(r["component"], "drive:drive:5000cca26a819e69");
        assert_eq!((r["bay"].as_u64(), r["serial"].as_str(), r["hostSide"].as_str()), (Some(3), Some("2EG3K1XJ"), Some("host")));
        assert_eq!(r["conditions"].as_array().unwrap().len(), 2, "only the link's conditions");
        assert!(row(&json!({"metadata": {"name": "x"}, "status": {"name": "sda"}}), "n", "drive").is_none(), "no link, no row");
    }

    #[test]
    fn behind_an_expander_the_host_side_says_so() {
        assert_eq!(host_side(Some("phy-1:2")), "host");
        assert_eq!(host_side(Some("phy-1:0:5")), "expander");
        assert_eq!(host_side(None), "host");
    }

    #[test]
    fn failing_raises_with_the_rate_and_the_recommendation() {
        let r = row(&bay3("failing", "True", "True", 1000), "forge1", "drive").unwrap();
        let a = alarm(&r, 2000).unwrap();
        assert_eq!(a.level, "failing");
        assert_eq!(
            a.text,
            "Drive link failing: bay 3 of shelf0 on forge1 (2EG3K1XJ) at 12 Gbit — drive phy 0 loss of sync 500/min — the drive's receiver or its slot: reseat; if it continues, swap bays or replace"
        );
        let c = component(&a, "drive");
        assert!(matches!(c.health, Health::Error) && c.kind == "alarm");
        assert_eq!(c.link.as_deref(), Some("#/drives?drive=drive:drive:5000cca26a819e69"));
        assert!(c.id.starts_with("drive:alarm:link:"), "under the plugin's prefix");
    }

    #[test]
    fn degraded_is_amber() {
        let r = row(&bay3("degraded", "False", "True", 1000), "forge1", "drive").unwrap();
        let a = alarm(&r, 2000).unwrap();
        assert_eq!(a.level, "degraded");
        assert!(matches!(component(&a, "drive").health, Health::Warn));
    }

    /// Cleared by the condition, kept until the link has been ok an hour.
    #[test]
    fn ok_again_stays_an_hour_then_goes() {
        let r = row(&bay3("ok", "False", "False", 10_000), "forge1", "drive").unwrap();
        let a = alarm(&r, 10_000 + 720).unwrap();
        assert_eq!(a.level, "clearing");
        assert!(a.text.starts_with("Drive link ok again for 12 min: bay 3"), "{}", a.text);
        assert!(a.text.contains("was failing"), "{}", a.text);
        assert!(alarm(&r, 10_000 + CLEAR_AFTER).is_none(), "an hour ok: cleared");
    }

    #[test]
    fn a_link_that_never_moved_raises_nothing() {
        let mut o = bay3("ok", "False", "False", 10_000);
        for c in o["status"]["conditions"].as_array_mut().unwrap() {
            c["lastTransitionTime"] = json!("1970-01-01T00:00:00Z");
        }
        let r = row(&o, "forge1", "drive").unwrap();
        assert!(alarm(&r, 10_060).is_none());
        let settling = row(&bay3("settling", "False", "False", 10_000), "forge1", "drive").unwrap();
        assert!(alarm(&settling, 10_060).is_none(), "settling after a reset is not ok, nor an alarm");
    }

    #[test]
    fn the_trend_keeps_an_hour_one_point_a_sample() {
        let mut s = Seen::default();
        for i in 0..70u64 {
            let mut o = bay3("failing", "True", "True", 1000);
            o["status"]["link"]["at"] = json!(1000 + i * 60);
            o["status"]["link"]["drive"]["rates"][0]["lossOfSync"] = json!(i as f64);
            let r = row(&o, "forge1", "drive").unwrap();
            s.take(vec![r.clone()], vec![]);
            s.take(vec![r], vec![]); // read twice between samples: one point
        }
        let h = &s.history["drive:drive:5000cca26a819e69"];
        assert_eq!(h.len(), HISTORY);
        assert_eq!((h.front().unwrap().drive, h.back().unwrap().drive, h.back().unwrap().host), (Some(10.0), Some(69.0), Some(0.0)));
        let a = s.answer();
        assert_eq!(a["drives"][0]["history"].as_array().unwrap().len(), HISTORY);
        s.take(vec![], vec![]);
        assert!(s.history.is_empty(), "a drive gone takes its trend");
    }

    #[test]
    fn rates_in_gbit() {
        assert_eq!((gbit(12000), gbit(6000), gbit(1500), gbit(22500)), ("12 Gbit".into(), "6 Gbit".into(), "1.5 Gbit".into(), "22.5 Gbit".into()));
        assert_eq!(sync_rate(&json!({"rates": []})), None);
        assert_eq!(sync_rate(&json!({"rates": [{"lossOfSync": 2.0}, {"lossOfSync": 3.5}]})), Some(5.5));
    }
}
