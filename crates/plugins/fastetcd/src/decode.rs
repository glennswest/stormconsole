//! What a value in the keyspace *is*.
//!
//! rustkube writes objects as JSON. Upstream Kubernetes writes most of
//! them as protobuf inside a four-byte `k8s\0` envelope, and a store that
//! has been migrated can hold both. The envelope is a `runtime.Unknown`
//! whose first field names the type, which is the one thing about a
//! protobuf object that can be read without its schema. So the envelope
//! is opened far enough to say what the object is, and the body is shown
//! as what it is: bytes.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize, PartialEq)]
pub struct Decoded {
    /// `json`, `protobuf`, `text` or `binary`.
    pub encoding: &'static str,
    /// The object, for JSON.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<Value>,
    /// The value as text, for text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `apiVersion` and `kind`, from JSON or from the protobuf envelope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The first bytes as hex, for what cannot be shown any other way.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// Why the value is not shown whole.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

const K8S_MAGIC: &[u8] = b"k8s\0";
const HEX_PREVIEW: usize = 256;

pub fn decode(bytes: &[u8]) -> Decoded {
    let blank = Decoded {
        encoding: "binary",
        json: None,
        text: None,
        api_version: None,
        kind: None,
        hex: None,
        note: String::new(),
    };
    if let Some(body) = bytes.strip_prefix(K8S_MAGIC) {
        let (api_version, kind, raw) = unknown(body);
        return Decoded {
            encoding: "protobuf",
            api_version,
            kind,
            hex: Some(hex(raw.unwrap_or(body))),
            note: format!(
                "Kubernetes protobuf: the envelope names the type; the {} byte body needs the type's schema to read",
                raw.map(<[u8]>::len).unwrap_or(body.len())
            ),
            ..blank
        };
    }
    if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
        if v.is_object() || v.is_array() {
            let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
            return Decoded {
                encoding: "json",
                api_version: s("apiVersion"),
                kind: s("kind"),
                json: Some(v),
                ..blank
            };
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(t) if !t.chars().any(|c| c.is_control() && !c.is_whitespace()) => {
            Decoded { encoding: "text", text: Some(t.to_string()), ..blank }
        }
        _ => Decoded {
            hex: Some(hex(bytes)),
            note: if bytes.len() > HEX_PREVIEW {
                format!("{} bytes; the first {HEX_PREVIEW} shown", bytes.len())
            } else {
                String::new()
            },
            ..blank
        },
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().take(HEX_PREVIEW).map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

/// `runtime.Unknown { TypeMeta typeMeta = 1; bytes raw = 2; … }`,
/// `TypeMeta { string apiVersion = 1; string kind = 2; }`.
fn unknown(b: &[u8]) -> (Option<String>, Option<String>, Option<&[u8]>) {
    let mut api_version = None;
    let mut kind = None;
    let mut raw = None;
    for (field, bytes) in fields(b) {
        match field {
            1 => {
                for (f, v) in fields(bytes) {
                    let s = std::str::from_utf8(v).ok().map(str::to_string);
                    match f {
                        1 => api_version = s,
                        2 => kind = s,
                        _ => {}
                    }
                }
            }
            2 => raw = Some(bytes),
            _ => {}
        }
    }
    (api_version, kind, raw)
}

/// The length-delimited fields of one protobuf message, in order. Stops at
/// the first thing that is not well formed rather than guessing past it.
fn fields(mut b: &[u8]) -> Vec<(u64, &[u8])> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let Some((tag, n)) = varint(b) else { break };
        b = &b[n..];
        let (field, wire) = (tag >> 3, tag & 7);
        match wire {
            0 => match varint(b) {
                Some((_, n)) => b = &b[n..],
                None => break,
            },
            1 if b.len() >= 8 => b = &b[8..],
            5 if b.len() >= 4 => b = &b[4..],
            2 => {
                let Some((len, n)) = varint(b) else { break };
                let len = len as usize;
                if b.len() < n + len {
                    break;
                }
                out.push((field, &b[n..n + len]));
                b = &b[n + len..];
            }
            _ => break,
        }
    }
    out
}

fn varint(b: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, byte) in b.iter().enumerate().take(10) {
        v |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ld(field: u8, bytes: &[u8]) -> Vec<u8> {
        let mut v = vec![(field << 3) | 2, bytes.len() as u8];
        v.extend_from_slice(bytes);
        v
    }

    #[test]
    fn rustkube_writes_json() {
        let d = decode(br#"{"apiVersion":"v1","kind":"Namespace","metadata":{"name":"default"}}"#);
        assert_eq!(d.encoding, "json");
        assert_eq!(d.kind.as_deref(), Some("Namespace"));
        assert_eq!(d.json.unwrap()["metadata"]["name"], "default");
    }

    #[test]
    fn the_protobuf_envelope_says_what_it_holds() {
        let mut tm = ld(1, b"apps/v1");
        tm.extend(ld(2, b"Deployment"));
        let mut unk = ld(1, &tm);
        unk.extend(ld(2, &[0x0a, 0x03, b'w', b'e', b'b']));
        let mut v = b"k8s\0".to_vec();
        v.extend(unk);
        let d = decode(&v);
        assert_eq!(d.encoding, "protobuf");
        assert_eq!(d.api_version.as_deref(), Some("apps/v1"));
        assert_eq!(d.kind.as_deref(), Some("Deployment"));
        assert_eq!(d.hex.as_deref(), Some("0a 03 77 65 62"));
        assert!(d.note.contains("5 byte body"), "{}", d.note);
    }

    #[test]
    fn a_truncated_envelope_is_not_a_panic() {
        let d = decode(b"k8s\0\x0a\x7f\x0a");
        assert_eq!(d.encoding, "protobuf");
        assert_eq!(d.kind, None);
    }

    #[test]
    fn text_and_bytes() {
        // rustkube's service IP allocations are plain strings.
        assert_eq!(decode(b"default/kubernetes").encoding, "text");
        assert_eq!(decode(b"42").encoding, "text");
        let d = decode(&[0u8, 1, 2, 0xff]);
        assert_eq!(d.encoding, "binary");
        assert_eq!(d.hex.as_deref(), Some("00 01 02 ff"));
    }
}
