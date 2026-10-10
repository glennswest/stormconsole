//! The fleet log ring, on redb.
//!
//! Three things shape this store, and all three come from watching a real
//! node misbehave: a single failing service can emit the *same* line
//! thousands of times a second, the ring must never grow without bound,
//! and it must never be the thing that fails.
//!
//! **Dedup on arrival.** An entry is keyed by what an operator would call
//! "the same message" — host, app, severity, text — and repeats bump a
//! `count` and a last-seen time instead of appending a row. A flood of one
//! line therefore costs one entry, not a million, and the viewer shows it
//! as `×N`. The suppressed total is kept as a lifetime counter so the
//! absorption is visible rather than silent.
//!
//! **Two bounds, both automatic.** Entries are dropped when they fall
//! outside the retention window (not seen for `retain`) or when the ring
//! exceeds `cap` distinct entries, whichever bites first. A background
//! task prunes on a timer, so a quiet fleet still expires old lines —
//! insert-driven pruning alone would leave them forever.
//!
//! **Aggregates are maintained, not computed.** redb has no `GROUP BY`,
//! and the components feed asks for per-host and per-severity counts every
//! few seconds; scanning the ring for that would be absurd. Insert and
//! prune keep the two summary tables in step instead.
//!
//! Ordering is receive order (`seq`), not the wire timestamp — emitters
//! disagree about clocks, and a repeat re-inserts at a fresh `seq` so a
//! chattering line surfaces in the tail. That also makes `seq` order and
//! `last_seen` order the same, which is what lets pruning stop at the
//! first live entry instead of scanning the whole ring.
//!
//! Durability is `Eventual`: this is a bounded ring of ephemeral fleet
//! chatter, not a ledger, and fsyncing every datagram would make the
//! collector the slowest thing on the node.
//!
//! **Bounded by the disk it is on, too** (#128). On a node the ring lives
//! on the console's own data volume, 64 MiB, and 200,000 entries did not
//! fit: the volume filled, and being kept, every boot started full. So
//! every [`ROOM_EVERY`] inserts and on every sweep the filesystem is asked
//! (`statvfs`) how much is free; under `keep_free` of it, the oldest entries
//! go in proportion to what is missing and the database is compacted —
//! redb reuses freed pages but only `compact` gives them back to the
//! filesystem. The same happens at open, for a volume a previous run left
//! full.
//!
//! **One I/O error is not the end of it** (#128). After a failed write
//! redb refuses every later transaction ("Previous I/O error … close and
//! re-open the database"), and a ring that never reopened stayed broken
//! for the rest of the run. Now an I/O error closes the handle and reopens
//! the file — redb repairs it — sheds half the ring and compacts; a file
//! that will not open at all is replaced by a new ring, because this is
//! chatter, not a ledger. The operation that hit the error is tried once
//! more. At most once every [`RECOVER_EVERY_MS`], said once per recovery.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use redb::{Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::parse::LogEvent;

/// Receive-order sequence → the stored entry, JSON encoded.
const EVENTS: TableDefinition<u64, &[u8]> = TableDefinition::new("events");
/// Dedup key → the `seq` of the entry it currently occupies.
const INDEX: TableDefinition<&str, u64> = TableDefinition::new("index");
/// Host → its rolling summary, JSON encoded.
const HOSTS: TableDefinition<&str, &[u8]> = TableDefinition::new("hosts");
/// Syslog severity → occurrences currently retained.
const SEVERITY: TableDefinition<u8, u64> = TableDefinition::new("severity");
/// Scalars: `next_seq`, `occurrences`, `suppressed`.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

/// Never re-announce the same repeating line to live followers more often
/// than this. Without it a flooding message is a flood on the wire, in the
/// browser, and in every other viewer too.
const NOTIFY_INTERVAL_MS: u64 = 1_000;

/// How often (in inserts) the filesystem is asked how much is free.
pub const ROOM_EVERY: u64 = 128;
/// The fraction of the filesystem kept free when nothing says otherwise.
pub const DEFAULT_KEEP_FREE: f64 = 0.20;
/// No more than one recovery in this long: a disk that stays full must not
/// turn every datagram into a reopen.
pub const RECOVER_EVERY_MS: u64 = 5_000;
/// After a shed that could not reach the floor, wait this long before the
/// next one, unless an open or a sweep asks.
const ROOM_EVERY_MS: u64 = 10_000;

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

macro_rules! from_err {
    ($($t:ty),* $(,)?) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                Error(e.to_string())
            }
        }
    )*};
}
from_err!(
    redb::Error,
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
    redb::CompactionError,
    serde_json::Error,
);

pub type Result<T> = std::result::Result<T, Error>;

/// One retained line. `count` is how many arrivals collapsed into it, so a
/// value above 1 is exactly the duplicate count the viewer renders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEvent {
    /// Wire timestamp of the most recent occurrence.
    pub ts: String,
    pub host: String,
    pub app: String,
    pub severity: u8,
    pub facility: u8,
    pub msg: String,
    /// Arrivals collapsed into this entry; 1 means it has never repeated.
    #[serde(default = "one")]
    pub count: u64,
    /// Wire timestamp of the first occurrence.
    #[serde(default)]
    pub first_ts: String,
    /// Receive time of the first occurrence, epoch milliseconds.
    #[serde(default)]
    pub first_seen: u64,
    /// Receive time of the most recent occurrence; the ring's ordering and
    /// what retention measures.
    #[serde(default)]
    pub last_seen: u64,
    /// Receive time this entry was last pushed to live followers. Bookkeeping
    /// for the repeat throttle, carried on the entry so it survives restart.
    #[serde(default)]
    pub last_notified: u64,
}

