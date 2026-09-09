//! Lenient RFC 5424 parsing for the stormcast dialect. The emitters are
//! trusted cooperators, not adversaries — anything that doesn't parse as
//! 5424 is kept whole as the message with the sender's address as host,
//! so a malformed line is never dropped.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    /// RFC 3339 as sent, or receive time when absent.
    pub ts: String,
    /// What the node calls itself — the RFC 5424 HOSTNAME field.
    pub host: String,
    /// Where the datagram actually came from. Always the sender's address,
    /// whatever the HOSTNAME field said, because the two can disagree and
    /// only one of them can be dialled: a node's syslog hostname need not
    /// resolve, and CLUSTER.md's whole drill-in story is "everything else
    /// it can ask the node's own API for **once it has an address**".
    pub addr: String,
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
        addr: src.to_string(),
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
        addr: src.to_string(),
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

#[cfg(test)]
mod addr_tests {
    use super::*;

    fn at(line: &str, src: &str) -> LogEvent {
        parse(line, src, || "2026-09-09T00:00:00Z".to_string())
    }

    #[test]
    fn the_address_is_kept_even_when_the_node_names_itself() {
        // What a node calls itself and where it is are different facts, and
        // they routinely disagree: a syslog HOSTNAME need not resolve.
        let e = at(
            "<134>1 2026-09-09T12:00:00Z storm-2c91b3 stormd 1 - - up",
            "192.168.8.106",
        );
        assert_eq!(e.host, "storm-2c91b3");
        assert_eq!(e.addr, "192.168.8.106");
    }

    #[test]
    fn an_unparseable_line_still_carries_the_address() {
        let e = at("not syslog at all", "192.168.8.107");
        assert_eq!(e.host, "192.168.8.107", "falls back to the address as a name");
        assert_eq!(e.addr, "192.168.8.107");
    }

    #[test]
    fn a_dash_hostname_falls_back_to_the_address_for_both() {
        let e = at("<134>1 2026-09-09T12:00:00Z - stormd 1 - - up", "192.168.8.108");
        assert_eq!(e.host, "192.168.8.108");
        assert_eq!(e.addr, "192.168.8.108");
    }
}
