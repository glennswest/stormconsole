//! This node's block engine, as every plugin that asks it reaches it (#160, stormcos#374).
//!
//! `:9090` is moving to TLS from the node CA (stormblock#203). Three plugins ask the engine, and one that
//! kept a plain client would quietly lose it on the day it moves, so they share this: the address, the
//! trust and the token, in one place.
//!
//! - **The URL's scheme decides.** `http://` is as it was, until the engine's `tls_mode = required`.
//!   `https://` is verified against `[stormblock] ca_file`, and only that CA. There is no way to skip the
//!   check, and no falling back to plain: an engine whose certificate does not verify is not read.
//! - **It follows the files**, as [`crate::tls::Client`] does, so a renewal needs no restart.
//! - **A client pair is optional.** The engine takes a node-CA certificate at the node token's tier. A
//!   destructive verb still goes with the viewer's bearer (#82), which it reviews.

use std::sync::Arc;

use crate::tls::{Client, TlsFiles};
use crate::token::FileToken;

/// Where the engine's own CLI looks last for a token on the machine it runs on (stormblock `mgmt/auth.rs`).
pub const TOKEN_FILES: [&str; 2] = ["/etc/stormblock/api_token", "/var/lib/stormblock/api_token"];

/// The console's credential for the engine at `url`, found as the engine's own client finds it (#30): `[stormblock]
/// token_file` when set, then `$STORMBLOCK_API_TOKEN`, then, for an engine on this machine only, `$STORMBLOCK_TOKEN_FILE`
/// and `defaults`. A token from one of those files is this node's: it means nothing to a peer and is not sent to one.
/// `$STORMBLOCK_API_TOKEN` is a shared one, named on purpose, so it goes wherever the url says.
pub fn find_token(explicit: Option<&str>, url: &str, env: impl Fn(&str) -> Option<String>, defaults: &[&str]) -> FileToken {
    if let Some(f) = explicit {
        return FileToken::file(f);
    }
    if let Some(t) = env("STORMBLOCK_API_TOKEN").filter(|t| !t.trim().is_empty()) {
        return FileToken::fixed(Some(t));
    }
    if !crate::upstream::is_loopback(url) {
        return FileToken::fixed(None);
    }
    let named = env("STORMBLOCK_TOKEN_FILE").filter(|f| !f.trim().is_empty());
    FileToken::first_of(named.into_iter().map(Into::into).chain(defaults.iter().map(Into::into)))
}

pub struct Engine {
    base: String,
    tls: Client,
    token: Arc<FileToken>,
}

impl Engine {
    pub fn new(url: &str, files: TlsFiles, token: Arc<FileToken>) -> Self {
        Self { base: url.trim_end_matches('/').to_string(), tls: Client::new("stormblock", files), token }
    }

    /// As configured, with no certificate: `http://`, a test or a console with nothing to verify.
    pub fn plain(url: &str, token: Arc<FileToken>) -> Self {
        Self::new(url, TlsFiles::default(), token)
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The console's own credential for it.
    pub fn token(&self) -> Option<String> {
        self.token.get()
    }

    /// The client to send with, as the files are now. `Err` names the file that stops it, and nothing is
    /// read until it is put right.
    pub fn client(&self) -> Result<reqwest::Client, String> {
        self.tls.refresh();
        match self.tls.error() {
            Some(e) => Err(e),
            None => Ok(self.tls.get()),
        }
    }

    /// A GET with the console's credential.
    pub fn get(&self, path: &str) -> Result<reqwest::RequestBuilder, String> {
        let req = self.client()?.get(format!("{}{path}", self.base));
        Ok(match self.token() {
            Some(t) => req.bearer_auth(t),
            None => req,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Arc<FileToken> {
        Arc::new(FileToken::fixed(None))
    }

    #[test]
    fn plain_is_as_it_was() {
        let e = Engine::plain("http://127.0.0.1:9090/", none());
        assert_eq!(e.base(), "http://127.0.0.1:9090");
        assert!(e.client().is_ok());
    }

    #[test]
    fn a_ca_not_there_stops_it_and_says_which() {
        let files = TlsFiles { ca: Some("/nonexistent/ca.crt".into()), cert: None, key: None };
        let e = Engine::new("https://127.0.0.1:9090", files, Arc::new(FileToken::fixed(Some("t".into()))));
        let why = e.get("/api/v1/health").err().expect("read with no CA");
        assert!(why.contains("[stormblock]") && why.contains("/nonexistent/ca.crt"), "{why}");
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("find-token-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const LOCAL: &str = "http://127.0.0.1:9090";
    const PEER: &str = "http://192.168.8.9:9090";

    #[test]
    fn the_order_the_engines_own_client_uses() {
        let d = dir("order");
        let (cfg, named, def) = (d.join("cfg"), d.join("named"), d.join("def"));
        for (p, w) in [(&cfg, "from-config"), (&named, "from-named"), (&def, "from-default")] {
            std::fs::write(p, w).unwrap();
        }
        let (c, n, f) = (cfg.to_str().unwrap(), named.to_str().unwrap().to_string(), def.to_str().unwrap());
        let both = |k: &str| match k {
            "STORMBLOCK_API_TOKEN" => Some("from-env".to_string()),
            "STORMBLOCK_TOKEN_FILE" => Some(n.clone()),
            _ => None,
        };
        let file_only = |k: &str| (k == "STORMBLOCK_TOKEN_FILE").then(|| n.clone());
        assert_eq!(find_token(Some(c), LOCAL, both, &[f]).get().as_deref(), Some("from-config"));
        assert_eq!(find_token(None, LOCAL, both, &[f]).get().as_deref(), Some("from-env"));
        assert_eq!(find_token(None, LOCAL, file_only, &[f]).get().as_deref(), Some("from-named"));
        assert_eq!(find_token(None, LOCAL, |_| None, &[f]).get().as_deref(), Some("from-default"));
        std::fs::remove_file(&named).unwrap();
        assert_eq!(find_token(None, LOCAL, file_only, &[f]).get().as_deref(), Some("from-default"), "a named file not there yet falls through");
        assert_eq!(find_token(None, LOCAL, |_| None, &[]).get(), None);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_nodes_own_token_is_not_sent_to_a_peer() {
        let d = dir("peer");
        let (named, def) = (d.join("named"), d.join("def"));
        std::fs::write(&named, "node").unwrap();
        std::fs::write(&def, "node").unwrap();
        let n = named.to_str().unwrap().to_string();
        let file_only = |k: &str| (k == "STORMBLOCK_TOKEN_FILE").then(|| n.clone());
        assert_eq!(find_token(None, PEER, file_only, &[def.to_str().unwrap()]).get(), None);
        let shared = |k: &str| (k == "STORMBLOCK_API_TOKEN").then(|| "shared".to_string());
        assert_eq!(find_token(None, PEER, shared, &[]).get().as_deref(), Some("shared"), "named on purpose");
        assert_eq!(find_token(Some(named.to_str().unwrap()), PEER, |_| None, &[]).get().as_deref(), Some("node"), "the config says so");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