fn one() -> u64 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostSummary {
    pub host: String,
    /// The address its datagrams arrive from — what the fleet plugin
    /// dials to drill into this node. Empty for a host heard only before
    /// this was recorded; the last address seen wins, so a node that
    /// changes address corrects itself on its next line.
    #[serde(default)]
    pub addr: String,
    /// Occurrences currently retained, duplicates included — the same
    /// number this reported before dedup existed.
    pub count: i64,
    /// Distinct retained entries for this host.
    #[serde(default)]
    pub entries: i64,
    pub last_ts: String,
}

/// What an insert did, so the collector knows whether to wake followers.
pub struct Insert {
    pub event: StoredEvent,
    /// False when this was a repeat seen again within the throttle window.
    pub notify: bool,
}

/// Ring counters for the summary endpoint and the components feed.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Stats {
    /// Distinct entries retained.
    pub entries: u64,
    /// Occurrences retained, duplicates included.
    pub occurrences: u64,
    /// Duplicates collapsed over this store's lifetime.
    pub suppressed: u64,
}

/// A filesystem's size and what is free on it, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub total: u64,
    pub avail: u64,
}

/// How the ring has kept itself within its disk and recovered (#128), for
/// the collector's card.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Care {
    /// Reopens after an I/O error.
    pub recoveries: u64,
    /// Entries dropped to keep the filesystem's floor free.
    pub shed: u64,
    /// The last recovery's cause, and when (epoch ms).
    pub last_cause: String,
    pub last_at: u64,
    /// Why the ring is closed, while it is.
    pub broken: Option<String>,
    /// Free space on the ring's filesystem at the last look, in percent.
    pub free_percent: Option<f64>,
    #[serde(skip)]
    last_try: u64,
    #[serde(skip)]
    last_room: u64,
    /// The last shed could not reach the floor: something else is filling
    /// the disk, and shedding again at once would only empty the ring.
    #[serde(skip)]
    futile: bool,
}

type SpaceFn = Box<dyn Fn(&Path) -> Option<Space> + Send + Sync>;

pub struct Store {
    path: PathBuf,
    /// `None` only while a recovery has failed: the next operation tries again.
    db: RwLock<Option<Database>>,
    cap: u64,
    retain_ms: u64,
    dedup: bool,
    /// Insert counter, so a busy ring prunes without waiting for the timer.
    since_prune: Mutex<u64>,
    /// The fraction of the filesystem kept free.
    keep_free: f64,
    /// How free space is measured: `statvfs`, or a test's stand-in.
    space: SpaceFn,
    care: Mutex<Care>,
}

/// The filesystem a path is on, by `statvfs`.
// The field types differ by platform (u32 on some), so the casts are not
// all no-ops everywhere.
#[allow(clippy::unnecessary_cast)]
pub fn statvfs(dir: &Path) -> Option<Space> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: a zeroed statvfs is a valid out-parameter, and `c` is a
    // NUL-terminated path that lives across the call.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let frsize = if st.f_frsize > 0 { st.f_frsize as u64 } else { st.f_bsize as u64 };
    Some(Space { total: st.f_blocks as u64 * frsize, avail: st.f_bavail as u64 * frsize })
}

