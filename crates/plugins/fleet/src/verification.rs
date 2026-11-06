//! What each node's verifier last found (#74).
//!
//! sectionsystems runs on every node and says, on `:9103/api/v1/status`, what
//! its last pass found: whether the node's images, boot files and running
//! binaries are the ones its release names. Read-only on both sides: it never
//! acts on what it finds, and neither does this.
//!
//! A pass is hours apart and at random, so a console that had only heard the
//! pass's line on the log group would have nothing to say after a restart.
//! Asking the node is one small request a minute per node, and the same
//! answer the node page shows.
//!
//! A node that does not answer has no verdict: sectionsystems is not on every
//! release yet (stormcos#227), and silence is not a failed verification.

use std::time::Duration;

use chrono::{DateTime, Utc};
use console_core::dial::Dial;
use console_core::{Health, Metric};
use serde_json::{json, Value};

pub const PORT: u16 = 9103;
/// How often the fleet row is brought up to date.
pub const POLL: Duration = Duration::from_secs(60);

/// A node's status, as far as its row needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// `pass`, `incomplete` or `fail`. None until the first pass finishes.
    result: Option<String>,
    finished: Option<String>,
    problems: u64,
    running: bool,
}

pub fn verdict(status: &Value) -> Verdict {
    let last = &status["last"];
    let counts = &last["counts"];
    Verdict {
        result: last["result"].as_str().map(str::to_string),
        finished: last["finished"].as_str().map(str::to_string),
        problems: ["fail", "missing", "error"].iter().filter_map(|k| counts[*k].as_u64()).sum(),
        running: status["running"].as_bool().unwrap_or(false),
    }
}

impl Verdict {
    /// What this adds to the node's health: only a pass that did not pass.
    pub fn health(&self) -> Option<Health> {
        match self.result.as_deref() {
            Some("fail") => Some(Health::Error),
            Some("incomplete") => Some(Health::Warn),
            _ => None,
        }
    }

    /// `answering` is false when this is the last thing the node said.
    pub fn metric(&self, now: DateTime<Utc>, answering: bool) -> Metric {
        let age = self
            .finished
            .as_deref()
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|t| stormview::format_duration((now - t.with_timezone(&Utc)).num_seconds().max(0)));
        let problems = match self.problems {
            1 => ", 1 problem".to_string(),
            n => format!(", {n} problems"),
        };
        let (mut text, tone) = match self.result.as_deref() {
            Some("pass") => ("passed".to_string(), "ok"),
            Some("incomplete") => (format!("incomplete{problems}"), "warn"),
            Some("fail") => (format!("FAILED{problems}"), "error"),
            Some(other) => (other.to_string(), "muted"),
            None if self.running => ("first pass running".to_string(), "muted"),
            None => ("no pass yet".to_string(), "muted"),
        };
        if let Some(age) = age {
            text.push_str(&format!(", {age} ago"));
        }
        if !answering {
            text.push_str(", last known");
        }
        Metric::new("verification", text).tone(tone)
    }
}

/// What the node page shows: the pass, and the findings that are not as they
/// should be. `unverifiable` is by nature (a blank, a workload), never a
/// problem, and there can be a hundred of them, so it stays a count.
pub fn page(host: &str, status: &Value) -> Value {
    let rank = |f: &Value| match f["status"].as_str() {
        Some("fail") => 0,
        Some("missing") => 1,
        _ => 2,
    };
    let mut problems: Vec<Value> = status["last"]["findings"]
        .as_array()
        .map(|all| all.iter().filter(|f| matches!(f["status"].as_str(), Some("fail" | "missing" | "error"))).cloned().collect())
        .unwrap_or_default();
    problems.sort_by_key(rank);
    let mut last = status["last"].clone();
    if let Some(o) = last.as_object_mut() {
        o.remove("findings");
    }
    json!({
        "host": host,
        "answered": true,
        "version": status["version"],
        "running": status["running"],
        "booted": status["booted"],
        "next": status["next"],
        "reporting": status["reporting"],
        "last": last,
        "problems": problems,
    })
}

