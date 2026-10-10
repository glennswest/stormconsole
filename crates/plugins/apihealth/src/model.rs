//! The shapes the node's API health arrives in, and what each one means.
//!
//! Two sources say the same thing (stormcos#458):
//! - **PID 1's summary**, `/run/stormpump/health.json` (stormpump#127):
//!   `{updated, worst, apis: [...]}`, PID 1's own probes (`source:
//!   "stormpump"`, `running`) and every container's stormd merged in
//!   (`source: "stormd"`, `container`, `file_age_secs`, `stale`,
//!   `reported_state`).
//! - **A stormd's own** `GET /api/v1/health/apis` (stormd#49): `{items:
//!   [...]}`, the same per-API fields without the merge's.
//!
//! Read leniently: a field the writer leaves out is `None`, never a parse
//! failure, so an older or newer writer still shows what it does say.

use chrono::{DateTime, Utc};
use console_core::Health;
use serde::Serialize;
use serde_json::Value;

/// stormd's states (stormd#49), which PID 1 uses too.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Unknown,
    Healthy,
    Slow,
    Stalled,
    Down,
}

impl State {
    pub fn parse(s: &str) -> State {
        match s {
            "healthy" => State::Healthy,
            "slow" => State::Slow,
            "stalled" => State::Stalled,
            "down" => State::Down,
            _ => State::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            State::Unknown => "unknown",
            State::Healthy => "healthy",
            State::Slow => "slow",
            State::Stalled => "stalled",
            State::Down => "down",
        }
    }

    /// Worse is larger: the node's state is its worst API's.
    pub fn rank(self) -> u8 {
        match self {
            State::Healthy => 0,
            State::Unknown => 1,
            State::Slow => 2,
            State::Down => 3,
            State::Stalled => 4,
        }
    }
}

/// One API, from either source.
#[derive(Clone, Debug, Serialize)]
pub struct Api {
    /// `stormpump` (PID 1's own probe) or `stormd`.
    pub source: String,
    /// The container whose stormd reported it, when a stormd did.
    pub container: Option<String>,
    /// The process (stormd) or boot unit (PID 1) that serves it.
    pub process: String,
    /// The API's declared name; empty for a container file PID 1 could
    /// not read, which stands for the whole container.
    pub api: String,
    pub url: String,
    pub state: State,
    pub since: Option<DateTime<Utc>>,
    /// PID 1 only: whether the unit runs. A unit that does not keeps its
    /// last state, which is then history, not a live stall.
    pub running: Option<bool>,
    pub last_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub p99_ms: Option<u64>,
    pub budget_p50_ms: Option<u64>,
    pub budget_p99_ms: Option<u64>,
    pub last_error: Option<String>,
    pub last_check: Option<DateTime<Utc>>,
    pub checks: Option<u64>,
    pub interval_secs: Option<u64>,
    /// The container's stormd stopped rewriting its file: PID 1 calls
    /// every API in it stalled, and keeps what the file said.
    pub stale: bool,
    pub reported_state: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(String::from)
}

