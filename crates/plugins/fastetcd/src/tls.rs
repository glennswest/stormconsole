//! The client fastetcd is spoken to with (#47).
//!
//! stormcos moves fastetcd's client port to mutual TLS (stormcos#81, its
//! docs/SECURITY.md): every listener on a node is TLS with a stormcert
//! certificate, every client verifies it against the node CA and
//! authenticates. So with `[fastetcd] ca_file`, `cert_file` and `key_file`
//! set, the client trusts **only** that CA (no built-in roots — a public CA
//! has no business vouching for the node's datastore) and presents the
//! pair.
//!
//! Two properties beyond building it once:
//!
//! - **It follows the files.** stormcert renews pairs in place; a client
//!   built at start would present an expired certificate until somebody
//!   restarted the console. Each poll compares the files' modification
//!   times and rebuilds when one moved.
//! - **A bad file is the etcd card's problem, not the console's.** A pair
//!   not yet minted, unreadable or not PEM is said on the store row, naming
//!   the file; the rest of the console keeps running, and the next poll
//!   tries again. Refusing to start over it would take every other page
//!   down with the datastore's.

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

/// The three files, as configured. `None` for one not set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TlsFiles {
    pub ca: Option<PathBuf>,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
}

impl TlsFiles {
    pub fn is_empty(&self) -> bool {
        self.ca.is_none() && self.cert.is_none() && self.key.is_none()
    }

    fn all(&self) -> impl Iterator<Item = &PathBuf> {
        [&self.ca, &self.cert, &self.key].into_iter().flatten()
    }

    /// What identifies this version of the files: each one's mtime.
    fn stamp(&self) -> Vec<Option<SystemTime>> {
        self.all().map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).collect()
    }
}

struct Built {
    client: reqwest::Client,
    stamp: Vec<Option<SystemTime>>,
    error: Option<String>,
}

/// The current client, rebuilt when its files change.
pub struct Client {
    files: TlsFiles,
    built: RwLock<Built>,
}

impl Client {
    pub fn new(files: TlsFiles) -> Self {
        let c = Self {
            built: RwLock::new(Built { client: reqwest::Client::new(), stamp: Vec::new(), error: None }),
            files,
        };
        c.rebuild();
        c
    }

    /// The client to send with. Cheap: a reqwest client is an Arc.
    pub fn get(&self) -> reqwest::Client {
        self.built.read().unwrap_or_else(|e| e.into_inner()).client.clone()
    }

    /// Why the configured TLS could not be set up, if it could not.
    pub fn error(&self) -> Option<String> {
        self.built.read().unwrap_or_else(|e| e.into_inner()).error.clone()
    }

    /// Does this client verify and present certificates?
    pub fn is_tls(&self) -> bool {
        !self.files.is_empty()
    }

    /// Rebuild when a file has changed since the last build — or the last
    /// one failed, so a pair minted after the console started is picked up.
    pub fn refresh(&self) {
        if self.files.is_empty() {
            return;
        }
        let changed = {
            let b = self.built.read().unwrap_or_else(|e| e.into_inner());
            b.error.is_some() || b.stamp != self.files.stamp()
        };
        if changed {
            self.rebuild();
        }
    }

    fn rebuild(&self) {
        let stamp = self.files.stamp();
        let next = match build(&self.files) {
            Ok(client) => Built { client, stamp, error: None },
            // Keep the old client: a renewal caught half-written should not
            // drop a working connection's replacement to nothing. The error
            // is still said, and the next poll tries again.
            Err(e) => {
                let old = self.get();
                Built { client: old, stamp, error: Some(e) }
            }
        };
        *self.built.write().unwrap_or_else(|e| e.into_inner()) = next;
    }
}

fn read(p: &Path, what: &str) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("[fastetcd] {what} {}: {e}", p.display()))
}

/// The client these files describe.
pub fn build(files: &TlsFiles) -> Result<reqwest::Client, String> {
    let mut b = reqwest::Client::builder().use_rustls_tls();
    if let Some(ca) = &files.ca {
        let pem = read(ca, "ca_file")?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| format!("[fastetcd] ca_file {}: not PEM certificates: {e}", ca.display()))?;
        if certs.is_empty() {
            return Err(format!("[fastetcd] ca_file {}: holds no certificate", ca.display()));
        }
        b = b.tls_built_in_root_certs(false);
        for c in certs {
            b = b.add_root_certificate(c);
        }
    }
    match (&files.cert, &files.key) {
        (Some(cert), Some(key)) => {
            let mut pem = read(cert, "cert_file")?;
            pem.push(b'\n');
            pem.extend(read(key, "key_file")?);
            let id = reqwest::Identity::from_pem(&pem).map_err(|e| {
                format!(
                    "[fastetcd] cert_file {} / key_file {}: not a certificate and its key in PEM: {e}",
                    cert.display(),
                    key.display()
                )
            })?;
            b = b.identity(id);
        }
        (None, None) => {}
        // The config check refuses this; said again here for a caller that
        // built files some other way.
        _ => return Err("[fastetcd] cert_file and key_file go together".into()),
    }
    b.build().map_err(|e| format!("[fastetcd] TLS client: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sc-etcd-tls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn no_files_is_the_plain_client() {
        let c = Client::new(TlsFiles::default());
        assert!(!c.is_tls());
        assert_eq!(c.error(), None);
    }

    #[test]
    fn a_missing_file_is_named_and_does_not_panic() {
        let files = TlsFiles { ca: Some("/nonexistent/ca.pem".into()), ..Default::default() };
        let c = Client::new(files);
        let e = c.error().unwrap();
        assert!(e.contains("ca_file /nonexistent/ca.pem"), "{e}");
    }

    #[test]
    fn junk_is_not_a_ca_or_a_pair() {
        let ca = tmp("junk-ca.pem", "not a certificate\n");
        let e = build(&TlsFiles { ca: Some(ca.clone()), ..Default::default() }).unwrap_err();
        assert!(e.contains("ca_file") && e.contains(&ca.display().to_string()), "{e}");
        let cert = tmp("junk-cert.pem", "nope\n");
        let key = tmp("junk-key.pem", "nope\n");
        let e = build(&TlsFiles { cert: Some(cert), key: Some(key), ..Default::default() }).unwrap_err();
        assert!(e.contains("cert_file") && e.contains("key_file"), "{e}");
    }

    #[test]
    fn half_a_pair_is_refused() {
        let cert = tmp("half.pem", "x");
        assert!(build(&TlsFiles { cert: Some(cert), ..Default::default() }).unwrap_err().contains("go together"));
    }

    #[test]
    fn a_failed_build_is_retried_on_refresh() {
        let dir = std::env::temp_dir().join(format!("sc-etcd-tls-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = dir.join("ca.pem");
        let c = Client::new(TlsFiles { ca: Some(ca.clone()), ..Default::default() });
        assert!(c.error().unwrap().contains("ca_file"));
        // Still missing: still said.
        c.refresh();
        assert!(c.error().is_some());
    }
}
