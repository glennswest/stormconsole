//! The one connection to the apiserver (#33): where it is, who the console
//! is to it, and whose certificate it trusts.
//!
//! stormcert mints the console a ServiceAccount token into a file and renews
//! it in place (stormcert#27), and stormcos mounts that file and the node CA
//! into the console (stormcos#76). Two properties follow:
//!
//! - **The bearer follows its file.** A token read once at start is a
//!   console that goes 401 at the first renewal and stays that way until
//!   somebody restarts it. The file is re-read whenever its modification
//!   time moves — a `stat` per request, which is nothing beside the request.
//! - **The bearer only goes to a verified peer.** With `ca_file`, the client
//!   trusts that CA and nothing else (a public CA has no business vouching
//!   for a node's apiserver), and is rebuilt when the file changes. A CA
//!   that cannot be read **fails closed**: the client trusts no root at all,
//!   so nothing is sent anywhere, and the reason is said on the kubernetes
//!   card rather than as a crash loop that takes every other page down.
//!
//! Everything that speaks to the apiserver as the console — the watches,
//! the plugins' writes, the namespace probes, the storage reviews, the
//! release read — holds the same [`Conn`], so there is one answer to "who
//! are we and whom do we trust", not five that can drift.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

/// Where the console's own bearer comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bearer {
    None,
    Inline(String),
    File(PathBuf),
}

/// How the apiserver's certificate is checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Trust only this CA (a PEM bundle).
    Ca(PathBuf),
    /// The system's roots — a configured server with no CA named.
    System,
    /// Not checked at all (`insecure_skip_tls_verify`, or the zero-config
    /// loopback default with no CA).
    Unverified,
}

struct Token {
    stamp: Option<SystemTime>,
    value: Option<String>,
    error: Option<String>,
}

struct Tls {
    stamp: Option<SystemTime>,
    client: reqwest::Client,
    error: Option<String>,
}