fn n(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

fn t(v: &Value, k: &str) -> Option<DateTime<Utc>> {
    v.get(k).and_then(Value::as_str).and_then(|x| DateTime::parse_from_rfc3339(x).ok()).map(|d| d.with_timezone(&Utc))
}

impl Api {
    /// One entry of either shape. `container` names the stormd an item
    /// came from when the item does not say (the stormd fallback).
    pub fn from_value(v: &Value, container: Option<&str>) -> Api {
        let source = s(v, "source").unwrap_or_else(|| "stormd".into());
        Api {
            container: s(v, "container").or_else(|| container.map(String::from)),
            process: s(v, "process").unwrap_or_default(),
            api: s(v, "api").unwrap_or_default(),
            url: s(v, "url").unwrap_or_default(),
            state: v.get("state").and_then(Value::as_str).map(State::parse).unwrap_or(State::Unknown),
            since: t(v, "since"),
            running: v.get("running").and_then(Value::as_bool),
            last_ms: n(v, "last_ms"),
            p50_ms: n(v, "p50_ms"),
            p99_ms: n(v, "p99_ms"),
            budget_p50_ms: n(v, "budget_p50_ms"),
            budget_p99_ms: n(v, "budget_p99_ms"),
            last_error: s(v, "last_error"),
            last_check: t(v, "last_check"),
            checks: n(v, "checks"),
            interval_secs: n(v, "interval_secs"),
            stale: v.get("stale").and_then(Value::as_bool).unwrap_or(false),
            reported_state: s(v, "reported_state"),
            source,
        }
    }

    /// Who serves it, as a person names it: the process, else the
    /// container (a container file that could not be read has neither
    /// process nor API).
    pub fn service(&self) -> String {
        if !self.process.is_empty() {
            self.process.clone()
        } else {
            self.container.clone().unwrap_or_else(|| "?".into())
        }
    }

    /// A stable key: where it was reported, what serves it, which API.
    pub fn key(&self) -> String {
        let at = match &self.container {
            Some(c) => c.clone(),
            None => "pid1".into(),
        };
        format!("{at}/{}/{}", self.process, self.api)
    }

    /// The unit is known not to run: its state is the last one seen.
    pub fn not_running(&self) -> bool {
        self.running == Some(false)
    }

    /// An alert: stalled or down, on something that runs.
    pub fn alerting(&self) -> bool {
        matches!(self.state, State::Stalled | State::Down) && !self.not_running()
    }

    pub fn health(&self) -> Health {
        if self.not_running() {
            return Health::Idle;
        }
        match self.state {
            State::Healthy => Health::Ok,
            State::Slow => Health::Warn,
            State::Stalled | State::Down => Health::Error,
            State::Unknown => Health::Unknown,
        }
    }

    /// How long it has been in its state, at `now`.
    pub fn for_secs(&self, now: DateTime<Utc>) -> Option<u64> {
        self.since.map(|s| (now - s).num_seconds().max(0) as u64)
    }

    /// The probe, as it is named in an alert: `<service> API <api>`.
    pub fn name(&self) -> String {
        if self.api.is_empty() {
            format!("{} (its API health file)", self.service())
        } else {
            format!("{} API {}", self.service(), self.api)
        }
    }

    /// The sentence: the service, the probe, the state and how long, and
    /// why. An alert reads on its own, without the row around it.
    pub fn sentence(&self, now: DateTime<Utc>) -> String {
        let how_long = self.for_secs(now).map(|s| format!(" for {}", duration(s))).unwrap_or_default();
        let why = self.last_error.as_deref().filter(|e| !e.is_empty());
        let mut out = match self.state {
            State::Unknown => format!("{} not probed yet", self.name()),
            State::Healthy => match self.last_ms {
                Some(ms) => format!("{} healthy{how_long}, {ms} ms", self.name()),
                None => format!("{} healthy{how_long}", self.name()),
            },
            State::Slow => format!("{} slow{how_long}: {}", self.name(), self.latency_line()),
            State::Stalled => {
                format!("{} STALLED{how_long}{}", self.name(), why.map(|w| format!(" — {w}")).unwrap_or_default())
            }
            State::Down => {
                format!("{} DOWN{how_long}{}", self.name(), why.map(|w| format!(" — {w}")).unwrap_or_default())
            }
        };
        if self.stale {
            if let Some(r) = &self.reported_state {
                out.push_str(&format!(" (its stormd last said {r})"));
            }
        }
        if self.not_running() {
            out = format!("{} — the unit is not running; this is the last state seen", out);
        }
        out
    }

    /// `last 812 ms, p50 300 / p99 812 ms, budget p50 50 / p99 200 ms`.
    pub fn latency_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(l) = self.last_ms {
            parts.push(format!("last {l} ms"));
        }
        if self.p50_ms.is_some() || self.p99_ms.is_some() {
            parts.push(format!("p50 {} / p99 {} ms", opt(self.p50_ms), opt(self.p99_ms)));
        }
        if self.budget_p50_ms.is_some() || self.budget_p99_ms.is_some() {
            parts.push(format!("budget p50 {} / p99 {} ms", opt(self.budget_p50_ms), opt(self.budget_p99_ms)));
        }
        if parts.is_empty() {
            "no answer timed yet".into()
        } else {
            parts.join(", ")
        }
    }
}

pub(crate) fn opt(v: Option<u64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "–".into())
}

/// `45s`, `6m 12s`, `2h 3m`, `3d 4h`.
pub fn duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// PID 1's summary: when it says it was written, and its APIs.
pub struct Summary {
    pub updated: Option<DateTime<Utc>>,
    pub apis: Vec<Api>,
}

/// `{updated, worst, apis}`. `Err` when it is not that shape at all.
pub fn parse_summary(text: &str) -> Result<Summary, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let list = v.get("apis").and_then(Value::as_array).ok_or("no \"apis\" list")?;
    Ok(Summary { updated: t(&v, "updated"), apis: list.iter().map(|a| Api::from_value(a, None)).collect() })
}

/// A stormd's `{items}`, each named for the stormd that served it.
pub fn parse_stormd(v: &Value, container: &str) -> Result<Vec<Api>, String> {
    let list = v.get("items").and_then(Value::as_array).ok_or("no \"items\" list")?;
    Ok(list.iter().map(|a| Api::from_value(a, Some(container))).collect())
}

/// Worst first, then by service and API, so the page and the alert bar
/// read the same way down.
pub fn order(apis: &mut [Api]) {
    apis.sort_by(|a, b| {
        (b.alerting(), b.state.rank(), a.service(), a.api.clone()).cmp(&(a.alerting(), a.state.rank(), b.service(), b.api.clone()))
    });
}
