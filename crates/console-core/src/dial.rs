//! Reaching a node's daemons over TLS as they move to it (#49).
//!
//! stormd (stormd#32) and stormdrive (stormdrive#19) serve HTTPS with a node-CA certificate and take
//! a node-CA client certificate. A node moves one daemon at a time (stormcos#371), so a port may
//! speak either, and the console finds out by asking: HTTPS first, every time it is not known.
//!
//! - **A certificate it does not trust is never a reason to fall back.** Reading that port plainly
//!   would make the check worth nothing to anyone who can answer on it, so it is said instead.
//! - **The client pair is only ever offered over TLS**, so a plain port learns nothing.
//! - **A plain answer is remembered briefly** ([`REMEMBER`]), so a daemon that moves to TLS is read
//!   over it within that time. `require` ends the fallback, for when the node has finished moving.

use std::collections::HashMap;
use std::error::Error as _;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::tls::{Client, TlsFiles};

/// How long a port's answer is kept.
pub const REMEMBER: Duration = Duration::from_secs(30);
const ASK: Duration = Duration::from_secs(2);

/// Where to send, and with what.
#[derive(Clone)]
pub struct Route {
    pub client: reqwest::Client,
    /// `https://host:port`, or as it was given.
    pub base: String,
}

/// What a failed HTTPS request says about the port.
#[derive(Debug, PartialEq, Eq)]
enum Failed {
    /// Nothing there, or nothing in time: the caller's own request will say so.
    Down,
    /// It speaks TLS, with a certificate this console does not trust.
    Untrusted,
    /// Not refused and not distrusted: perhaps it does not speak TLS.
    Other,
}

fn classify(text: &str) -> Failed {
    let t = text.to_lowercase();
    if t.contains("certificate") || t.contains("unknownissuer") || t.contains("notvalidforname") {
        return Failed::Untrusted;
    }
    if ["refused", "timed out", "no route", "unreachable", "deadline"].iter().any(|w| t.contains(w)) {
        return Failed::Down;
    }
    Failed::Other
}

/// Every cause in an error's chain, which is where a TLS failure's reason is.
/// An error and its causes, for a card: one level hides what TLS said.
pub fn chain(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut at = e.source();
    while let Some(s) = at {
        out.push_str(": ");
        out.push_str(&s.to_string());
        at = s.source();
    }
    out
}

pub struct Dial {
    tls: Option<Client>,
    plain: reqwest::Client,
    require: bool,
    /// host:port → (speaks TLS, when asked).
    seen: RwLock<HashMap<String, (bool, Instant)>>,
}

impl Dial {
    /// No `[node_tls]`: every daemon as it was configured.
    pub fn off() -> Arc<Self> {
        Arc::new(Self { tls: None, plain: reqwest::Client::new(), require: false, seen: RwLock::default() })
    }

    pub fn new(files: TlsFiles, require: bool) -> Arc<Self> {
        if files.is_empty() {
            return Self::off();
        }
        Arc::new(Self {
            // stormd serves HTTP/1.1 only.
            tls: Some(Client::http1("node_tls", files)),
            plain: reqwest::Client::new(),
            require,
            seen: RwLock::default(),
        })
    }

    fn remembered(&self, at: &str) -> Option<bool> {
        let seen = self.seen.read().unwrap_or_else(|e| e.into_inner());
        seen.get(at).filter(|(_, when)| when.elapsed() < REMEMBER).map(|(tls, _)| *tls)
    }

    fn remember(&self, at: &str, tls: bool) {
        self.seen.write().unwrap_or_else(|e| e.into_inner()).insert(at.to_string(), (tls, Instant::now()));
    }

    /// How to reach `base` (`http://host:port`). `Err` says why it must not be read at all.
    pub async fn route(&self, base: &str) -> Result<Route, String> {
        let base = base.trim_end_matches('/');
        let as_given = || Route { client: self.plain.clone(), base: base.to_string() };
        let Some(tls) = &self.tls else { return Ok(as_given()) };
        tls.refresh();
        if let Some(e) = tls.error() {
            return Err(e);
        }
        let at = base.trim_start_matches("http://").trim_start_matches("https://").to_string();
        let secure = Route { client: tls.get(), base: format!("https://{at}") };
        match self.remembered(&at) {
            Some(true) => return Ok(secure),
            Some(false) => return Ok(as_given()),
            None => {}
        }
        let failed = match secure.client.get(format!("{}/healthz", secure.base)).timeout(ASK).send().await {
            Ok(_) => {
                self.remember(&at, true);
                return Ok(secure);
            }
            Err(e) => chain(&e),
        };
        match classify(&failed) {
            Failed::Down => Ok(if base.starts_with("https://") { secure } else { as_given() }),
            Failed::Untrusted => Err(format!("{at} is not trusted, so it was not read: {failed}")),
            Failed::Other if self.require => Err(format!("{at} does not speak TLS, and [node_tls] require is on: {failed}")),
            Failed::Other => {
                // Does it answer plainly at all? Nothing is presented here.
                match self.plain.get(format!("http://{at}/healthz")).timeout(ASK).send().await {
                    Ok(_) => {
                        self.remember(&at, false);
                        Ok(as_given())
                    }
                    Err(_) => Err(format!("{at} did not answer over TLS or plainly: {failed}")),
                }
            }
        }
    }
}

