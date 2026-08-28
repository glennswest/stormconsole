//! The SQLite ring: bounded by row count, WAL, one shared connection with
//! short lock holds. Fleet log volume is modest; per-event inserts with a
//! periodic prune keep this simple and crash-safe — the ring exists so a
//! chatty fleet can never fill the volume.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

use crate::parse::LogEvent;

pub struct Store {
    conn: Mutex<Connection>,
    cap: i64,
    inserts: Mutex<u64>,
}

#[derive(Debug, serde::Serialize)]
pub struct HostSummary {
    pub host: String,
    pub count: i64,
    pub last_ts: String,
}

impl Store {
    pub fn open(path: &str, cap: i64) -> rusqlite::Result<Self> {
        if let Some(dir) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL,
                host TEXT NOT NULL,
                app TEXT NOT NULL,
                severity INTEGER NOT NULL,
                facility INTEGER NOT NULL,
                msg TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_events_host ON events(host);
            CREATE INDEX IF NOT EXISTS idx_events_severity ON events(severity);",
        )?;
        Ok(Self { conn: Mutex::new(conn), cap, inserts: Mutex::new(0) })
    }

    #[cfg(test)]
    pub fn open_in_memory(cap: i64) -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts TEXT NOT NULL, host TEXT NOT NULL, app TEXT NOT NULL,
                severity INTEGER NOT NULL, facility INTEGER NOT NULL,
                msg TEXT NOT NULL);",
        )?;
        Ok(Self { conn: Mutex::new(conn), cap, inserts: Mutex::new(0) })
    }

    pub fn insert(&self, e: &LogEvent) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events (ts, host, app, severity, facility, msg)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (&e.ts, &e.host, &e.app, e.severity, e.facility, &e.msg),
        )?;
        drop(conn);
        let mut n = self.inserts.lock().unwrap();
        *n += 1;
        if *n % 1024 == 0 {
            self.prune()?;
        }
        Ok(())
    }

    fn prune(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM events
             WHERE id <= (SELECT COALESCE(MAX(id), 0) FROM events) - ?1",
            [self.cap],
        )?;
        Ok(())
    }

    pub fn count(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0)).unwrap_or(0)
    }

    /// Most-recent events matching the filters, returned oldest-first.
    pub fn query(
        &self,
        host: Option<&str>,
        min_severity: Option<u8>,
        search: Option<&str>,
        last: i64,
    ) -> rusqlite::Result<Vec<LogEvent>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = String::from(
            "SELECT ts, host, app, severity, facility, msg FROM events WHERE 1=1",
        );
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(h) = host {
            sql.push_str(" AND host = ?");
            params.push(Box::new(h.to_string()));
        }
        if let Some(s) = min_severity {
            // Syslog severity counts down toward emergency: "at least warning"
            // means severity <= 4.
            sql.push_str(" AND severity <= ?");
            params.push(Box::new(s));
        }
        if let Some(q) = search {
            sql.push_str(" AND (msg LIKE ? OR app LIKE ?)");
            let pat = format!("%{q}%");
            params.push(Box::new(pat.clone()));
            params.push(Box::new(pat));
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        params.push(Box::new(last));

        let mut stmt = conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let mut rows: Vec<LogEvent> = stmt
            .query_map(refs.as_slice(), |r| {
                Ok(LogEvent {
                    ts: r.get(0)?,
                    host: r.get(1)?,
                    app: r.get(2)?,
                    severity: r.get(3)?,
                    facility: r.get(4)?,
                    msg: r.get(5)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    pub fn hosts(&self) -> rusqlite::Result<Vec<HostSummary>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT host, COUNT(*), MAX(ts) FROM events GROUP BY host ORDER BY host",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(HostSummary { host: r.get(0)?, count: r.get(1)?, last_ts: r.get(2)? })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    pub fn severity_counts(&self) -> rusqlite::Result<Vec<(u8, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT severity, COUNT(*) FROM events GROUP BY severity")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let s = Store::open_in_memory(1000).unwrap();
        s.insert(&ev("a", 6, "one")).unwrap();
        s.insert(&ev("b", 3, "two")).unwrap();
        s.insert(&ev("a", 4, "three")).unwrap();

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
    fn ring_prunes_to_cap() {
        let s = Store::open_in_memory(10).unwrap();
        for i in 0..2048 {
            s.insert(&ev("a", 6, &format!("m{i}"))).unwrap();
        }
        assert!(s.count() <= 1024 + 10);
        s.prune().unwrap();
        assert!(s.count() <= 10);
    }
}
