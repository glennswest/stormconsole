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
