//! Who did what (#15): "who opened a console on that VM" has an answer.
//!
//! One record per act, in the host and by path, for the reason the write gate is: a route added
//! without a call here would be a hole, and the proxies are `any`, so they could never have been
//! listed. Consoles opened, sign-ins and everything that is not a read, whether it was allowed or
//! refused, because a refusal is often the part worth knowing.
//!
//! A record has the path and never the query or the body: a console door's `?token=` is in the query,
//! and a body can be anything. It goes to `<data_dir>/audit.jsonl` and, as a log line, off the node.
//! The file is bounded: at [`CAP_BYTES`] it becomes `audit.jsonl.1`, so at most twice that is kept on
//! a data volume the log ring also lives on (#128).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::http::Method;
use console_core::Viewer;
use serde_json::{json, Value};

pub const CAP_BYTES: u64 = 4 * 1024 * 1024;

/// What an act was, in the words the console offers: what the owner asked about on #15.
#[derive(Debug, PartialEq, Eq)]
pub struct Act {
    pub capability: &'static str,
    pub object: String,
}

/// The act a request is, or `None` for a read.
pub fn act(method: &Method, path: &str, upgrade: bool) -> Option<Act> {
    let object = path.strip_prefix("/api/plugins/").or_else(|| path.strip_prefix("/api/v1/")).unwrap_or(path).to_string();
    let door = |o: &str| o.starts_with("ipmi/console/") || (o.starts_with("vm/console/") && (o.ends_with("/serial") || o.ends_with("/vnc")));
    if upgrade && door(&object) {
        return Some(Act { capability: "console", object });
    }
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    // Signing in records itself, with the name it was given. A plan changes nothing.
    if path == "/api/v1/auth/login" || object == "cluster/plan" {
        return None;
    }
    if path == "/api/v1/auth/logout" {
        return Some(Act { capability: "sign-in", object: "signed out".into() });
    }
    let capability = if console_core::storage::classify(method, path, None).is_some() {
        "storage"
    } else if object.starts_with("reg/") || object.starts_with("img/") {
        "image"
    } else if object.starts_with("ipmi/") && object.contains("/power") {
        "power"
    } else if object.starts_with("cluster/") {
        "cluster"
    } else if object.starts_with("update/") {
        "update"
    } else if *method == Method::DELETE || object.ends_with("/delete") {
        "delete"
    } else {
        "change"
    };
    Some(Act { capability, object })
}

pub struct Audit {
    file: PathBuf,
    /// Said once: an audit that cannot write must not fill the log saying so.
    failing: Mutex<bool>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Audit {
    pub fn new(data_dir: &str) -> Self {
        Self { file: Path::new(data_dir).join("audit.jsonl"), failing: Mutex::new(false) }
    }

    fn append(&self, line: &str) -> std::io::Result<()> {
        if std::fs::metadata(&self.file).map(|m| m.len()).unwrap_or(0) >= CAP_BYTES {
            std::fs::rename(&self.file, self.file.with_extension("jsonl.1"))?;
        }
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&self.file)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")
    }

