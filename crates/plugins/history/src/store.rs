//! Reading system-data: what each writer leaves there, as it leaves it.
//!
//! | path | writer | shape |
//! |---|---|---|
//! | `history/installs/<ts>.json` | stormblock's `/init` | a copy of the boot's record |
//! | `history/boots/<ts>.json` | stormblock's `/init` | `at, release, tag, cmdline, local_disk, install_config, handover` |
//! | `history/boots/<boot id>/stormpump.jsonl` | PID 1 | a `boot` header, then `t_ms, event, name, ms, detail` |
//! | `logs/kernel/<boot id>.log` | PID 1 | `seq [s.us] <pri> text`, and `-- … --` gap lines |
//! | `logs/kernel/crash/<dir>/` | PID 1 | `manifest.json`, `dmesg.txt`, each pstore record |
//! | `history/drives/<key>/<YYYY-MM>.jsonl` | stormdrive | a health record a line |
//! | `history/drives/<key>/log/<YYYY-MM>.jsonl` | stormdrive | what happened to the drive |
//! | `history/drives/<key>/summary.json` | stormdrive | the drive at a glance |
//! | `assets/<ts>-<boot id>.json` | stormdrive | the hardware at that boot, and what changed |
//!
//! **A boot is written in two places under two names.** stormblock runs
//! first and names its record by the time; PID 1 names its directory by the
//! boot id. [`Store::boots`] puts them back together, so a boot is one row:
//! by a `boot_id` in the record or a `stormblock.json` in the directory when
//! a writer gives them, otherwise by time and command line.
//!
//! The volume is append-mostly and written live, so a torn last line is
//! normal and skipped, and every read is bounded: nothing here reads a file
//! to its end without knowing how big it is.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;
use serde_json::Value;

/// A JSON record bigger than this is not one the writers make.
const RECORD_MAX: u64 = 8 << 20;
/// How much of a boot's timeline is read: PID 1 stops it at 4096 events.
const TIMELINE_MAX: u64 = 4 << 20;
/// How much of a month of a drive's records is read.
const MONTH_MAX: u64 = 16 << 20;
/// How much of a month is read for the drive's latest record.
const LAST_RECORD_MAX: u64 = 256 << 10;
/// How much of a kernel log is served: PID 1 stops one at 16 MiB.
const KERNEL_MAX: u64 = 32 << 20;
/// stormblock's record and PID 1's header are one boot if they are this close
/// and in this order, give or take the clock moving while it boots.
const PAIR_WITHIN_SECS: i64 = 15 * 60;
const PAIR_SLACK_SECS: i64 = 2 * 60;

/// Why one record could not be given.
#[derive(Debug, PartialEq)]
pub enum Miss {
    /// This node keeps no record at all.
    NoSystemData(String),
    /// A name that could not be a record's: never touches the disk.
    BadName(String),
    NotFound(String),
    /// There, and could not be read.
    Unreadable(String),
}

/// Files and directories only by a single plain name, so nothing asked for
/// reaches outside the record it names.
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 255 && !s.starts_with('.') && !s.contains(['/', '\\', '\0'])
}

fn checked(s: &str) -> Result<&str, Miss> {
    if plain_name(s) {
        Ok(s)
    } else {
        Err(Miss::BadName(format!("not a record's name: {s:?}")))
    }
}

/// A regular file, not whatever a link points at: the volume is read
/// read-only, and it still must not be able to serve the console's own files.
fn is_file(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file())
}

fn is_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// The entries of a directory, sorted, hidden and temporary files left out.
fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| plain_name(n)).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// The last `max` bytes, from a line's start, and whether that was all of it.
fn tail(path: &Path, max: u64) -> std::io::Result<(String, bool)> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf)?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        text = text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default();
    }
    Ok((text, start > 0))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
    let f = std::fs::File::open(path).map_err(|e| format!("{name}: {e}"))?;
    let mut buf = Vec::new();
    f.take(RECORD_MAX + 1).read_to_end(&mut buf).map_err(|e| format!("{name}: {e}"))?;
    if buf.len() as u64 > RECORD_MAX {
        return Err(format!("{name}: over {} MiB", RECORD_MAX >> 20));
    }
    serde_json::from_slice(&buf).map_err(|e| format!("{name}: {e}"))
}

/// The lines that parse. A torn last line does not, and is not an error.
fn json_lines(text: &str) -> impl DoubleEndedIterator<Item = Value> + '_ {
    text.lines().filter_map(|l| serde_json::from_str(l).ok())
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

