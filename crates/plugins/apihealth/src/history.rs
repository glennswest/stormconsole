//! The kept changes: `<history_dir>/<process>.jsonl`, one line per change
//! of state, written by stormd (#49) and PID 1 (stormpump#127) in one
//! format: `ts, process, api, url, from, to, from_secs, latency_ms, p50_ms,
//! p99_ms, error`. On a node `history_dir` is
//! `/system-data/history/api`, the kept volume (stormcos#456).
//!
//! Read on demand and only the tail of each file: a service that flaps
//! for a year has a long file, and the page wants the latest changes.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

/// How much of each file's end is read.
const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub ts: DateTime<Utc>,
    pub process: String,
    pub api: String,
    pub url: String,
    pub from: String,
    pub to: String,
    /// How long it had been `from`.
    pub from_secs: Option<u64>,
    pub latency_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub p99_ms: Option<u64>,
    pub error: Option<String>,
}

impl Change {
    pub fn parse(line: &str) -> Option<Change> {
        let v: Value = serde_json::from_str(line).ok()?;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        let n = |k: &str| v.get(k).and_then(Value::as_u64);
        let ts = DateTime::parse_from_rfc3339(v.get("ts")?.as_str()?).ok()?.with_timezone(&Utc);
        Some(Change {
            ts,
            process: s("process").unwrap_or_default(),
            api: s("api").unwrap_or_default(),
            url: s("url").unwrap_or_default(),
            from: s("from").unwrap_or_default(),
            to: s("to")?,
            from_secs: n("from_secs"),
            latency_ms: n("latency_ms"),
            p50_ms: n("p50_ms"),
            p99_ms: n("p99_ms"),
            error: s("error"),
        })
    }
}

/// What a read found: the changes (newest first), and what stopped it
/// from finding more.
#[derive(Clone, Debug, Default, Serialize)]
pub struct History {
    pub dir: String,
    /// The directory is there to read.
    pub available: bool,
    /// Why not, or which files could not be read.
    pub note: String,
    pub changes: Vec<Change>,
}

fn tail(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    // Started mid-file: the first line is a fragment.
    if start > 0 {
        text = text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default();
    }
    Ok(text)
}

/// The newest `limit` changes, narrowed to a process and an API when they
/// are given. A missing directory is an answer ("not kept here"), not an
/// error.
pub fn read(dir: &str, process: Option<&str>, api: Option<&str>, limit: usize) -> History {
    let mut out = History { dir: dir.to_string(), ..Default::default() };
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            out.note = format!(
                "no history at {dir} ({e}): the node keeps it in system-data, which is not mounted into the console here"
            );
            return out;
        }
    };
    out.available = true;
    let mut bad = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        let Some(stem) = name.strip_suffix(".jsonl") else { continue };
        if stem.starts_with('.') {
            continue;
        }
        // The file is the process's; skip the others when one is asked for.
        if process.is_some_and(|p| p != stem) {
            continue;
        }
        match tail(&path) {
            Ok(text) => out.changes.extend(
                text.lines()
                    .filter_map(Change::parse)
                    .filter(|c| process.is_none_or(|p| c.process == p) && api.is_none_or(|a| c.api == a)),
            ),
            Err(e) => bad.push(format!("{name}: {e}")),
        }
    }
    if !bad.is_empty() {
        out.note = format!("unreadable: {}", bad.join("; "));
    }
    out.changes.sort_by(|a, b| b.ts.cmp(&a.ts));
    out.changes.truncate(limit);
    out
}