/// One upstream with its credential: how to reach it through a [`Dial`], and the bearer to send (#35).
///
/// **The bearer goes only over TLS or to this machine.** While a node moves (stormcos#371) a port may
/// answer plainly, and a credential sent plainly to another machine could be read on the way; the
/// registry refuses such a one anyway.
pub struct Reach {
    base: String,
    dial: Arc<Dial>,
    token: Arc<crate::token::FileToken>,
}

/// A way to one upstream, as it is now.
pub struct Reached {
    pub route: Route,
    /// What to present.
    pub bearer: Option<String>,
    /// There is a credential, and this route must not carry it.
    pub withheld: bool,
}

fn may_carry(base: &str) -> bool {
    base.starts_with("https://") || crate::upstream::is_loopback(base)
}

impl Reach {
    pub fn new(base: &str, dial: Arc<Dial>, token: Arc<crate::token::FileToken>) -> Self {
        Self { base: base.trim_end_matches('/').to_string(), dial, token }
    }

    /// The address as configured, for a card.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// `Err` says why it must not be read at all.
    pub async fn reach(&self) -> Result<Reached, String> {
        let route = self.dial.route(&self.base).await?;
        let word = self.token.get();
        let carry = may_carry(&route.base);
        Ok(Reached { withheld: word.is_some() && !carry, bearer: word.filter(|_| carry), route })
    }

    /// A request to `path`, with the credential where it may go.
    pub async fn request(&self, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder, String> {
        let r = self.reach().await?;
        let req = r.route.client.request(method, format!("{}{path}", r.route.base));
        Ok(match r.bearer {
            Some(t) => req.bearer_auth(t),
            None => req,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_credential_goes_over_tls_or_stays_on_this_machine() {
        let word = || Arc::new(crate::token::FileToken::fixed(Some("secret".into())));
        let at = |url: &str| Reach::new(url, Dial::off(), word());
        for url in ["http://127.0.0.1:5100", "https://127.0.0.1:5100", "https://node-1:5100"] {
            let r = at(url).reach().await.unwrap();
            assert_eq!((r.bearer.as_deref(), r.withheld), (Some("secret"), false), "{url}");
        }
        let r = at("http://node-1:5100/").reach().await.unwrap();
        assert_eq!((r.bearer, r.withheld), (None, true), "plainly to another machine");
        assert_eq!(r.route.base, "http://node-1:5100");
        let none = Reach::new("http://node-1:5100", Dial::off(), Arc::new(crate::token::FileToken::fixed(None)));
        let r = none.reach().await.unwrap();
        assert_eq!((r.bearer, r.withheld), (None, false), "nothing to withhold");
    }

    #[test]
    fn what_a_failure_says_about_the_port() {
        assert_eq!(classify("error sending request: invalid peer certificate: UnknownIssuer"), Failed::Untrusted);
        assert_eq!(classify("invalid peer certificate: NotValidForName"), Failed::Untrusted);
        assert_eq!(classify("tcp connect error: Connection refused (os error 111)"), Failed::Down);
        assert_eq!(classify("operation timed out"), Failed::Down);
        assert_eq!(classify("received corrupt message of type InvalidContentType"), Failed::Other);
        assert_eq!(classify("tls handshake eof"), Failed::Other);
    }

    #[tokio::test]
    async fn with_no_files_a_base_is_used_as_given() {
        let r = Dial::off().route("http://127.0.0.1:1/").await.unwrap();
        assert_eq!(r.base, "http://127.0.0.1:1");
        let r = Dial::new(TlsFiles::default(), true).route("http://127.0.0.1:1").await.unwrap();
        assert_eq!(r.base, "http://127.0.0.1:1", "require means nothing with nothing to present");
    }

    #[tokio::test]
    async fn a_pair_not_there_is_said_and_nothing_is_read() {
        let files = TlsFiles { ca: Some("/nonexistent/ca.crt".into()), cert: None, key: None };
        let e = match Dial::new(files, false).route("http://127.0.0.1:1").await {
            Err(e) => e,
            Ok(_) => panic!("read with no CA"),
        };
        assert!(e.contains("/nonexistent/ca.crt"), "{e}");
    }

    #[test]
    fn an_answer_is_kept_for_a_while() {
        let files = TlsFiles { ca: Some("/nonexistent/ca.crt".into()), cert: None, key: None };
        let d = Dial::new(files, false);
        assert_eq!(d.remembered("h:1"), None);
        d.remember("h:1", false);
        assert_eq!(d.remembered("h:1"), Some(false));
        d.seen.write().unwrap().insert("h:1".into(), (false, Instant::now() - REMEMBER));
        assert_eq!(d.remembered("h:1"), None, "so a daemon that moves to TLS is asked again");
    }
}
