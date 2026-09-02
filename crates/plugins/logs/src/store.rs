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

use std::path::Path;
use std::sync::Mutex;

use redb::{Database, Durability, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};

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

pub struct Store {
    db: Database,
    cap: u64,
    retain_ms: u64,
    dedup: bool,
    /// Insert counter, so a busy ring prunes without waiting for the timer.
    since_prune: Mutex<u64>,
}

fn dedup_key(host: &str, app: &str, severity: u8, msg: &str) -> String {
    // \x1f (unit separator) cannot appear in a parsed syslog field, so the
    // parts can never run together into a colliding key.
    format!("{host}\x1f{app}\x1f{severity}\x1f{msg}")
}

impl Store {
    pub fn open(path: &str, cap: u64, retain_ms: u64, dedup: bool) -> Result<Self> {
        if let Some(dir) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let db = Database::create(path)?;
        let store = Self { db, cap, retain_ms, dedup, since_prune: Mutex::new(0) };
        store.create_tables()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_temp(cap: u64, retain_ms: u64, dedup: bool) -> Result<(Self, tempfile::TempDir)> {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ring.redb");
        let store = Self::open(path.to_str().unwrap(), cap, retain_ms, dedup)?;
        Ok((store, dir))
    }

    /// Every table is created up front so read transactions never have to
    /// cope with one that does not exist yet.
    fn create_tables(&self) -> Result<()> {
        let tx = self.db.begin_write()?;
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

    pub fn insert(&self, e: &LogEvent, now_ms: u64) -> Result<Insert> {
        let key = dedup_key(&e.host, &e.app, e.severity, &e.msg);
        let mut tx = self.db.begin_write()?;
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
            hosts.insert(e.host.as_str(), serde_json::to_vec(&summary)?.as_slice())?;

            out = Insert { event: record, notify };
        }
        tx.commit()?;

        let mut n = self.since_prune.lock().unwrap();
        *n += 1;
        let due = *n >= 4096;
        if due {
            *n = 0;
        }
        drop(n);
        if due {
            self.prune(now_ms)?;
        }
        Ok(out)
    }

    /// Drop entries that fall outside either bound. Returns how many went.
    ///
    /// Both `seq` and `last_seen` increase together, so walking from the
    /// oldest entry and stopping at the first one that is still wanted
    /// visits only what it removes.
    pub fn prune(&self, now_ms: u64) -> Result<u64> {
        let mut tx = self.db.begin_write()?;
        tx.set_durability(Durability::Eventual);
        let mut removed = 0u64;
        {
            let mut events = tx.open_table(EVENTS)?;
            let mut index = tx.open_table(INDEX)?;
            let mut hosts = tx.open_table(HOSTS)?;
            let mut severity = tx.open_table(SEVERITY)?;
            let mut meta = tx.open_table(META)?;

            let cutoff = now_ms.saturating_sub(self.retain_ms);
            let mut over = events.len()?.saturating_sub(self.cap);

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
        let read = || -> Result<Stats> {
            let tx = self.db.begin_read()?;
            let events = tx.open_table(EVENTS)?;
            let meta = tx.open_table(META)?;
            Ok(Stats {
                entries: events.len()?,
                occurrences: meta.get("occurrences")?.map(|v| v.value()).unwrap_or(0),
                suppressed: meta.get("suppressed")?.map(|v| v.value()).unwrap_or(0),
            })
        };
        read().unwrap_or_default()
    }

    /// Most-recent entries matching the filters, returned oldest-first.
    pub fn query(
        &self,
        host: Option<&str>,
        min_severity: Option<u8>,
        search: Option<&str>,
        last: i64,
    ) -> Result<Vec<StoredEvent>> {
        let want = last.max(0) as usize;
        let needle = search.map(|s| s.to_lowercase());
        let tx = self.db.begin_read()?;
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
            // Syslog severity counts down toward emergency: "at least
            // warning" means severity <= 4.
            if let Some(s) = min_severity {
                if record.severity > s {
                    continue;
                }
            }
            if let Some(q) = &needle {
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

    pub fn hosts(&self) -> Result<Vec<HostSummary>> {
        let tx = self.db.begin_read()?;
        let hosts = tx.open_table(HOSTS)?;
        let mut out = Vec::new();
        for item in hosts.iter()? {
            let (_, v) = item?;
            out.push(serde_json::from_slice::<HostSummary>(v.value())?);
        }
        out.sort_by(|a, b| a.host.cmp(&b.host));
        Ok(out)
    }

    pub fn severity_counts(&self) -> Result<Vec<(u8, i64)>> {
        let tx = self.db.begin_read()?;
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
    }
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
            app: "test".into(),
            severity,
            facility: 16,
            msg: msg.into(),
        }
    }

    #[test]
    fn query_filters_and_orders() {
        let (s, _d) = Store::open_temp(1000, HOUR, true).unwrap();
        s.insert(&ev("a", 6, "one"), 1).unwrap();
        s.insert(&ev("b", 3, "two"), 2).unwrap();
        s.insert(&ev("a", 4, "three"), 3).unwrap();

        let all = s.query(None, None, None, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].msg, "one"); // oldest first

        let errors = s.query(None, Some(4), None, 10).unwrap();
        assert_eq!(errors.len(), 2); // severity <= 4

        let a = s.query(Some("a"), None, Some("thr"), 10).unwrap();
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

        let rows = s.query(None, None, None, 10).unwrap();
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

    #[test]
    fn ring_prunes_to_cap() {
        let (s, _d) = Store::open_temp(10, HOUR, true).unwrap();
        for i in 0..64 {
            s.insert(&ev("a", 6, &format!("m{i}")), 1000 + i).unwrap();
        }
        s.prune(2000).unwrap();
        assert_eq!(s.stats().entries, 10);
        // The survivors are the newest ten.
        let rows = s.query(None, None, None, 100).unwrap();
        assert_eq!(rows.first().unwrap().msg, "m54");
        assert_eq!(rows.last().unwrap().msg, "m63");
    }

    #[test]
    fn retention_drops_what_has_not_been_seen() {
        let (s, _d) = Store::open_temp(1_000_000, HOUR, true).unwrap();
        s.insert(&ev("a", 6, "old"), 0).unwrap();
        s.insert(&ev("a", 6, "recent"), 3 * HOUR).unwrap();

        s.prune(3 * HOUR + 1).unwrap();
        let rows = s.query(None, None, None, 10).unwrap();
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
        assert_eq!(s.query(None, None, None, 10).unwrap()[0].count, 2);
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
}