pub struct Conn {
    server: String,
    bearer: Bearer,
    trust: Trust,
    timeout: Option<Duration>,
    token: RwLock<Token>,
    tls: RwLock<Tls>,
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl Conn {
    pub fn new(server: &str, bearer: Bearer, trust: Trust) -> Arc<Self> {
        Self::build(server, bearer, trust, None)
    }

    /// The same connection with a per-request timeout on its client.
    pub fn with_timeout(&self, timeout: Duration) -> Arc<Self> {
        Self::build(&self.server, self.bearer.clone(), self.trust.clone(), Some(timeout))
    }

    fn build(server: &str, bearer: Bearer, trust: Trust, timeout: Option<Duration>) -> Arc<Self> {
        let c = Self {
            server: server.trim_end_matches('/').to_string(),
            bearer,
            trust,
            timeout,
            token: RwLock::new(Token { stamp: None, value: None, error: None }),
            tls: RwLock::new(Tls { stamp: None, client: reqwest::Client::new(), error: None }),
        };
        c.reload_token();
        c.rebuild_tls();
        Arc::new(c)
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn trust(&self) -> &Trust {
        &self.trust
    }

    /// Is the peer's certificate checked?
    pub fn verified(&self) -> bool {
        self.trust != Trust::Unverified
    }

    /// The console's own bearer, as of now.
    pub fn token(&self) -> Option<String> {
        if let Bearer::File(p) = &self.bearer {
            let moved = self.token.read().unwrap_or_else(|e| e.into_inner()).stamp != mtime(p);
            if moved {
                self.reload_token();
            }
        }
        self.token.read().unwrap_or_else(|e| e.into_inner()).value.clone()
    }

    /// The client to send with, rebuilt first if the CA file moved.
    pub fn http(&self) -> reqwest::Client {
        if let Trust::Ca(p) = &self.trust {
            let (stamp, failed) = {
                let t = self.tls.read().unwrap_or_else(|e| e.into_inner());
                (t.stamp, t.error.is_some())
            };
            if failed || stamp != mtime(p) {
                self.rebuild_tls();
            }
        }
        self.tls.read().unwrap_or_else(|e| e.into_inner()).client.clone()
    }

    /// What is wrong with the token or CA file, if anything — for the card.
    pub fn error(&self) -> Option<String> {
        let _ = self.token();
        let _ = self.http();
        let t = self.token.read().unwrap_or_else(|e| e.into_inner()).error.clone();
        let c = self.tls.read().unwrap_or_else(|e| e.into_inner()).error.clone();
        match (t, c) {
            (Some(t), Some(c)) => Some(format!("{c}; {t}")),
            (t, c) => c.or(t),
        }
    }

    fn reload_token(&self) {
        let next = match &self.bearer {
            Bearer::None => Token { stamp: None, value: None, error: None },
            Bearer::Inline(t) => Token { stamp: None, value: Some(t.trim().to_string()), error: None },
            Bearer::File(p) => {
                let stamp = mtime(p);
                match std::fs::read_to_string(p) {
                    Ok(s) if !s.trim().is_empty() => Token { stamp, value: Some(s.trim().to_string()), error: None },
                    Ok(_) => Token {
                        stamp,
                        value: None,
                        error: Some(format!("[kubernetes] token_file {} is empty", p.display())),
                    },
                    Err(e) => Token {
                        stamp,
                        value: None,
                        error: Some(format!("[kubernetes] token_file {}: {e}", p.display())),
                    },
                }
            }
        };
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = next;
    }

    fn rebuild_tls(&self) {
        let stamp = match &self.trust {
            Trust::Ca(p) => mtime(p),
            _ => None,
        };
        let (client, error) = match client(&self.trust, self.timeout) {
            Ok(c) => (c, None),
            // Fail closed: a client that trusts nothing reaches nothing, so
            // a bearer never goes to a peer that was not checked.
            Err(e) => (closed(self.timeout), Some(e)),
        };
        *self.tls.write().unwrap_or_else(|e| e.into_inner()) = Tls { stamp, client, error };
    }
}

fn builder(timeout: Option<Duration>) -> reqwest::ClientBuilder {
    let b = reqwest::Client::builder().use_rustls_tls();
    match timeout {
        Some(t) => b.timeout(t),
        None => b,
    }
}

fn closed(timeout: Option<Duration>) -> reqwest::Client {
    builder(timeout).tls_built_in_root_certs(false).build().unwrap_or_default()
}

/// The client a trust setting describes.
pub fn client(trust: &Trust, timeout: Option<Duration>) -> Result<reqwest::Client, String> {
    let b = builder(timeout);
    let b = match trust {
        Trust::Unverified => b.danger_accept_invalid_certs(true),
        Trust::System => b,
        Trust::Ca(p) => {
            let pem = std::fs::read(p).map_err(|e| format!("[kubernetes] ca_file {}: {e}", p.display()))?;
            let certs = reqwest::Certificate::from_pem_bundle(&pem)
                .map_err(|e| format!("[kubernetes] ca_file {}: not PEM certificates: {e}", p.display()))?;
            if certs.is_empty() {
                return Err(format!("[kubernetes] ca_file {}: holds no certificate", p.display()));
            }
            let mut b = b.tls_built_in_root_certs(false);
            for c in certs {
                b = b.add_root_certificate(c);
            }
            b
        }
    };
    b.build().map_err(|e| format!("[kubernetes] TLS client: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-apiserver-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A renewed token is used without a restart (stormcert#27).
    #[test]
    fn the_bearer_follows_its_file() {
        let p = dir().join("token");
        std::fs::write(&p, "first\n").unwrap();
        let c = Conn::new("https://k:6443/", Bearer::File(p.clone()), Trust::Unverified);
        assert_eq!(c.server(), "https://k:6443");
        assert_eq!(c.token().as_deref(), Some("first"));
        // mtime granularity: make sure the renewal is a different stamp.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&p, "second\n").unwrap();
        let later = SystemTime::now() + Duration::from_secs(5);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(later).unwrap();
        assert_eq!(c.token().as_deref(), Some("second"));
        assert_eq!(c.error(), None);
    }

    #[test]
    fn a_missing_token_file_is_named_and_sends_no_bearer() {
        let c = Conn::new("https://k:6443", Bearer::File("/nonexistent/sa.token".into()), Trust::Unverified);
        assert_eq!(c.token(), None);
        assert!(c.error().unwrap().contains("token_file /nonexistent/sa.token"));
    }

    /// A CA that cannot be read fails closed and says why.
    #[test]
    fn a_bad_ca_fails_closed_and_is_named() {
        let c = Conn::new("https://k:6443", Bearer::Inline("t".into()), Trust::Ca("/nonexistent/ca.crt".into()));
        assert!(c.verified());
        assert!(c.error().unwrap().contains("ca_file /nonexistent/ca.crt"));
        let junk = dir().join("junk.crt");
        std::fs::write(&junk, "not a certificate\n").unwrap();
        let c = Conn::new("https://k:6443", Bearer::None, Trust::Ca(junk.clone()));
        let e = c.error().unwrap();
        assert!(e.contains("ca_file") && e.contains(&junk.display().to_string()), "{e}");
    }

    #[test]
    fn unverified_is_said() {
        assert!(!Conn::new("https://127.0.0.1:6443", Bearer::None, Trust::Unverified).verified());
        assert!(Conn::new("https://k:6443", Bearer::None, Trust::System).verified());
    }
}
