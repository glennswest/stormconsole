//! Lenient RFC 5424 parsing for the stormcast dialect. The emitters are
//! trusted cooperators, not adversaries — anything that doesn't parse as
//! 5424 is kept whole as the message with the sender's address as host,
//! so a malformed line is never dropped.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    /// RFC 3339 as sent, or receive time when absent.
    pub ts: String,
    pub host: String,
    pub app: String,
    /// Syslog severity 0–7 (emergency..debug).
    pub severity: u8,
    pub facility: u8,
    pub msg: String,
}

pub fn parse(line: &str, src: &str, now: impl Fn() -> String) -> LogEvent {
    let line = line.trim_end_matches(['\r', '\n']);
    let (pri, rest) = take_pri(line);
    let (facility, severity) = match pri {
        Some(p) => (p >> 3, (p & 7) as u8),
        None => (1, 6),
    };
    let fallback = |msg: &str| LogEvent {
        ts: now(),
        host: src.to_string(),
        app: String::new(),
        severity,
        facility: facility as u8,
        msg: msg.to_string(),
    };

    // RFC 5424: VERSION SP TIMESTAMP SP HOSTNAME SP APP-NAME SP PROCID SP
    // MSGID SP STRUCTURED-DATA [SP MSG]
    let mut it = rest.splitn(7, ' ');
    let (Some(ver), Some(ts), Some(host), Some(app), Some(_procid), Some(_msgid), tail) = (
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
    ) else {
        return fallback(rest);
    };
    if ver != "1" {
        return fallback(rest);
    }
    let msg = skip_structured_data(tail.unwrap_or(""));
    LogEvent {
        ts: if ts == "-" { now() } else { ts.to_string() },
        host: if host == "-" { src.to_string() } else { host.to_string() },
        app: if app == "-" { String::new() } else { app.to_string() },
        severity,
        facility: facility as u8,
        msg: msg.to_string(),
    }
}

fn take_pri(line: &str) -> (Option<u16>, &str) {
    let Some(rest) = line.strip_prefix('<') else { return (None, line) };
    let Some(end) = rest.find('>') else { return (None, line) };
    match rest[..end].parse::<u16>() {
        Ok(p) if p <= 191 => (Some(p), &rest[end + 1..]),
        _ => (None, line),
    }
}

/// Structured data is `-` or one or more `[...]` blocks (with `\]` escapes);
/// the message is whatever follows.
fn skip_structured_data(tail: &str) -> &str {
    let tail = tail.trim_start();
    if let Some(rest) = tail.strip_prefix("- ") {
        return rest;
    }
    if tail == "-" {
        return "";
    }
    if !tail.starts_with('[') {
        return tail;
    }
    let b = tail.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] == b'[' {
        i += 1;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b']' => {
                    i += 1;
                    break;
                }
                _ => i += 1,
            }
        }
    }
    tail[i..].trim_start()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(_: ()) -> String {
        "2026-08-28T00:00:00Z".to_string()
    }

    #[test]
    fn full_5424_line_parses() {
        let e = parse(
            "<131>1 2026-08-28T12:00:00Z storm-a1 stormd 1 - - process web crashed",
            "192.168.8.20",
            || at(()),
        );
        assert_eq!(e.severity, 3);
        assert_eq!(e.facility, 16);
        assert_eq!(e.host, "storm-a1");
        assert_eq!(e.app, "stormd");
        assert_eq!(e.msg, "process web crashed");
        assert_eq!(e.ts, "2026-08-28T12:00:00Z");
    }

    #[test]
    fn structured_data_is_skipped() {
        let e = parse(
            "<134>1 - - beacon 1 - [storm@0 cores=\"8\"] hello",
            "10.0.0.9",
            || at(()),
        );
        assert_eq!(e.host, "10.0.0.9");
        assert_eq!(e.msg, "hello");
        assert_eq!(e.ts, "2026-08-28T00:00:00Z");
    }

    #[test]
    fn garbage_survives_as_message_from_source() {
        let e = parse("plain kernel text", "10.0.0.5", || at(()));
        assert_eq!(e.host, "10.0.0.5");
        assert_eq!(e.severity, 6);
        assert_eq!(e.msg, "plain kernel text");
    }
}