/// An error that leaves redb refusing everything until it is reopened, or
/// that says the disk is the problem.
pub fn is_io(e: &Error) -> bool {
    let m = e.0.as_str();
    m.contains("I/O error") || m.contains("No space left") || m.contains("os error")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// SQLite stamps every database with this header, so an old ring is
/// identifiable rather than merely unreadable.
fn is_sqlite(path: &str) -> bool {
    use std::io::Read;
    let mut header = [0u8; 16];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header == b"SQLite format 3\0"
}

fn dedup_key(host: &str, app: &str, severity: u8, msg: &str) -> String {
    // \x1f (unit separator) cannot appear in a parsed syslog field, so the
    // parts can never run together into a colliding key.
    format!("{host}\x1f{app}\x1f{severity}\x1f{}", fingerprint(msg))
}

/// What makes two arrivals "the same line".
///
/// Emitters on this fleet forward a process's own log line verbatim, and
/// tracing writes its timestamp at the front of that line — so the text
/// carries a microsecond clock that changes on every single occurrence:
///
/// ```text
/// 2026-09-02T18:26:30.258373Z  WARN plugin_logs::collector: store insert failed
/// ```
///
/// Keyed on the raw text, a message repeating a thousand times a second is
/// a thousand distinct entries and dedup does nothing at all — which is
/// exactly what happened the first time this ran against a real node. A
/// leading timestamp is redundant with the event's own `ts` and can never
/// be what distinguishes two messages, so it comes off before keying. The
/// stored text is untouched; only the key is normalised.
fn fingerprint(msg: &str) -> &str {
    let rest = strip_leading_timestamp(msg);
    // Never let normalisation collapse everything into one entry.
    if rest.is_empty() {
        msg
    } else {
        rest
    }
}

/// Strip a leading ISO-8601 / RFC 3339 timestamp and the whitespace after
/// it. Anything that is not one is returned unchanged.
fn strip_leading_timestamp(msg: &str) -> &str {
    let b = msg.as_bytes();
    let digits = |from: usize, n: usize| {
        b.len() >= from + n && b[from..from + n].iter().all(u8::is_ascii_digit)
    };
    // YYYY-MM-DD
    if !(digits(0, 4) && b.get(4) == Some(&b'-') && digits(5, 2) && b.get(7) == Some(&b'-')
        && digits(8, 2))
    {
        return msg;
    }
    // Date and time are joined by 'T' or a space.
    if !matches!(b.get(10), Some(b'T') | Some(b' ')) {
        return msg;
    }
    // HH:MM:SS
    if !(digits(11, 2) && b.get(13) == Some(&b':') && digits(14, 2) && b.get(16) == Some(&b':')
        && digits(17, 2))
    {
        return msg;
    }
    let mut i = 19;
    // Optional fractional seconds.
    if b.get(i) == Some(&b'.') {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    // Optional zone: Z, or ±HH:MM / ±HHMM.
    match b.get(i) {
        Some(b'Z') | Some(b'z') => i += 1,
        Some(b'+') | Some(b'-') => {
            let start = i;
            i += 1;
            if digits(i, 2) {
                i += 2;
                if b.get(i) == Some(&b':') {
                    i += 1;
                }
                if digits(i, 2) {
                    i += 2;
                } else {
                    i = start; // not a zone after all — leave it in place
                }
            } else {
                i = start;
            }
        }
        _ => {}
    }
    msg[i..].trim_start()
}

impl Store {
    /// With the default floor and `statvfs`; the plugin passes its own.
    #[cfg(test)]
    pub fn open(path: &str, cap: u64, retain_ms: u64, dedup: bool) -> Result<Self> {
        Self::open_with(path, cap, retain_ms, dedup, DEFAULT_KEEP_FREE, Box::new(statvfs))
    }

    /// `keep_free` is the fraction of the filesystem kept free; `space`
    /// measures it.
    pub fn open_with(path: &str, cap: u64, retain_ms: u64, dedup: bool, keep_free: f64, space: SpaceFn) -> Result<Self> {
        if let Some(dir) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let db = match Database::create(path) {
            Ok(db) => db,
            // The default path changed name with the format, so this only
            // happens when a config points at the old ring explicitly. redb
            // says "invalid data", which tells an operator nothing; say what
            // the file actually is and what to do about it.
            Err(e) if is_sqlite(path) => {
                return Err(Error(format!(
                    "{path} is the SQLite ring from stormconsole 0.6 and \
                     earlier, which nothing reads any more. Delete it, or \
                     point [logs] db_path at a new file such as \
                     logs.redb ({e})"
                )))
            }
            // A ring that will not open — corrupt, or a repair that needs
            // space a full volume does not have — is replaced: this is
            // chatter, and a console with no fleet log is worse off than
            // one that starts its ring over (#128).
            Err(e) => {
                warn!(path, error = %e, "the log ring will not open: starting a new one");
                let _ = std::fs::remove_file(path);
                Database::create(path)?
            }
        };
        create_tables(&db)?;
        let store = Self {
            path: PathBuf::from(path),
            db: RwLock::new(Some(db)),
            cap,
            retain_ms,
            dedup,
            since_prune: Mutex::new(0),
            keep_free: keep_free.clamp(0.0, 0.9),
            space,
            care: Mutex::new(Care::default()),
        };
        // A volume the last run left full is made room on before the
        // first datagram, not after it fails.
        if let Err(e) = store.make_room(true) {
            warn!(error = %e, "making room in the log ring at open failed");
        }
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_temp(cap: u64, retain_ms: u64, dedup: bool) -> Result<(Self, tempfile::TempDir)> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ring.redb");
        let store = Self::open(path.to_str().unwrap(), cap, retain_ms, dedup)?;
        Ok((store, dir))
    }

    /// Run `op` on the database; on an I/O error, recover and run it once
    /// more.
    fn with<T>(&self, op: impl Fn(&Database) -> Result<T>) -> Result<T> {
        let first = {
            let g = self.db.read().unwrap_or_else(|e| e.into_inner());
            match g.as_ref() {
                Some(db) => op(db),
                None => Err(Error(format!(
                    "the log ring is closed: {}",
                    self.care().broken.unwrap_or_else(|| "reopening".into())
                ))),
            }
        };
        match first {
            Err(e) if is_io(&e) || e.0.starts_with("the log ring is closed") => {
                if self.recover(&e.0) {
                    let g = self.db.read().unwrap_or_else(|e| e.into_inner());
                    if let Some(db) = g.as_ref() {
                        return op(db);
                    }
                }
                Err(e)
            }
            r => r,
        }
    }

    /// What the ring has done to stay within its disk, and its recoveries.
    pub fn care(&self) -> Care {
        self.care.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Close the handle redb has poisoned, reopen the file, shed half and
    /// compact. `false` when it was tried too recently or did not work.
    pub fn recover(&self, cause: &str) -> bool {
        let now = now_ms();
        {
            let mut c = self.care.lock().unwrap_or_else(|e| e.into_inner());
            if c.last_try > 0 && now.saturating_sub(c.last_try) < RECOVER_EVERY_MS {
                return false;
            }
            c.last_try = now;
        }
        let mut g = self.db.write().unwrap_or_else(|e| e.into_inner());
        // Dropping the database closes the file: the only way out of
        // redb's "Previous I/O error".
        drop(g.take());
        let path = self.path.to_string_lossy().into_owned();
        let opened = match Database::create(&self.path) {
            Ok(db) => Ok(db),
            Err(e) => {
                warn!(path, error = %e, "the log ring will not reopen: starting a new one");
                let _ = std::fs::remove_file(&self.path);
                Database::create(&self.path).map_err(Error::from)
            }
        };
        let mut db = match opened.and_then(|db| create_tables(&db).map(|_| db)) {
            Ok(db) => db,
            Err(e) => {
                let why = format!("{path}: {e} (after: {cause})");
                warn!(error = %why, "the log ring could not be reopened; trying again in a few seconds");
                self.care.lock().unwrap_or_else(|e| e.into_inner()).broken = Some(why);
                return false;
            }
        };
        let entries = count(&db).unwrap_or(0);
        let shed = self.prune_in(&db, now, Some(entries / 2)).unwrap_or(0);
        if let Err(e) = db.compact() {
            warn!(error = %e, "compacting the reopened log ring failed");
        }
        *g = Some(db);
        drop(g);
        let mut c = self.care.lock().unwrap_or_else(|e| e.into_inner());
        c.recoveries += 1;
        c.shed += shed;
        c.last_cause = cause.to_string();
        c.last_at = now;
        c.broken = None;
        warn!(cause, shed, recoveries = c.recoveries, "the log ring was reopened after an I/O error");
        true
    }

    /// Keep `keep_free` of the filesystem free: drop the oldest entries in
    /// proportion to what is missing, then compact so the file shrinks.
    /// Returns how many went. `force` skips the rate limit (open, a sweep).
    pub fn make_room(&self, force: bool) -> Result<u64> {
        let now = now_ms();
        let dir = self.path.parent().unwrap_or(Path::new("."));
        let Some(sp) = (self.space)(dir) else { return Ok(0) };
        {
            let mut c = self.care.lock().unwrap_or_else(|e| e.into_inner());
            c.free_percent = (sp.total > 0).then(|| sp.avail as f64 * 100.0 / sp.total as f64);
            let floor = (sp.total as f64 * self.keep_free) as u64;
            if sp.avail >= floor || (!force && c.futile && now.saturating_sub(c.last_room) < ROOM_EVERY_MS) {
                return Ok(0);
            }
            c.last_room = now;
        }
        let floor = (sp.total as f64 * self.keep_free) as u64;
        let need = floor.saturating_sub(sp.avail);
        let file = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0).max(1);
        // The share of the file that is missing, and a tenth more so this
        // does not run again at the next look. At most nine tenths: what
        // filled the disk may not be the ring at all.
        let share = (need as f64 / file as f64 + 0.1).min(0.9);
        let entries = self.with(count)?;
        let n = (entries as f64 * share).ceil() as u64;
        let shed = self.with(|db| self.prune_in(db, now, Some(n)))?;
        {
            let mut g = self.db.write().unwrap_or_else(|e| e.into_inner());
            if let Some(db) = g.as_mut() {
                db.compact().map_err(Error::from)?;
            }
        }
        let after = (self.space)(dir);
        let mut c = self.care.lock().unwrap_or_else(|e| e.into_inner());
        c.shed += shed;
        if let Some(a) = after {
            c.free_percent = (a.total > 0).then(|| a.avail as f64 * 100.0 / a.total as f64);
        }
        c.futile = after.is_some_and(|a| a.avail < floor);
        info!(
            shed,
            free_before = sp.avail,
            free_after = after.map(|a| a.avail),
            floor,
            "the log ring gave space back to keep its filesystem's floor free"
        );
        Ok(shed)
    }

    pub fn insert(&self, e: &LogEvent, now_ms: u64) -> Result<Insert> {
        let out = self.with(|db| self.insert_in(db, e, now_ms))?;
        let (room, due) = {
            let mut n = self.since_prune.lock().unwrap_or_else(|e| e.into_inner());
            *n += 1;
            let room = (*n).is_multiple_of(ROOM_EVERY);
            let due = *n >= 4096;
            if due {
                *n = 0;
            }
            (room, due)
        };
        if due {
            self.prune(now_ms)?;
        } else if room {
            self.make_room(false)?;
        }
        Ok(out)
    }

    fn insert_in(&self, db: &Database, e: &LogEvent, now_ms: u64) -> Result<Insert> {
        let key = dedup_key(&e.host, &e.app, e.severity, &e.msg);
        let mut tx = db.begin_write()?;
        tx.set_durability(Durability::Eventual);
        let out;
        {
            let mut events = tx.open_table(EVENTS)?;
            let mut index = tx.open_table(INDEX)?;
            let mut hosts = tx.open_table(HOSTS)?;
            let mut severity = tx.open_table(SEVERITY)?;
            let mut meta = tx.open_table(META)?;

            let seq = meta.get("next_seq")?.map(|v| v.value()).unwrap_or(1);

            // An existing entry only counts as a duplicate if it is really
            // still there; a stale index row is treated as a fresh line.
            let previous = if self.dedup {
                match index.get(key.as_str())?.map(|v| v.value()) {
                    Some(old) => {
                        let bytes = events.get(old)?.map(|v| v.value().to_vec());
                        match bytes {
                            Some(b) => {
                                events.remove(old)?;
                                Some(serde_json::from_slice::<StoredEvent>(&b)?)
                            }
                            None => None,
                        }
                    }
                    None => None,
                }
            } else {
                None
            };

            let repeat = previous.is_some();
            let mut record = match previous {
                Some(mut r) => {
                    r.count += 1;
                    r.ts = e.ts.clone();
                    r.last_seen = now_ms;
                    r
                }
                None => StoredEvent {
                    ts: e.ts.clone(),
                    host: e.host.clone(),
                    app: e.app.clone(),
                    severity: e.severity,
                    facility: e.facility,
                    msg: e.msg.clone(),
                    count: 1,
                    first_ts: e.ts.clone(),
                    first_seen: now_ms,
                    last_seen: now_ms,
                    last_notified: 0,
                },
            };

            let notify = !repeat
                || now_ms.saturating_sub(record.last_notified) >= NOTIFY_INTERVAL_MS;
            if notify {
                record.last_notified = now_ms;
            }

            events.insert(seq, serde_json::to_vec(&record)?.as_slice())?;
            index.insert(key.as_str(), seq)?;
            meta.insert("next_seq", seq + 1)?;

            // Aggregates: an occurrence always counts, a distinct entry only
            // when this line is new.
            bump(&mut meta, "occurrences", 1)?;
            if repeat {
                bump(&mut meta, "suppressed", 1)?;
            }
            let sev_now = severity.get(e.severity)?.map(|v| v.value()).unwrap_or(0);
            severity.insert(e.severity, sev_now + 1)?;

            let mut summary = read_host(&hosts, &e.host)?.unwrap_or_else(|| HostSummary {
                host: e.host.clone(),
                ..Default::default()
            });
            summary.count += 1;
            if !repeat {
                summary.entries += 1;
            }
            summary.last_ts = e.ts.clone();
            if !e.addr.is_empty() {
                summary.addr = e.addr.clone();
            }
            hosts.insert(e.host.as_str(), serde_json::to_vec(&summary)?.as_slice())?;

            out = Insert { event: record, notify };
        }
        tx.commit()?;
        Ok(out)
    }

    /// Drop entries that fall outside either bound. Returns how many went.
    ///
    /// Both `seq` and `last_seen` increase together, so walking from the
    /// oldest entry and stopping at the first one that is still wanted
    /// visits only what it removes.
    pub fn prune(&self, now_ms: u64) -> Result<u64> {
        let n = self.with(|db| self.prune_in(db, now_ms, None))?;
        Ok(n + self.make_room(true)?)
    }

    /// The bounds, plus `shed` more of the oldest when space is wanted.
    fn prune_in(&self, db: &Database, now_ms: u64, shed: Option<u64>) -> Result<u64> {
        let mut tx = db.begin_write()?;
        tx.set_durability(Durability::Eventual);
        let mut removed = 0u64;
        {
            let mut events = tx.open_table(EVENTS)?;
            let mut index = tx.open_table(INDEX)?;
            let mut hosts = tx.open_table(HOSTS)?;
            let mut severity = tx.open_table(SEVERITY)?;
            let mut meta = tx.open_table(META)?;

            let cutoff = now_ms.saturating_sub(self.retain_ms);
            let mut over = events.len()?.saturating_sub(self.cap).max(shed.unwrap_or(0));

            let mut victims: Vec<(u64, StoredEvent)> = Vec::new();
            for item in events.iter()? {
                let (k, v) = item?;
                let record: StoredEvent = serde_json::from_slice(v.value())?;
                let too_many = over > 0;
                let too_old = self.retain_ms > 0 && record.last_seen < cutoff;
                if !too_many && !too_old {
                    break;
                }
                if too_many {
                    over -= 1;
                }
                victims.push((k.value(), record));
            }

            for (seq, record) in victims {
                events.remove(seq)?;
                let key =
                    dedup_key(&record.host, &record.app, record.severity, &record.msg);
                // Only clear the index if it still points here — a repeat may
                // already have moved this key to a newer seq.
                if index.get(key.as_str())?.map(|v| v.value()) == Some(seq) {
                    index.remove(key.as_str())?;
                }

                drop_from(&mut meta, "occurrences", record.count)?;
                let sev_now = severity.get(record.severity)?.map(|v| v.value()).unwrap_or(0);
                severity.insert(record.severity, sev_now.saturating_sub(record.count))?;

                if let Some(mut summary) = read_host(&hosts, &record.host)? {
                    summary.count = (summary.count - record.count as i64).max(0);
                    summary.entries = (summary.entries - 1).max(0);
                    if summary.entries == 0 {
                        hosts.remove(record.host.as_str())?;
                    } else {
                        hosts.insert(
                            record.host.as_str(),
                            serde_json::to_vec(&summary)?.as_slice(),
                        )?;
                    }
                }
                removed += 1;
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    pub fn stats(&self) -> Stats {
        self.with(|db| {
            let tx = db.begin_read()?;
            let events = tx.open_table(EVENTS)?;
            let meta = tx.open_table(META)?;
            Ok(Stats {
                entries: events.len()?,
                occurrences: meta.get("occurrences")?.map(|v| v.value()).unwrap_or(0),
                suppressed: meta.get("suppressed")?.map(|v| v.value()).unwrap_or(0),
            })
        })
        .unwrap_or_default()
    }

    /// Most-recent entries matching the filters, returned oldest-first.
    /// `app` is an exact match on the emitter — the container or service
    /// name, which is what "show me this container's logs" means.
    ///
    /// Exact, not a substring of the message, because `search` already does
    /// substring and the two answer different questions: searching for
    /// `cilium` finds every line that *mentions* cilium, from any emitter,
    /// which is the wrong answer when what you want is what one crashing
    /// container said. That distinction is why this is its own parameter
    /// rather than a preset search.
    pub fn query(
        &self,
        host: Option<&str>,
        app: Option<&str>,
        min_severity: Option<u8>,
        search: Option<&str>,
        last: i64,
    ) -> Result<Vec<StoredEvent>> {
        let want = last.max(0) as usize;
        let needle = search.map(|s| s.to_lowercase());
        self.with(|db| self.query_in(db, host, app, min_severity, needle.as_deref(), want))
    }

    fn query_in(
        &self,
        db: &Database,
        host: Option<&str>,
        app: Option<&str>,
        min_severity: Option<u8>,
        needle: Option<&str>,
        want: usize,
    ) -> Result<Vec<StoredEvent>> {
        let tx = db.begin_read()?;
        let events = tx.open_table(EVENTS)?;

        let mut rows: Vec<StoredEvent> = Vec::with_capacity(want.min(1024));
        for item in events.iter()?.rev() {
            if rows.len() >= want {
                break;
            }
            let (_, v) = item?;
            let record: StoredEvent = serde_json::from_slice(v.value())?;
            if let Some(h) = host {
                if record.host != h {
                    continue;
                }
            }
            if let Some(a) = app {
                if record.app != a {
                    continue;
                }
            }
            // Syslog severity counts down toward emergency: "at least
            // warning" means severity <= 4.
            if let Some(s) = min_severity {
                if record.severity > s {
                    continue;
                }
            }
            if let Some(q) = needle {
                if !record.msg.to_lowercase().contains(q)
                    && !record.app.to_lowercase().contains(q)
                {
                    continue;
                }
            }
            rows.push(record);
        }
        rows.reverse();
        Ok(rows)
    }

    /// Every emitter the ring has heard from, so a container can be picked
    /// rather than typed.
    ///
    /// Scanned rather than kept in a table like hosts: an app list is read
    /// when somebody opens a filter, not on every insert, and a second index
    /// maintained on the hot path to answer a question asked twice an hour is
    /// the wrong trade. Bounded by the ring, which is bounded.
    pub fn apps(&self) -> Result<Vec<String>> {
        self.with(|db| {
        let tx = db.begin_read()?;
        let events = tx.open_table(EVENTS)?;
        let mut seen = std::collections::BTreeSet::new();
        for item in events.iter()? {
            let (_, v) = item?;
            let record: StoredEvent = serde_json::from_slice(v.value())?;
            if !record.app.is_empty() {
                seen.insert(record.app);
            }
        }
        Ok(seen.into_iter().collect())
        })
    }

    pub fn hosts(&self) -> Result<Vec<HostSummary>> {
        self.with(|db| {
        let tx = db.begin_read()?;
        let hosts = tx.open_table(HOSTS)?;
        let mut out = Vec::new();
        for item in hosts.iter()? {
            let (_, v) = item?;
            out.push(serde_json::from_slice::<HostSummary>(v.value())?);
        }
        out.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(out)
        })
    }

    pub fn severity_counts(&self) -> Result<Vec<(u8, i64)>> {
        self.with(|db| {
        let tx = db.begin_read()?;
        let severity = tx.open_table(SEVERITY)?;
        let mut out = Vec::new();
        for item in severity.iter()? {
            let (k, v) = item?;
            let n = v.value();
            if n > 0 {
                out.push((k.value(), n as i64));
            }
        }
        Ok(out)
        })
    }
}

/// Every table is created up front so read transactions never have to
/// cope with one that does not exist yet.
fn create_tables(db: &Database) -> Result<()> {
    let tx = db.begin_write()?;
    {
        tx.open_table(EVENTS)?;
        tx.open_table(INDEX)?;
        tx.open_table(HOSTS)?;
        tx.open_table(SEVERITY)?;
        tx.open_table(META)?;
    }
    tx.commit()?;
    Ok(())
}

/// Distinct entries in the ring.
fn count(db: &Database) -> Result<u64> {
    let tx = db.begin_read()?;
    let events = tx.open_table(EVENTS)?;
    Ok(events.len()?)
}

fn bump(meta: &mut redb::Table<&str, u64>, key: &str, by: u64) -> Result<()> {
    let now = meta.get(key)?.map(|v| v.value()).unwrap_or(0);
    meta.insert(key, now + by)?;
    Ok(())
}

fn drop_from(meta: &mut redb::Table<&str, u64>, key: &str, by: u64) -> Result<()> {
    let now = meta.get(key)?.map(|v| v.value()).unwrap_or(0);
    meta.insert(key, now.saturating_sub(by))?;
    Ok(())
}

fn read_host(
    hosts: &redb::Table<&str, &[u8]>,
    host: &str,
) -> Result<Option<HostSummary>> {
    match hosts.get(host)? {
        Some(v) => Ok(Some(serde_json::from_slice(v.value())?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000;

    fn ev(host: &str, severity: u8, msg: &str) -> LogEvent {
        LogEvent {
            ts: "2026-08-28T00:00:00Z".into(),
            host: host.into(),
            addr: format!("192.168.8.{}", 100 + (host.len() as u8)),
            app: "test".into(),
            severity,
            facility: 16,
            msg: msg.into(),
        }
    }

    #[test]
    fn a_host_summary_carries_the_address_to_dial_it_on() {
        let (s, _d) = Store::open_temp(1000, HOUR, true).unwrap();
        s.insert(&ev("storm-1", 6, "up"), 1).unwrap();
        let hosts = s.hosts().unwrap();
        let h = hosts.iter().find(|h| h.host == "storm-1").unwrap();
        assert_eq!(h.addr, "192.168.8.107", "the address the datagram came from");

        // A node that moves corrects itself on its next line.
        let mut moved = ev("storm-1", 6, "up again");
        moved.addr = "192.168.8.200".into();
        s.insert(&moved, 2).unwrap();
        let hosts = s.hosts().unwrap();
        assert_eq!(hosts.iter().find(|h| h.host == "storm-1").unwrap().addr, "192.168.8.200");
    }

    #[test]
    fn a_leading_timestamp_never_makes_a_line_distinct() {
        // The line that defeated the first version of this, verbatim off
        // the wire from a node running stormconsole 0.6.
        let a = "2026-09-02T18:26:30.258373Z  WARN plugin_logs::collector: \
                 store insert failed error=database or disk is full";
        let b = "2026-09-02T18:26:44.545181Z  WARN plugin_logs::collector: \
                 store insert failed error=database or disk is full";
        assert_eq!(fingerprint(a), fingerprint(b));
        assert!(fingerprint(a).starts_with("WARN"));

        // Every shape a fleet emitter is likely to put in front.
        for stamp in [
            "2026-09-02T18:26:30Z ",
            "2026-09-02T18:26:30.1Z ",
            "2026-09-02 18:26:30 ",
            "2026-09-02T18:26:30+01:00 ",
            "2026-09-02T18:26:30.123456-0500 ",
        ] {
            assert_eq!(fingerprint(&format!("{stamp}the same thing")), "the same thing", "{stamp}");
        }
    }

    #[test]
    fn normalisation_leaves_ordinary_messages_alone() {
        assert_eq!(fingerprint("disk is full"), "disk is full");
        // A near-miss must not be eaten.
        assert_eq!(fingerprint("2026-09-02 is the date"), "2026-09-02 is the date");
        assert_eq!(fingerprint("20260902T182630Z packed"), "20260902T182630Z packed");
        // A bare timestamp is all the message there is; keep it, or every
        // such line would collapse into one entry.
        assert_eq!(fingerprint("2026-09-02T18:26:30Z"), "2026-09-02T18:26:30Z");
    }

    #[test]
    fn the_flood_collapses_even_though_every_line_differs() {
        let (s, _d) = Store::open_temp(1000, HOUR, true).unwrap();
        for i in 0..1000 {
            let msg = format!(
                "2026-09-02T18:26:{:02}.{:06}Z  WARN collector: store insert failed",
                i % 60,
                i
            );
            s.insert(&ev("a", 4, &msg), 1000 + i as u64).unwrap();
        }
        let stats = s.stats();
        assert_eq!(stats.entries, 1, "one entry for one repeating line");
        assert_eq!(stats.suppressed, 999);
        // The row shows the most recent text, not a normalised one.
        let row = &s.query(None, None, None, None, 10).unwrap()[0];
        assert_eq!(row.count, 1000);
        assert!(row.msg.starts_with("2026-09-02T18:26:"));
    }

    #[test]
    fn an_old_sqlite_ring_is_named_not_just_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs.db");
        std::fs::write(&path, b"SQLite format 3\0and then some payload").unwrap();
        // `unwrap_err` would need Store: Debug, which a handle to a
        // database has no business deriving.
        let msg = match Store::open(path.to_str().unwrap(), 10, HOUR, true) {
            Ok(_) => panic!("opened a SQLite file as a redb ring"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("SQLite ring"), "{msg}");
        assert!(msg.contains("Delete it"), "{msg}");
    }

    #[test]
    fn query_filters_and_orders() {
        let (s, _d) = Store::open_temp(1000, HOUR, true).unwrap();
        s.insert(&ev("a", 6, "one"), 1).unwrap();
        s.insert(&ev("b", 3, "two"), 2).unwrap();
        s.insert(&ev("a", 4, "three"), 3).unwrap();

        let all = s.query(None, None, None, None, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].msg, "one"); // oldest first

        let errors = s.query(None, None, Some(4), None, 10).unwrap();
        assert_eq!(errors.len(), 2); // severity <= 4

        let a = s.query(Some("a"), None, None, Some("thr"), 10).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].msg, "three");
    }

    #[test]
    fn repeats_collapse_into_one_entry() {
        let (s, _d) = Store::open_temp(1000, HOUR, true).unwrap();
        for i in 0..500 {
            s.insert(&ev("a", 4, "disk is full"), 1000 + i).unwrap();
        }
        s.insert(&ev("a", 6, "something else"), 2000).unwrap();

        let stats = s.stats();
        assert_eq!(stats.entries, 2, "one entry per distinct line");
        assert_eq!(stats.occurrences, 501);
        assert_eq!(stats.suppressed, 499);

        let rows = s.query(None, None, None, None, 10).unwrap();
        let flood = rows.iter().find(|r| r.msg == "disk is full").unwrap();
        assert_eq!(flood.count, 500);
        assert_eq!(flood.first_seen, 1000);
        assert_eq!(flood.last_seen, 1499);

        let hosts = s.hosts().unwrap();
        assert_eq!(hosts[0].count, 501, "occurrences, as before dedup");
        assert_eq!(hosts[0].entries, 2);
    }

    #[test]
    fn repeats_are_throttled_on_the_live_tail() {
        let (s, _d) = Store::open_temp(1000, HOUR, true).unwrap();
        assert!(s.insert(&ev("a", 4, "again"), 0).unwrap().notify, "first always");
        assert!(!s.insert(&ev("a", 4, "again"), 100).unwrap().notify);
        assert!(!s.insert(&ev("a", 4, "again"), 999).unwrap().notify);
        assert!(s.insert(&ev("a", 4, "again"), 1000).unwrap().notify, "window passed");
    }

    #[test]
    fn dedup_can_be_turned_off() {
        let (s, _d) = Store::open_temp(1000, HOUR, false).unwrap();
        for i in 0..10 {
            s.insert(&ev("a", 4, "same"), i).unwrap();
        }
        assert_eq!(s.stats().entries, 10);
        assert_eq!(s.stats().suppressed, 0);
    }

    /// A store whose filesystem reports `avail` of 1000 units free.
    fn on_disk(avail: std::sync::Arc<std::sync::atomic::AtomicU64>) -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ring.redb");
        let space: SpaceFn = Box::new(move |_| {
            Some(Space { total: 1000, avail: avail.load(std::sync::atomic::Ordering::SeqCst) })
        });
        let store = Store::open_with(path.to_str().unwrap(), 1_000_000, 0, true, 0.2, space).unwrap();
        (store, dir)
    }

    fn fill(store: &Store, n: usize) {
        for i in 0..n {
            store.insert(&ev("h", 6, &format!("line {i} {}", "x".repeat(200))), 1).unwrap();
        }
    }

    #[test]
    fn under_the_floor_the_oldest_go_and_the_file_shrinks() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let avail = std::sync::Arc::new(AtomicU64::new(900));
        let (store, dir) = on_disk(avail.clone());
        fill(&store, 2000);
        let size = |d: &tempfile::TempDir| std::fs::metadata(d.path().join("ring.redb")).unwrap().len();
        let before = size(&dir);
        // Plenty free: nothing goes.
        assert_eq!(store.make_room(true).unwrap(), 0);
        assert_eq!(store.stats().entries, 2000);
        assert_eq!(store.care().free_percent, Some(90.0));
        // 15% free against a 20% floor: some of the oldest go, and only them.
        avail.store(150, Ordering::SeqCst);
        let shed = store.make_room(true).unwrap();
        assert!(shed > 0 && shed < 2000, "{shed}");
        assert_eq!(store.stats().entries, 2000 - shed);
        let rows = store.query(None, None, None, None, 1).unwrap();
        assert!(rows[0].msg.starts_with("line 1999 "), "the newest stays");
        assert!(size(&dir) < before, "compacted: {} → {}", before, size(&dir));
        assert_eq!(store.care().shed, shed);
        // Without force, not again at once.
        assert_eq!(store.make_room(false).unwrap(), 0);
    }

    #[test]
    fn a_closed_handle_is_reopened_and_the_insert_goes_through() {
        use std::sync::atomic::AtomicU64;
        let (store, _dir) = on_disk(std::sync::Arc::new(AtomicU64::new(900)));
        fill(&store, 100);
        // What a poisoned handle comes to once dropped: nothing to use.
        *store.db.write().unwrap() = None;
        store.insert(&ev("h", 3, "after the error"), 2).unwrap();
        let care = store.care();
        assert_eq!(care.recoveries, 1);
        assert!(care.broken.is_none());
        assert!(care.last_cause.contains("closed"), "{}", care.last_cause);
        // Half shed on reopen, the new line kept.
        let hits = store.query(None, None, Some(3), None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(store.stats().entries, 51);
        // Not twice within the window.
        assert!(!store.recover("again"));
    }

    #[test]
    fn a_ring_that_will_not_open_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ring.redb");
        std::fs::write(&path, vec![0x5a; 8192]).unwrap();
        let store = Store::open(path.to_str().unwrap(), 10, 0, true).unwrap();
        store.insert(&ev("h", 6, "fresh"), 1).unwrap();
        assert_eq!(store.stats().entries, 1);
    }

    #[test]
    fn io_errors_are_told_from_the_rest() {
        assert!(is_io(&Error("I/O error: No space left on device (os error 28)".into())));
        assert!(is_io(&Error("Previous I/O error occurred. Please close and re-open the database.".into())));
        assert!(!is_io(&Error("expected value at line 1 column 1".into())));
    }

    #[test]
    fn statvfs_reads_a_real_filesystem() {
        let s = statvfs(Path::new(".")).unwrap();
        assert!(s.total > 0 && s.avail <= s.total);
    }

    #[test]
    fn ring_prunes_to_cap() {
        let (s, _d) = Store::open_temp(10, HOUR, true).unwrap();
        for i in 0..64 {
            s.insert(&ev("a", 6, &format!("m{i}")), 1000 + i).unwrap();
        }
        s.prune(2000).unwrap();
        assert_eq!(s.stats().entries, 10);
        // The survivors are the newest ten.
        let rows = s.query(None, None, None, None, 100).unwrap();
        assert_eq!(rows.first().unwrap().msg, "m54");
        assert_eq!(rows.last().unwrap().msg, "m63");
    }

    #[test]
    fn retention_drops_what_has_not_been_seen() {
        let (s, _d) = Store::open_temp(1_000_000, HOUR, true).unwrap();
        s.insert(&ev("a", 6, "old"), 0).unwrap();
        s.insert(&ev("a", 6, "recent"), 3 * HOUR).unwrap();

        s.prune(3 * HOUR + 1).unwrap();
        let rows = s.query(None, None, None, None, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].msg, "recent");
        // The aggregates followed the eviction.
        assert_eq!(s.stats().occurrences, 1);
        assert_eq!(s.hosts().unwrap()[0].count, 1);
    }

    #[test]
    fn a_repeat_keeps_an_entry_alive() {
        let (s, _d) = Store::open_temp(1_000_000, HOUR, true).unwrap();
        s.insert(&ev("a", 6, "heartbeat"), 0).unwrap();
        // Seen again well after it would otherwise have expired.
        s.insert(&ev("a", 6, "heartbeat"), 5 * HOUR).unwrap();
        s.prune(5 * HOUR + 1).unwrap();
        assert_eq!(s.stats().entries, 1);
        assert_eq!(s.query(None, None, None, None, 10).unwrap()[0].count, 2);
    }

    #[test]
    fn severity_counts_follow_eviction() {
        let (s, _d) = Store::open_temp(2, HOUR, true).unwrap();
        s.insert(&ev("a", 3, "e1"), 1).unwrap();
        s.insert(&ev("a", 3, "e2"), 2).unwrap();
        s.insert(&ev("a", 6, "i1"), 3).unwrap();
        s.prune(4).unwrap();
        let counts: std::collections::HashMap<u8, i64> =
            s.severity_counts().unwrap().into_iter().collect();
        assert_eq!(counts.get(&3).copied().unwrap_or(0), 1);
        assert_eq!(counts.get(&6).copied().unwrap_or(0), 1);
    }

    fn from_app(app: &str, msg: &str) -> LogEvent {
        LogEvent { app: app.into(), ..ev("storm-1", 6, msg) }
    }

    #[test]
    fn app_filters_to_one_emitter() {
        // The distinction that makes this its own parameter: a search for
        // "vmimages" finds every line that *mentions* it, from any emitter.
        // Asking for one container's logs is a different question, and it is
        // the one somebody asks when that container is crashing.
        let (s, _d) = Store::open_temp(1000, HOUR, false).unwrap();
        for (app, msg) in [
            ("vmimages", "failed to load certificate authority"),
            ("kubelet", "starting vmimages container"),
            ("vmimages", "process exited"),
        ] {
            s.insert(&from_app(app, msg), 1).unwrap();
        }
        let mine = s.query(None, Some("vmimages"), None, None, 10).unwrap();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|r| r.app == "vmimages"));

        // The same needle as a search also catches the kubelet's line.
        let searched = s.query(None, None, None, Some("vmimages"), 10).unwrap();
        assert_eq!(searched.len(), 3);
    }

    #[test]
    fn app_is_exact_not_a_prefix() {
        // "cilium" must not return "cilium-operator": two containers, and
        // one of them being fine says nothing about the other.
        let (s, _d) = Store::open_temp(1000, HOUR, false).unwrap();
        s.insert(&from_app("cilium", "up"), 1).unwrap();
        s.insert(&from_app("cilium-operator", "up"), 1).unwrap();
        assert_eq!(s.query(None, Some("cilium"), None, None, 10).unwrap().len(), 1);
    }
}
