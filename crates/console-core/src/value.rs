//! Small helpers for plugins that map a daemon's own JSON into components.

use serde_json::Value;

/// The first of `keys` present on `v` as text — strings as they are,
/// numbers and booleans printed.
pub fn field(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        match v.get(*k) {
            Some(Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            Some(Value::Number(n)) => return Some(n.to_string()),
            Some(Value::Bool(b)) => return Some(b.to_string()),
            _ => {}
        }
    }
    None
}

pub fn u64_field(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

/// `1.5 GB`, the way stormblock prints its own sizes.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn field_takes_the_first_present_key() {
        let v = json!({"name": "", "id": 7, "digest": "sha256:ab"});
        assert_eq!(field(&v, &["name", "digest", "id"]), Some("sha256:ab".into()));
        assert_eq!(field(&v, &["id"]), Some("7".into()));
        assert_eq!(field(&v, &["nope"]), None);
    }

    #[test]
    fn human_bytes_matches_stormblock() {
        assert_eq!(human_bytes(134217728), "128.0 MB");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(26986151936), "25.1 GB");
    }
}
