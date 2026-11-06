//! Bringing a disk in by URL (#44): the form's fields as the registry's `POST /v1/media`.

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize, Debug, Default)]
pub struct Import {
    pub url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub format: String,
}

/// The name from a URL's file: `/images/rocky-9.4.qcow2?x=1` is `rocky-9.4`.
fn from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let file = path.rsplit('/').next().unwrap_or("");
    let stem = ["qcow2", "img", "raw", "vmdk", "ova", "iso"]
        .iter()
        .find_map(|e| file.strip_suffix(&format!(".{e}")))
        .unwrap_or(file);
    stem.to_ascii_lowercase()
}

fn good_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 63
        && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_'))
        && n.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// What goes to the registry, or why it should not. Only http and https: the registry fetches this on
/// the node, so `file:` would read the node's own files.
pub fn request(i: &Import) -> Result<Value, String> {
    let url = i.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) || url.len() < 9 {
        return Err("the URL must be http:// or https://".into());
    }
    let name = match i.name.trim() {
        "" => from_url(url),
        n => n.to_string(),
    };
    if !good_name(&name) {
        return Err(format!(
            "{name:?} is not a usable name: lower-case letters, digits, '-', '.' and '_', starting with a letter or digit"
        ));
    }
    let sha = i.sha256.trim().trim_start_matches("sha256:").to_ascii_lowercase();
    if !sha.is_empty() && !(sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err("a sha256 is 64 hex digits".into());
    }
    let format = match i.format.trim() {
        "" | "detect" => None,
        f @ ("raw" | "qcow2" | "vmdk" | "ova" | "iso") => Some(f),
        f => return Err(format!("{f:?} is not a format this takes: qcow2, raw, vmdk, ova or iso")),
    };
    let mut body = json!({"repository": format!("media/{name}"), "name": name, "url": url, "hold": true});
    if !sha.is_empty() {
        // The registry takes a digest with its algorithm, and refuses a bare one.
        body["sha256"] = json!(format!("sha256:{sha}"));
    }
    if let Some(f) = format {
        body["format"] = json!(f);
    }
    Ok(body)
}

/// The registry's refusal, in words. With auth on it wants an admin, and the console holds no credential
/// for it (#35).
pub fn refusal(status: u16, said: &Value) -> String {
    if status == 401 || status == 403 {
        return "the registry wants an administrator's credential for an import, and this console holds none for it (#35)".into();
    }
    let error = said.get("error").and_then(Value::as_str).unwrap_or("it did not say why");
    match said.get("fault").and_then(Value::as_str) {
        Some(f) => format!("{error} (the fault is the {f}'s)"),
        None => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(url: &str, name: &str, sha: &str, format: &str) -> Result<Value, String> {
        request(&Import { url: url.into(), name: name.into(), sha256: sha.into(), format: format.into() })
    }

    #[test]
    fn the_form_becomes_a_held_import() {
        let b = asked("https://x/images/Rocky-9.4.qcow2?y=1", "", "", "detect").unwrap();
        assert_eq!(b["repository"], "media/rocky-9.4", "named from the file");
        assert_eq!(b["hold"], true, "fetched once and verified, and answered at once");
        assert!(b.get("format").is_none(), "detect means the registry is not told one");
        assert!(b.get("sha256").is_none());
        let sha = "A".repeat(64);
        let b = asked("http://x/a.img", "mine", &format!("sha256:{sha}"), "raw").unwrap();
        assert_eq!((b["name"].as_str(), b["format"].as_str()), (Some("mine"), Some("raw")));
        assert_eq!(b["sha256"], format!("sha256:{}", "a".repeat(64)));
    }

    #[test]
    fn what_it_will_not_ask_for() {
        for url in ["file:///etc/passwd", "ftp://x/a.img", "", "http://"] {
            assert!(asked(url, "n", "", "").is_err(), "{url}");
        }
        assert!(asked("http://x/a.img", "../etc", "", "").is_err(), "not a path");
        assert!(asked("http://x/a.img", "Bad Name", "", "").is_err());
        assert!(asked("http://x/", "", "", "").is_err(), "no name to be had");
        assert!(asked("http://x/a.img", "n", "abc", "").is_err(), "a short sha256");
        assert!(asked("http://x/a.img", "n", "", "exe").is_err());
    }

    #[test]
    fn a_refusal_says_whose_it_is() {
        assert!(refusal(401, &json!({})).contains("#35"));
        assert_eq!(refusal(502, &json!({"error": "404 from x", "fault": "upstream"})), "404 from x (the fault is the upstream's)");
        assert_eq!(refusal(400, &json!({"error": "bad"})), "bad");
    }
}
