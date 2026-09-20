//! The node capability beacon, read off the log group.
//!
//! stormcos#26: every node puts an RFC 5424 structured-data element —
//! `[storm-beacon@0 cores="8" ...]` — on the same multicast group as its logs,
//! every thirty seconds. This is the reading half. The collector already sees
//! every datagram, so the beacon costs no socket, no discovery and no polling:
//! a node that is talking at all is a node whose shape is known.
//!
//! # Fields are not hardcoded
//!
//! The issue says "stormconsole will implement whatever shape stormcos lands",
//! and a parser with a fixed struct makes that a lie — every new field would
//! need a release here before it could be seen. So every parameter is kept,
//! and the typed accessors below are conveniences over the map rather than the
//! schema. A field this console has never heard of still reaches the node card.
//!
//! # Absent is not zero
//!
//! The emitter omits what it cannot read rather than sending an empty value,
//! precisely so a reader can tell "no role" from "role unknown". Nothing here
//! may undo that by defaulting: every accessor returns `Option`.

use std::collections::BTreeMap;

/// The SD-ID a node's beacon carries.
pub const SD_ID: &str = "storm-beacon@0";

/// One node's most recent beacon.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Beacon {
    /// The RFC 5424 HOSTNAME the beacon was sent with.
    pub host: String,
    /// Where the datagram came from — the address a console can actually dial.
    pub addr: String,
    /// The beacon's own timestamp, as sent.
    pub ts: String,
    /// Every parameter, whether or not this build knows what it means.
    pub params: BTreeMap<String, String>,
}

impl Beacon {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(String::as_str)
    }

    pub fn num(&self, key: &str) -> Option<u64> {
        self.get(key)?.parse().ok()
    }

    pub fn cores(&self) -> Option<u64> {
        self.num("cores")
    }

    pub fn mem_bytes(&self) -> Option<u64> {
        self.num("mem_bytes")
    }

    pub fn drives(&self) -> Option<u64> {
        self.num("drives")
    }

    pub fn uptime_s(&self) -> Option<u64> {
        self.num("uptime_s")
    }

    pub fn running(&self) -> Option<u64> {
        self.num("running")
    }

    pub fn failed(&self) -> Option<u64> {
        self.num("failed")
    }

    pub fn arch(&self) -> Option<&str> {
        self.get("arch")
    }

    pub fn release(&self) -> Option<&str> {
        self.get("release")
    }

    pub fn role(&self) -> Option<&str> {
        self.get("role")
    }

    /// The pallets this node has mounted.
    ///
    /// Empty rather than `None` when the field is absent: a caller wants to
    /// iterate, and the distinction between "no pallets field" and "no
    /// pallets" is available through `get` for anyone who needs it.
    pub fn pallets(&self) -> Vec<&str> {
        match self.get("pallets") {
            Some(s) if !s.is_empty() => s.split(',').filter(|p| !p.is_empty()).collect(),
            _ => Vec::new(),
        }
    }
}

/// Read a beacon out of a raw syslog line, if it carries one.
///
/// Returns `None` for every ordinary log line, which is almost all of them —
/// this runs on every datagram the collector takes, so the cheap rejection
/// comes first.
pub fn from_line(line: &str, src: &str) -> Option<Beacon> {
    if !line.contains(SD_ID) {
        return None;
    }
    let line = line.trim_end_matches(['\r', '\n']);
    let rest = strip_pri(line);
    // VERSION SP TIMESTAMP SP HOSTNAME SP APP-NAME SP PROCID SP MSGID SP SD
    let mut it = rest.splitn(7, ' ');
    let (Some(ver), Some(ts), Some(host), Some(_app), Some(_pid), Some(_msgid), Some(tail)) = (
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
    ) else {
        return None;
    };
    if ver != "1" {
        return None;
    }
    let params = element_params(tail, SD_ID)?;
    Some(Beacon {
        host: if host == "-" { src.to_string() } else { host.to_string() },
        addr: src.to_string(),
        ts: if ts == "-" { String::new() } else { ts.to_string() },
        params,
    })
}

fn strip_pri(line: &str) -> &str {
    let Some(rest) = line.strip_prefix('<') else { return line };
    match rest.find('>') {
        Some(end) => &rest[end + 1..],
        None => line,
    }
}

/// Pull the named element's parameters out of a STRUCTURED-DATA field.
///
/// The field may hold several elements, and only one of them is ours.
fn element_params(sd: &str, id: &str) -> Option<BTreeMap<String, String>> {
    let mut rest = sd.trim_start();
    while rest.starts_with('[') {
        let (body, after) = split_element(rest)?;
        let body = &body[1..body.len() - 1];
        let (this_id, params) = body.split_once(' ').unwrap_or((body, ""));
        if this_id == id {
            return Some(parse_params(params));
        }
        rest = after.trim_start();
    }
    None
}