    fn write(&self, record: Value) {
        tracing::info!(target: "audit", "{record}");
        let result = self.append(&record.to_string());
        let mut failing = self.failing.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Err(e) if !*failing => {
                *failing = true;
                tracing::warn!(file = %self.file.display(), "the audit file cannot be written, acts go only to the log: {e}");
            }
            Ok(()) => *failing = false,
            Err(_) => {}
        }
    }

    pub fn record(&self, who: &Viewer, from: Option<std::net::IpAddr>, act: &Act, status: u16) {
        self.write(json!({
            "at": now(),
            "who": who.user.as_deref().unwrap_or("anonymous"),
            "roles": who.roles,
            "from": from.map(|a| a.to_string()),
            "capability": act.capability,
            "object": act.object,
            "status": status,
            // A console a viewer opens shows and takes no typing.
            "may_write": who.may_write(),
        }));
    }

    pub fn sign_in(&self, name: &str, from: Option<std::net::IpAddr>, ok: bool) {
        self.write(json!({
            "at": now(),
            "who": name,
            "from": from.map(|a| a.to_string()),
            "capability": "sign-in",
            "object": if ok { "signed in" } else { "refused" },
            "status": if ok { 200 } else { 401 },
        }));
    }

    /// The newest first, narrowed. Lines that do not parse are skipped: a torn last line after a
    /// power loss is not worth failing the page over.
    pub fn read(&self, who: Option<&str>, capability: Option<&str>, limit: usize) -> Vec<Value> {
        let mut out = Vec::new();
        for file in [self.file.clone(), self.file.with_extension("jsonl.1")] {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            for line in text.lines().rev() {
                let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
                let wanted = |k: &str, w: Option<&str>| w.is_none_or(|w| w.is_empty() || v[k] == w);
                if wanted("who", who) && wanted("capability", capability) {
                    out.push(v);
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(m: Method, p: &str) -> Option<&'static str> {
        act(&m, p, false).map(|a| a.capability)
    }

    #[test]
    fn a_read_is_not_an_act() {
        assert_eq!(cap(Method::GET, "/api/plugins/k8s/pods/a/b"), None);
        assert_eq!(cap(Method::GET, "/api/plugins/vm/console/a/b/serial"), None, "not a door until it upgrades");
        assert_eq!(cap(Method::POST, "/api/plugins/cluster/plan"), None, "a dry run");
        assert_eq!(cap(Method::POST, "/api/v1/auth/login"), None, "it records itself, with the name");
    }

    #[test]
    fn what_the_owner_asked_about_is_named() {
        let door = |p: &str| act(&Method::GET, p, true).map(|a| a.capability);
        assert_eq!(door("/api/plugins/vm/console/shop/vm1/serial"), Some("console"));
        assert_eq!(door("/api/plugins/vm/console/shop/vm1/vnc"), Some("console"));
        assert_eq!(door("/api/plugins/ipmi/console/ns/host"), Some("console"));
        assert_eq!(door("/api/v1/ws/components"), None, "the feed's socket is a read");
        assert_eq!(cap(Method::DELETE, "/api/plugins/k8s/raw/api/v1/namespaces/a/pods/b"), Some("delete"));
        assert_eq!(cap(Method::POST, "/api/plugins/k8s/pods/a/b/delete"), Some("delete"));
        assert_eq!(cap(Method::POST, "/api/plugins/reg/proxy/v1/goldens"), Some("image"));
        assert_eq!(cap(Method::POST, "/api/plugins/ipmi/proxy/api/v1/machines/T1/power/off"), Some("power"));
        assert_eq!(cap(Method::POST, "/api/plugins/cluster/form"), Some("cluster"));
        assert_eq!(cap(Method::POST, "/api/plugins/update/upgrade"), Some("update"));
        assert_eq!(cap(Method::DELETE, "/api/plugins/sb/proxy/api/v1/volumes/v1"), Some("storage"), "a guarded one, as #82 has it");
        assert_eq!(cap(Method::PUT, "/api/plugins/vm/machines/a/b/settings"), Some("change"));
        assert_eq!(cap(Method::POST, "/api/v1/auth/logout"), Some("sign-in"));
    }

    #[test]
    fn it_is_kept_and_read_newest_first_and_never_grows_past_its_bound() {
        let dir = std::env::temp_dir().join(format!("audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = Audit::new(dir.to_str().unwrap());
        let who = Viewer { user: Some("alice".into()), ..Default::default() };
        a.record(&who, None, &Act { capability: "console", object: "vm/console/a/b/serial".into() }, 101);
        a.record(&who, None, &Act { capability: "delete", object: "x".into() }, 403);
        a.sign_in("bob", None, false);
        let all = a.read(None, None, 10);
        assert_eq!(all[0]["who"], "bob");
        assert_eq!(all[0]["status"], 401);
        assert_eq!(all[2]["capability"], "console");
        assert_eq!(a.read(Some("alice"), None, 10).len(), 2);
        assert_eq!(a.read(None, Some("console"), 10).len(), 1);
        assert_eq!(a.read(None, None, 1).len(), 1);
        // A torn line does not lose the rest.
        std::fs::OpenOptions::new().append(true).open(&a.file).unwrap().write_all(b"{\"at\":1,").unwrap();
        assert_eq!(a.read(None, None, 10).len(), 3);
        // At the bound it moves aside, and the older half is still read.
        std::fs::write(&a.file, format!("{{\"who\":\"old\"}}\n{}", " ".repeat(CAP_BYTES as usize))).unwrap();
        a.sign_in("new", None, true);
        assert!(std::fs::metadata(&a.file).unwrap().len() < 1024);
        let names: Vec<_> = a.read(None, None, 10).iter().map(|v| v["who"].clone()).collect();
        assert_eq!(names, ["new", "old"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