/// Ask a node. `Err` says why there is no answer.
pub async fn ask(dial: &Dial, addr: &str) -> Result<Value, String> {
    let route = dial.route(&format!("http://{addr}:{PORT}")).await?;
    let resp = route
        .client
        .get(format!("{}/api/v1/status", route.base))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| console_core::dial::chain(&e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("it answered {status}"));
    }
    let body: Value = resp.json().await.map_err(|e| format!("its answer could not be read: {e}"))?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The shape in sectionsystems' docs/api.md.
    fn status(result: &str) -> Value {
        json!({
            "node": "blade1", "version": "0.1.0", "running": false, "booted": true,
            "next": "2026-10-02T23:41:07Z",
            "reporting": {"events_off": "kube.url is not set"},
            "last": {
                "id": "20261002T174107.512Z", "trigger": "boot", "release": "11.50", "result": result,
                "finished": "2026-10-02T17:41:31.090Z",
                "counts": {"pass": 61, "fail": 1, "missing": 1, "unverifiable": 44, "error": 1},
                "findings": [
                    {"check": "golden", "asset": "stormlb", "status": "pass"},
                    {"check": "golden", "asset": "registry-data", "status": "unverifiable"},
                    {"check": "golden", "asset": "engine", "status": "error", "detail": "no answer"},
                    {"check": "boot", "asset": "initramfs", "status": "missing"},
                    {"check": "process", "asset": "stormlb:/usr/sbin/stormlb", "status": "fail", "expected": "e7", "actual": "aa", "pids": [2211]}
                ]
            }
        })
    }
    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-02T20:41:31Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_failed_pass_stands_out_and_a_passed_one_adds_nothing() {
        let v = verdict(&status("fail"));
        assert_eq!(v.health(), Some(Health::Error));
        let m = v.metric(at(), true);
        assert_eq!(m.tone.as_deref(), Some("error"));
        assert!(m.value.starts_with("FAILED, 3 problems, "), "{}", m.value);
        assert!(m.value.ends_with(" ago"), "{}", m.value);

        assert_eq!(verdict(&status("incomplete")).health(), Some(Health::Warn));
        let v = verdict(&status("pass"));
        assert_eq!(v.health(), None);
        assert!(v.metric(at(), true).value.starts_with("passed, "));
    }

    #[test]
    fn no_pass_yet_is_not_a_problem() {
        let v = verdict(&json!({"running": true, "booted": false, "last": null}));
        assert_eq!(v.health(), None);
        assert_eq!(v.metric(at(), true).value, "first pass running");
        let v = verdict(&json!({"running": false, "last": null}));
        assert_eq!(v.metric(at(), true).value, "no pass yet");
    }

    #[test]
    fn a_node_that_stopped_answering_keeps_what_it_last_said_and_says_so() {
        let m = verdict(&status("fail")).metric(at(), false);
        assert!(m.value.ends_with(", last known"), "{}", m.value);
        assert_eq!(m.tone.as_deref(), Some("error"));
    }

    #[test]
    fn the_page_gets_what_is_wrong_worst_first_and_not_the_rest() {
        let p = page("blade1", &status("fail"));
        let got: Vec<&str> = p["problems"].as_array().unwrap().iter().map(|f| f["status"].as_str().unwrap()).collect();
        assert_eq!(got, ["fail", "missing", "error"]);
        assert_eq!(p["problems"][0]["expected"], "e7");
        assert!(p["last"].get("findings").is_none());
        assert_eq!(p["last"]["counts"]["unverifiable"], 44);
        // Before the first pass.
        let p = page("blade1", &json!({"running": true, "last": null}));
        assert_eq!(p["problems"], json!([]));
        assert_eq!(p["last"], Value::Null);
    }
}