/// Split `[...]` from what follows, honouring `\]`.
///
/// The escape is the whole reason this is not `find(']')`: a pallet list or an
/// error string containing `]` is escaped by the emitter, and a reader that
/// stops at the first bracket loses every parameter after it while still
/// seeing a well-formed line.
fn split_element(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b']' => return Some((&s[..i + 1], &s[i + 1..])),
            _ => i += 1,
        }
    }
    None
}

/// `name="value" name="value"`, with `\"`, `\\` and `\]` unescaped.
fn parse_params(s: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        let name_start = i;
        while i < b.len() && b[i] != b'=' {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let name = &s[name_start..i];
        i += 1; // '='
        if i >= b.len() || b[i] != b'"' {
            break;
        }
        i += 1; // opening quote
        let mut value = String::new();
        while i < b.len() {
            match b[i] {
                b'\\' if i + 1 < b.len() => {
                    value.push(b[i + 1] as char);
                    i += 2;
                }
                b'"' => {
                    i += 1;
                    break;
                }
                _ => {
                    // Multi-byte UTF-8 has to travel whole.
                    let ch_len = utf8_len(b[i]);
                    value.push_str(&s[i..(i + ch_len).min(s.len())]);
                    i += ch_len;
                }
            }
        }
        if !name.is_empty() {
            out.insert(name.to_string(), value);
        }
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real beacon, captured from storm-06f96d on release 10.87.
    const LINE: &str = r#"<134>1 2026-09-20T21:48:46.017941Z storm-06f96d beacon - - [storm-beacon@0 cores="8" arch="x86_64" running="14" failed="0" mem_bytes="16708300800" drives="3" uptime_s="56" pallets="busybox,cadvisor,cilium"] node beacon — 14 running, 0 failed"#;

    #[test]
    fn a_real_beacon_parses() {
        let b = from_line(LINE, "192.168.30.2").expect("beacon");
        assert_eq!(b.host, "storm-06f96d");
        assert_eq!(b.addr, "192.168.30.2");
        assert_eq!(b.ts, "2026-09-20T21:48:46.017941Z");
        assert_eq!(b.cores(), Some(8));
        assert_eq!(b.arch(), Some("x86_64"));
        assert_eq!(b.mem_bytes(), Some(16_708_300_800));
        assert_eq!(b.drives(), Some(3));
        assert_eq!(b.running(), Some(14));
        assert_eq!(b.failed(), Some(0));
        assert_eq!(b.pallets(), vec!["busybox", "cadvisor", "cilium"]);
    }

    #[test]
    fn an_absent_field_is_none_and_not_zero() {
        // The emitter omits what it cannot read. Reporting 0 cores, or a role
        // of "", would erase exactly the distinction it went to trouble for.
        let b = from_line(LINE, "1.2.3.4").unwrap();
        assert_eq!(b.role(), None);
        assert_eq!(b.release(), None);
    }

    #[test]
    fn an_ordinary_log_line_is_not_a_beacon() {
        let line = "<134>1 2026-09-20T21:48:46Z storm-06f96d stormpump - - - boot complete";
        assert!(from_line(line, "1.2.3.4").is_none());
    }

    #[test]
    fn an_escaped_bracket_does_not_end_the_element() {
        let line = r#"<134>1 - host app - - [storm-beacon@0 pallets="a\]b" cores="2"] msg"#;
        let b = from_line(line, "1.2.3.4").unwrap();
        assert_eq!(b.get("pallets"), Some("a]b"));
        // The parameter after the escaped bracket must survive.
        assert_eq!(b.cores(), Some(2));
    }

    #[test]
    fn quotes_and_backslashes_come_back() {
        let line = r#"<134>1 - host app - - [storm-beacon@0 note="say \"hi\"\\ok"] msg"#;
        let b = from_line(line, "1.2.3.4").unwrap();
        assert_eq!(b.get("note"), Some(r#"say "hi"\ok"#));
    }

    #[test]
    fn our_element_is_found_among_others() {
        let line = r#"<134>1 - host app - - [other@1 x="1"][storm-beacon@0 cores="4"] msg"#;
        assert_eq!(from_line(line, "1.2.3.4").unwrap().cores(), Some(4));
    }

    #[test]
    fn a_field_this_build_has_never_heard_of_is_still_kept() {
        // stormcos owns the shape; a new field must reach the node card
        // without a release here.
        let line = r#"<134>1 - host app - - [storm-beacon@0 cores="4" gpus="2"] msg"#;
        let b = from_line(line, "1.2.3.4").unwrap();
        assert_eq!(b.get("gpus"), Some("2"));
    }

    #[test]
    fn utf8_in_a_value_survives() {
        let line = r#"<134>1 - host app - - [storm-beacon@0 note="café — 8 cores"] msg"#;
        let b = from_line(line, "1.2.3.4").unwrap();
        assert_eq!(b.get("note"), Some("café — 8 cores"));
    }
}