/// `YYYYMMDDTHHMMSSZ`, the writers' file names and stormblock's `at`.
pub fn stamp(s: &str) -> Option<DateTime<Utc>> {
    NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ").ok().map(|t| t.and_utc())
}

/// The same command line, whichever writer kept it: stormblock's drops quotes
/// and backslashes and turns newlines into spaces.
fn same_cmdline(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace(['"', '\\'], "").split_whitespace().collect::<Vec<_>>().join(" ");
    norm(a) == norm(b)
}

/// A list, with what kept it from being longer.
#[derive(Debug, Serialize)]
pub struct Listing<T> {
    pub root: String,
    /// This node keeps a record at all.
    pub available: bool,
    /// Why not, or why this part is empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// How many there are; `items` is the newest of them.
    pub total: usize,
    /// What is there and could not be read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<String>,
    pub items: Vec<T>,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub root: String,
    pub available: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub boot_id: Option<String>,
    pub installs: usize,
    pub boots: usize,
    pub kernel_logs: usize,
    pub crashes: usize,
    pub drives: usize,
    pub assets: usize,
}

#[derive(Debug, Serialize)]
pub struct InstallRow {
    pub id: String,
    pub at: Option<DateTime<Utc>>,
    pub release: Option<String>,
    pub tag: Option<String>,
    /// The disk verdict and the drive it chose.
    pub disk: Option<String>,
    pub drive: Option<String>,
    /// What the handover says was installed over what.
    pub version: Option<String>,
    pub previous: Option<String>,
    pub carried: Option<Value>,
    pub install_config: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootRow {
    /// The boot id when PID 1 recorded the boot, else stormblock's name.
    pub id: String,
    pub boot_id: Option<String>,
    /// stormblock's file, when it recorded this boot.
    pub stormblock_file: Option<String>,
    /// How the two were joined, when both are here: `named` or `time`.
    pub joined: Option<&'static str>,
    pub at: Option<DateTime<Utc>>,
    pub release: Option<String>,
    pub kernel: Option<String>,
    pub hostname: Option<String>,
    pub tag: Option<String>,
    pub disk: Option<String>,
    /// This boot installed: the version it put down.
    pub installed: Option<String>,
    /// `running`, `crashed`, `shut down`, `no clean end` or `no timeline`.
    pub end: &'static str,
    /// The panic line, or the shutdown's reason.
    pub end_detail: Option<String>,
    pub all_running_ms: Option<u64>,
    /// PID 1 started this many times (a re-exec is one more).
    pub pid1_starts: usize,
    pub started: usize,
    pub exited: usize,
    pub refused: usize,
    pub events: usize,
    pub current: bool,
    pub kernel_log_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BootDetail {
    #[serde(flatten)]
    pub row: BootRow,
    pub cmdline: Option<String>,
    /// stormblock's record: the disk verdict, the inventory, the handover.
    pub stormblock: Option<Value>,
    /// PID 1's lines, in order, the header first.
    pub timeline: Vec<Value>,
    pub timeline_cut: bool,
    /// The assets record of this boot, when stormdrive wrote one.
    pub assets: Option<String>,
    pub crash: Option<CrashRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrashRow {
    pub dir: String,
    pub manifest: Value,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DriveRow {
    pub key: String,
    /// Who the drive is, as of its latest record.
    pub drive: Value,
    pub last_at: Option<String>,
    pub status: Option<String>,
    pub temperature_c: Option<i64>,
    pub node: Option<String>,
    pub findings: Value,
    /// stormdrive's summary, kept forever.
    pub summary: Value,
    pub months: usize,
}

#[derive(Debug, Serialize)]
pub struct DriveDetail {
    pub key: String,
    pub summary: Value,
    /// Newest first, and as many as were asked for.
    pub records: Vec<Value>,
    /// What happened to the drive, newest first.
    pub log: Vec<Value>,
    /// An older part of a month was not read.
    pub cut: bool,
}

#[derive(Debug, Serialize)]
pub struct AssetRow {
    pub id: String,
    pub at: Option<String>,
    pub boot_id: Option<String>,
    pub node: Option<String>,
    pub kernel: Option<String>,
    pub memory_total_bytes: Option<u64>,
    pub items: usize,
    pub changes: Value,
    pub changed_in_boot: Value,
    pub previous: Option<String>,
}

/// A boot as found, before it is a row.
struct Found {
    dir: Option<String>,
    timeline: Vec<Value>,
    cut: bool,
    flat: Option<(String, Value)>,
    joined: Option<&'static str>,
}

impl Found {
    fn header(&self) -> Option<&Value> {
        self.timeline.iter().find(|e| e["event"] == "boot")
    }

    fn wall(&self) -> Option<DateTime<Utc>> {
        let s = self.header()?.get("wall_unix")?.as_i64().filter(|s| *s > 0)?;
        DateTime::from_timestamp(s, 0)
    }

    fn flat_at(&self) -> Option<DateTime<Utc>> {
        let (name, rec) = self.flat.as_ref()?;
        rec.get("at").and_then(Value::as_str).and_then(stamp).or_else(|| stamp(name))
    }
}

pub struct Store {
    root: PathBuf,
    /// This boot's id, which is the one still being written.
    boot_id: Option<String>,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok().map(|s| s.trim().to_string());
        Store { root: root.into(), boot_id }
    }

    pub fn with_boot_id(mut self, id: Option<&str>) -> Self {
        self.boot_id = id.map(String::from);
        self
    }

    pub fn root(&self) -> String {
        self.root.display().to_string()
    }

    /// Whether this node keeps a record. stormblock makes `history/` when it
    /// mounts the volume, and leaves an empty directory on `/run` when it
    /// cannot, which is a record of nothing.
    pub fn available(&self) -> bool {
        self.root.join("history").is_dir()
    }

    fn none_here(&self) -> String {
        if self.root.exists() {
            format!("no system-data on this node: {} holds no records, so its volume is not mounted there", self.root())
        } else {
            format!("no system-data on this node: {} is not there (a diskless boot keeps no record)", self.root())
        }
    }

    fn need(&self) -> Result<(), Miss> {
        if self.available() {
            Ok(())
        } else {
            Err(Miss::NoSystemData(self.none_here()))
        }
    }

    fn listing<T>(&self, note: &str, total: usize, unreadable: Vec<String>, items: Vec<T>) -> Listing<T> {
        let available = self.available();
        let note = if !available {
            self.none_here()
        } else if total == 0 {
            note.to_string()
        } else {
            String::new()
        };
        Listing { root: self.root(), available, note, total, unreadable, items }
    }

    pub fn status(&self) -> Status {
        let available = self.available();
        let count = |dir: &str| names(&self.root.join(dir)).len();
        Status {
            root: self.root(),
            available,
            note: if available { String::new() } else { self.none_here() },
            boot_id: self.boot_id.clone(),
            installs: count("history/installs"),
            boots: if available { self.found().len() } else { 0 },
            kernel_logs: names(&self.root.join("logs/kernel")).iter().filter(|n| n.ends_with(".log")).count(),
            crashes: count("logs/kernel/crash"),
            drives: count("history/drives"),
            assets: count("assets"),
        }
    }

    // --- installs

    pub fn installs(&self, limit: usize) -> Listing<InstallRow> {
        let dir = self.root.join("history/installs");
        let all: Vec<String> = names(&dir).into_iter().filter(|n| n.ends_with(".json")).rev().collect();
        let mut bad = Vec::new();
        let mut items = Vec::new();
        for name in all.iter().take(limit) {
            match read_json(&dir.join(name)) {
                Ok(v) => items.push(install_row(name, &v)),
                Err(e) => bad.push(e),
            }
        }
        self.listing("no install recorded here yet", all.len(), bad, items)
    }

    pub fn install(&self, id: &str) -> Result<Value, Miss> {
        self.need()?;
        self.record(&self.root.join("history/installs"), id, "install")
    }

    fn record(&self, dir: &Path, id: &str, what: &str) -> Result<Value, Miss> {
        let p = dir.join(format!("{}.json", checked(id)?));
        if !is_file(&p) {
            return Err(Miss::NotFound(format!("no {what} {id} on this node")));
        }
        read_json(&p).map_err(Miss::Unreadable)
    }

    // --- boots

    /// Every boot, one each, newest first.
    fn found(&self) -> Vec<Found> {
        let dir = self.root.join("history/boots");
        let mut named: Vec<Found> = Vec::new();
        let mut flats: Vec<(String, Value)> = Vec::new();
        for name in names(&dir) {
            let p = dir.join(&name);
            if is_dir(&p) {
                let (timeline, cut) = match tail(&p.join("stormpump.jsonl"), TIMELINE_MAX) {
                    Ok((t, c)) => (json_lines(&t).collect(), c),
                    Err(_) => (Vec::new(), false),
                };
                // Where the writer's documentation puts stormblock's record.
                let flat = read_json(&p.join("stormblock.json")).ok().map(|v| ("stormblock.json".to_string(), v));
                let joined = flat.as_ref().map(|_| "named");
                named.push(Found { dir: Some(name), timeline, cut, flat, joined });
            } else if let (Some(stem), true) = (name.strip_suffix(".json"), is_file(&p)) {
                flats.push((stem.to_string(), read_json(&p).unwrap_or(Value::Null)));
            }
        }
        // A record that names its boot, then the newest record to the
        // earliest timeline at or after it. Newest first, so a boot that
        // never reached PID 1 cannot take the next boot's timeline.
        flats.sort_by(|a, b| b.0.cmp(&a.0));
        let mut alone = Vec::new();
        for flat in flats {
            if let Some(found) = string(&flat.1, "boot_id")
                .and_then(|id| named.iter_mut().find(|f| f.dir.as_deref() == Some(id.as_str()) && f.flat.is_none()))
            {
                found.flat = Some(flat);
                found.joined = Some("named");
            } else {
                alone.push(flat);
            }
        }
        for flat in alone {
            let at = flat.1.get("at").and_then(Value::as_str).and_then(stamp).or_else(|| stamp(&flat.0));
            let cmd = string(&flat.1, "cmdline");
            let best = at.and_then(|at| {
                named
                    .iter_mut()
                    .filter(|f| f.flat.is_none())
                    .filter_map(|f| {
                        let gap = (f.wall()? - at).num_seconds();
                        let same = match (&cmd, f.header().and_then(|h| string(h, "cmdline"))) {
                            (Some(a), Some(b)) => same_cmdline(a, &b),
                            _ => true,
                        };
                        ((-PAIR_SLACK_SECS..=PAIR_WITHIN_SECS).contains(&gap) && same).then_some((gap, f))
                    })
                    .min_by_key(|(gap, _)| *gap)
            });
            match best {
                Some((_, f)) => {
                    f.flat = Some(flat);
                    f.joined = Some("time");
                }
                None => named.push(Found { dir: None, timeline: Vec::new(), cut: false, flat: Some(flat), joined: None }),
            }
        }
        named.sort_by_key(|f| std::cmp::Reverse((f.wall().or_else(|| f.flat_at()), f.dir.clone())));
        named
    }

    fn row(&self, f: &Found) -> BootRow {
        let header = f.header();
        let rec = f.flat.as_ref().map(|(_, v)| v);
        let events = |kind: &'static str| f.timeline.iter().filter(move |e| e["event"] == kind);
        let crashed = events("crashed").next_back().map(|e| string(e, "detail").unwrap_or_default());
        let shutdown = events("shutdown").next_back().map(|e| string(e, "detail").unwrap_or_default());
        let current = f.dir.is_some() && f.dir == self.boot_id;
        let (end, end_detail) = if let Some(c) = crashed {
            ("crashed", Some(c))
        } else if current {
            ("running", None)
        } else if let Some(s) = shutdown {
            ("shut down", Some(s))
        } else if f.timeline.is_empty() {
            ("no timeline", None)
        } else {
            ("no clean end", None)
        };
        BootRow {
            id: f.dir.clone().or_else(|| f.flat.as_ref().map(|(n, _)| n.clone())).unwrap_or_default(),
            boot_id: f.dir.clone(),
            stormblock_file: f.flat.as_ref().map(|(n, _)| n.clone()),
            joined: f.joined,
            at: f.wall().or_else(|| f.flat_at()),
            release: rec.and_then(|r| string(r, "release")).or_else(|| header.and_then(|h| string(h, "release"))),
            kernel: header.and_then(|h| string(h, "kernel")),
            hostname: header.and_then(|h| string(h, "hostname")),
            tag: rec.and_then(|r| string(r, "tag")),
            disk: rec.and_then(|r| string(&r["local_disk"], "state")),
            installed: rec.and_then(|r| string(&r["handover"]["installed"], "version")),
            end,
            end_detail,
            all_running_ms: events("all-running").next().and_then(|e| e["t_ms"].as_u64()),
            pid1_starts: events("pid1").count(),
            started: events("started").count(),
            exited: events("exited").count(),
            refused: events("refused").count(),
            events: f.timeline.len(),
            current,
            kernel_log_bytes: f
                .dir
                .as_ref()
                .and_then(|d| std::fs::symlink_metadata(self.root.join("logs/kernel").join(format!("{d}.log"))).ok())
                .filter(|m| m.is_file())
                .map(|m| m.len()),
        }
    }

    pub fn boots(&self, limit: usize) -> Listing<BootRow> {
        let found = self.found();
        let items = found.iter().take(limit).map(|f| self.row(f)).collect();
        self.listing("no boot recorded here yet", found.len(), Vec::new(), items)
    }

    /// One boot, by its id or by stormblock's name for it, so a link from an
    /// install (which only has stormblock's name) reaches the whole boot.
    fn find(&self, id: &str) -> Result<Found, Miss> {
        self.need()?;
        checked(id)?;
        self.found()
            .into_iter()
            .find(|f| f.dir.as_deref() == Some(id) || f.flat.as_ref().is_some_and(|(n, _)| n == id))
            .ok_or_else(|| Miss::NotFound(format!("no boot {id} on this node")))
    }

    pub fn boot(&self, id: &str) -> Result<BootDetail, Miss> {
        let f = self.find(id)?;
        let row = self.row(&f);
        let assets = f.dir.as_ref().and_then(|d| {
            let suffix = format!("-{d}.json");
            names(&self.root.join("assets")).iter().rev().find(|n| n.ends_with(&suffix)).map(|n| n.trim_end_matches(".json").to_string())
        });
        let crash = f.dir.as_ref().and_then(|d| self.crash_row(d));
        let cmdline = f.header().and_then(|h| string(h, "cmdline")).or_else(|| f.flat.as_ref().and_then(|(_, r)| string(r, "cmdline")));
        Ok(BootDetail {
            row,
            cmdline,
            stormblock: f.flat.map(|(_, v)| v),
            timeline: f.timeline,
            timeline_cut: f.cut,
            assets,
            crash,
        })
    }

    /// A boot's kernel log, or its last `lines` lines.
    pub fn kernel(&self, id: &str, lines: Option<usize>) -> Result<String, Miss> {
        let f = self.find(id)?;
        let none = || Miss::NotFound(format!("no kernel log kept for boot {id}"));
        let dir = f.dir.ok_or_else(none)?;
        let p = self.root.join("logs/kernel").join(format!("{dir}.log"));
        if !is_file(&p) {
            return Err(none());
        }
        let max = lines.map_or(KERNEL_MAX, |n| ((n as u64).saturating_add(1)).saturating_mul(1024).min(KERNEL_MAX));
        let (text, _) = tail(&p, max).map_err(|e| Miss::Unreadable(format!("{dir}.log: {e}")))?;
        Ok(match lines {
            Some(n) => {
                let all: Vec<&str> = text.lines().collect();
                let mut out = all[all.len().saturating_sub(n)..].join("\n");
                out.push('\n');
                out
            }
            None => text,
        })
    }

    // --- kernel crashes

    fn crash_row(&self, dir: &str) -> Option<CrashRow> {
        let p = self.root.join("logs/kernel/crash").join(dir);
        if !is_dir(&p) {
            return None;
        }
        Some(CrashRow { dir: dir.to_string(), manifest: read_json(&p.join("manifest.json")).unwrap_or(Value::Null), files: names(&p) })
    }

    pub fn crashes(&self) -> Listing<CrashRow> {
        let mut items: Vec<CrashRow> = names(&self.root.join("logs/kernel/crash")).iter().filter_map(|d| self.crash_row(d)).collect();
        items.sort_by_key(|c| std::cmp::Reverse(c.manifest["record_unix"].as_u64().unwrap_or(0)));
        let total = items.len();
        self.listing("no kernel crash kept here", total, Vec::new(), items)
    }

    pub fn crash_file(&self, dir: &str, file: &str) -> Result<String, Miss> {
        self.need()?;
        let p = self.root.join("logs/kernel/crash").join(checked(dir)?).join(checked(file)?);
        if !is_file(&p) {
            return Err(Miss::NotFound(format!("no {file} in kernel crash {dir}")));
        }
        tail(&p, KERNEL_MAX).map(|(t, _)| t).map_err(|e| Miss::Unreadable(format!("{file}: {e}")))
    }

    // --- drives

    pub fn drives(&self, limit: usize) -> Listing<DriveRow> {
        let dir = self.root.join("history/drives");
        let all = names(&dir);
        let items: Vec<DriveRow> = all.iter().take(limit).map(|k| drive_row(&dir, k)).collect();
        self.listing(
            "no drive history here yet: stormdrive writes it once system-data is bound into its unit",
            all.len(),
            Vec::new(),
            items,
        )
    }

    pub fn drive(&self, key: &str, limit: usize) -> Result<DriveDetail, Miss> {
        self.need()?;
        let dir = self.root.join("history/drives").join(checked(key)?);
        if !is_dir(&dir) {
            return Err(Miss::NotFound(format!("no drive {key} in this node's history")));
        }
        let (records, cut_a) = newest_lines(&dir, limit);
        let (log, cut_b) = newest_lines(&dir.join("log"), limit);
        Ok(DriveDetail {
            key: key.to_string(),
            summary: read_json(&dir.join("summary.json")).unwrap_or(Value::Null),
            records,
            log,
            cut: cut_a || cut_b,
        })
    }

    // --- assets

    pub fn assets(&self, limit: usize) -> Listing<AssetRow> {
        let dir = self.root.join("assets");
        let all: Vec<String> = names(&dir).into_iter().filter(|n| n.ends_with(".json")).rev().collect();
        let mut bad = Vec::new();
        let mut items = Vec::new();
        for name in all.iter().take(limit) {
            match read_json(&dir.join(name)) {
                Ok(v) => items.push(asset_row(name, &v)),
                Err(e) => bad.push(e),
            }
        }
        self.listing(
            "no hardware record here yet: stormdrive writes one each boot once system-data is bound into its unit",
            all.len(),
            bad,
            items,
        )
    }

    pub fn asset(&self, id: &str) -> Result<Value, Miss> {
        self.need()?;
        self.record(&self.root.join("assets"), id, "hardware record")
    }
}

fn install_row(name: &str, v: &Value) -> InstallRow {
    let id = name.trim_end_matches(".json").to_string();
    let installed = &v["handover"]["installed"];
    InstallRow {
        at: v.get("at").and_then(Value::as_str).and_then(stamp).or_else(|| stamp(&id)),
        release: string(v, "release"),
        tag: string(v, "tag"),
        disk: string(&v["local_disk"], "state"),
        drive: string(&v["local_disk"], "drive"),
        version: string(installed, "version"),
        previous: string(installed, "previous"),
        carried: installed.get("carried").cloned(),
        install_config: string(v, "install_config"),
        id,
    }
}

fn asset_row(name: &str, v: &Value) -> AssetRow {
    let id = name.trim_end_matches(".json").to_string();
    AssetRow {
        at: string(v, "at"),
        boot_id: string(v, "boot_id").or_else(|| id.split_once('-').map(|(_, b)| b.to_string())),
        node: string(v, "node"),
        kernel: string(v, "kernel"),
        memory_total_bytes: v["memory_total_bytes"].as_u64(),
        items: v["items"].as_object().map_or(0, |o| o.len()),
        changes: v.get("changes").cloned().unwrap_or(Value::Null),
        changed_in_boot: v.get("changed_in_boot").cloned().unwrap_or(Value::Null),
        previous: string(v, "previous"),
        id,
    }
}

/// `<YYYY-MM>.jsonl` in a directory, newest first.
fn months(dir: &Path) -> Vec<String> {
    let mut m: Vec<String> = names(dir).into_iter().filter(|n| n.ends_with(".jsonl") && is_file(&dir.join(n))).collect();
    m.reverse();
    m
}

fn drive_row(dir: &Path, key: &str) -> DriveRow {
    let d = dir.join(key);
    let months = months(&d);
    let last = months.iter().find_map(|m| {
        let (text, _) = tail(&d.join(m), LAST_RECORD_MAX).ok()?;
        let newest = json_lines(&text).next_back();
        newest
    });
    let last = last.unwrap_or(Value::Null);
    DriveRow {
        key: key.to_string(),
        drive: last.get("drive").cloned().unwrap_or(Value::Null),
        last_at: string(&last, "at"),
        status: string(&last, "status"),
        temperature_c: last["temperature_c"].as_i64(),
        node: string(&last, "node"),
        findings: last.get("findings").cloned().unwrap_or_else(|| Value::Array(Vec::new())),
        summary: read_json(&d.join("summary.json")).unwrap_or(Value::Null),
        months: months.len(),
    }
}

/// The newest `limit` lines over a directory's months, newest first.
fn newest_lines(dir: &Path, limit: usize) -> (Vec<Value>, bool) {
    let mut out = Vec::new();
    let mut cut = false;
    for m in months(dir) {
        if out.len() >= limit {
            break;
        }
        let Ok((text, c)) = tail(&dir.join(&m), MONTH_MAX) else { continue };
        cut |= c;
        out.extend(json_lines(&text).rev().take(limit - out.len()));
    }
    (out, cut)
}
