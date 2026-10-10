//! The console's listener: plain HTTP, or TLS with client certificates as
//! credentials (#48, #127).
//!
//! stormcos's rule (owner, 2026-09-25): every API on a node is TLS with a
//! stormcert certificate, and nothing answers anonymously except health.
//! With `[api] tls_cert_file`/`tls_key_file` :9094 serves TLS. Plain HTTP
//! still reaches the same port — each connection's first byte says which
//! it is (a TLS record starts 0x16) — and is answered by
//! [`crate::server`]'s transport gate: health only, a redirect for a
//! browser, a refusal for anything else. The pattern is stormcluster#5's.
//!
//! **Client certificates** (#127). stormcentral reads a node's logs with
//! the client certificate forge enrolled it with (stormcentral#416), so a
//! certificate issued by `[api] client_ca_file` is a credential, mapped to
//! a role by `[[api.client_roles]]` on its subject's CN or O.
//!
//! The handshake **asks for** a certificate and accepts any — the client
//! still has to prove it holds the key — and the chain is checked against
//! the CA after the handshake. So a certificate from another CA, an expired
//! one, or none at all is an anonymous request that gets the console's 401,
//! saying so, instead of a TLS alert a caller can only guess at.
//!
//! **Files are followed.** stormcert renews the serving pair in place and
//! forge's CA and CRL can change under a running console, so each accept
//! compares the files' modification times and rebuilds when one moved. A
//! file that is missing or bad is logged once per change: a missing pair
//! refuses TLS connections until it is there; a missing CA turns client
//! certificates off.
//!
//! Handshakes run on their own tasks, so one slow client cannot hold up
//! the accept loop.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, CertificateRevocationListDer, PrivateKeyDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::WebPkiClientVerifier;
use rustls::{DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::config::ClientRole;

/// How long a client gets to say anything, and then to finish a handshake.
const HANDSHAKE: Duration = Duration::from_secs(10);

/// What the console knows about the connection a request came on.
#[derive(Clone, Debug, Default)]
pub struct Peer {
    /// The port serves TLS (so a plain request is on the wrong door).
    pub tls_on: bool,
    /// This connection is TLS.
    pub tls: bool,
    /// The certificate the client presented, if it presented one.
    pub cert: Option<CertIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertIdentity {
    pub cn: String,
    pub orgs: Vec<String>,
    /// `None` when the chain verified against the client CA, else why not.
    pub refused: Option<String>,
    /// The role a rule gives it; only for a verified certificate.
    pub role: Option<String>,
}

impl Peer {
    /// The role this connection's certificate carries, when it verified and
    /// a rule matched.
    pub fn cert_role(&self) -> Option<(&str, &str)> {
        let c = self.cert.as_ref()?;
        if c.refused.is_some() {
            return None;
        }
        Some((c.cn.as_str(), c.role.as_deref()?))
    }
}

impl Connected<IncomingStream<'_, Listener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, Listener>) -> Self {
        stream.io().peer.clone()
    }
}

/// The TLS files, as configured.
#[derive(Clone, Debug)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub client_ca: Option<PathBuf>,
    pub client_crl: Option<PathBuf>,
    pub roles: Vec<ClientRole>,
}

struct Built {
    stamp: Vec<Option<SystemTime>>,
    server: Option<Arc<ServerConfig>>,
    /// Verifies a presented chain against the client CA, after the
    /// handshake.
    clients: Option<Arc<dyn ClientCertVerifier>>,
    said: Vec<String>,
}

/// The current TLS state, rebuilt when a file changes.
pub struct Tls {
    files: TlsConfig,
    built: Mutex<Option<Built>>,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn read(p: &PathBuf, what: &str) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("[api] {what} {}: {e}", p.display()))
}

impl Tls {
    pub fn new(files: TlsConfig) -> Self {
        Self { files, built: Mutex::new(None) }
    }

    fn stamp(&self) -> Vec<Option<SystemTime>> {
        let f = &self.files;
        [Some(&f.cert), Some(&f.key), f.client_ca.as_ref(), f.client_crl.as_ref()]
            .into_iter()
            .flatten()
            .map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
            .collect()
    }

    /// The client CA's verifier, or why there is none.
    fn clients(&self) -> Result<Option<Arc<dyn ClientCertVerifier>>, String> {
        let Some(ca) = &self.files.client_ca else { return Ok(None) };
        let pem = read(ca, "client_ca_file")?;
        let mut roots = RootCertStore::empty();
        for c in CertificateDer::pem_slice_iter(&pem) {
            let c = c.map_err(|e| format!("[api] client_ca_file {}: not PEM: {e}", ca.display()))?;
            roots.add(c).map_err(|e| format!("[api] client_ca_file {}: {e}", ca.display()))?;
        }
        if roots.is_empty() {
            return Err(format!("[api] client_ca_file {}: no certificate in it", ca.display()));
        }
        let mut b = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider());
        if let Some(crl) = &self.files.client_crl {
            match read(crl, "client_crl_file") {
                Ok(pem) => {
                    let crls: Vec<CertificateRevocationListDer<'static>> =
                        CertificateRevocationListDer::pem_slice_iter(&pem)
                            .collect::<Result<_, _>>()
                            .map_err(|e| format!("[api] client_crl_file {}: not PEM: {e}", crl.display()))?;
                    b = b.with_crls(crls);
                }
                // No list yet is no revocation, not no certificates.
                Err(e) => warn!("{e}: revoked certificates are not refused until it is there"),
            }
        }
        b.build().map(|v| Some(v as Arc<dyn ClientCertVerifier>)).map_err(|e| format!("[api] client_ca_file {}: {e}", ca.display()))
    }

    fn build(&self) -> Built {
        let stamp = self.stamp();
        let mut said = Vec::new();
        let clients = match self.clients() {
            Ok(c) => c,
            Err(e) => {
                said.push(format!("{e}: client certificates are not accepted until it is there"));
                None
            }
        };
        let server = match self.server_config(clients.as_ref()) {
            Ok(s) => Some(s),
            Err(e) => {
                said.push(format!("{e}: TLS connections are refused until it is there"));
                None
            }
        };
        Built { stamp, server, clients, said }
    }

    fn server_config(&self, clients: Option<&Arc<dyn ClientCertVerifier>>) -> Result<Arc<ServerConfig>, String> {
        let f = &self.files;
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&read(&f.cert, "tls_cert_file")?)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("[api] tls_cert_file {}: not PEM: {e}", f.cert.display()))?;
        if certs.is_empty() {
            return Err(format!("[api] tls_cert_file {}: no certificate in it", f.cert.display()));
        }
        let key = PrivateKeyDer::from_pem_slice(&read(&f.key, "tls_key_file")?)
            .map_err(|e| format!("[api] tls_key_file {}: {e}", f.key.display()))?;
        let b = ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?;
        let b = match clients {
            Some(v) => b.with_client_cert_verifier(Arc::new(AskAny(v.clone()))),
            None => b.with_no_client_auth(),
        };
        let mut cfg = b.with_single_cert(certs, key).map_err(|e| format!("[api] tls_cert_file/tls_key_file: {e}"))?;
        cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Arc::new(cfg))
    }

    /// The server config and client verifier for a connection arriving now.
    fn current(&self) -> (Option<Arc<ServerConfig>>, Option<Arc<dyn ClientCertVerifier>>) {
        let mut g = self.built.lock().unwrap_or_else(|e| e.into_inner());
        let stamp = self.stamp();
        if g.as_ref().is_none_or(|b| b.stamp != stamp) {
            let next = self.build();
            // Said once per change of the files, not once per connection.
            let before = g.as_ref().map(|b| b.said.clone()).unwrap_or_default();
            for s in &next.said {
                if !before.contains(s) {
                    warn!("{s}");
                }
            }
            if g.is_some() && next.server.is_some() {
                info!("[api] TLS files changed: new connections use them");
            }
            *g = Some(next);
        }
        let b = g.as_ref().expect("built above");
        (b.server.clone(), b.clients.clone())
    }

    /// What a verified-or-not chain says about who is on the other end.
    fn identify(&self, chain: &[CertificateDer<'_>], clients: Option<&Arc<dyn ClientCertVerifier>>) -> CertIdentity {
        let (cn, orgs) = subject(&chain[0]).unwrap_or_default();
        let refused = match clients {
            None => Some("no client CA is configured".to_string()),
            Some(v) => v.verify_client_cert(&chain[0], &chain[1..], UnixTime::now()).err().map(|e| e.to_string()),
        };
        let role = if refused.is_none() { role_for(&self.files.roles, &cn, &orgs) } else { None };
        CertIdentity { cn, orgs, refused, role }
    }
}

/// The role the first matching rule gives a subject.
pub fn role_for(rules: &[ClientRole], cn: &str, orgs: &[String]) -> Option<String> {
    rules
        .iter()
        .find(|r| r.cn.as_deref() == Some(cn) || r.o.as_ref().is_some_and(|o| orgs.contains(o)))
        .map(|r| r.role.clone())
}

/// Asks every client for a certificate and takes whatever it presents; the
/// signature over the handshake is still checked, so a client presenting a
/// certificate holds its key. Whether the certificate is any good is
/// decided after the handshake (see the module docs).
#[derive(Debug)]
struct AskAny(Arc<dyn ClientCertVerifier>);

impl ClientCertVerifier for AskAny {
    fn offer_client_auth(&self) -> bool {
        true
    }
    fn client_auth_mandatory(&self) -> bool {
        false
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.0.root_hint_subjects()
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

// ---- the subject of a certificate, read from its DER

/// One TLV: its tag, its contents, and what follows it.
fn tlv(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = b.split_first()?;
    let (&l0, rest) = rest.split_first()?;
    let (len, rest) = if l0 < 0x80 {
        (l0 as usize, rest)
    } else {
        let n = (l0 & 0x7f) as usize;
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        (rest[..n].iter().fold(0usize, |a, &b| (a << 8) | b as usize), &rest[n..])
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

const OID_CN: &[u8] = &[0x55, 0x04, 0x03];
const OID_O: &[u8] = &[0x55, 0x04, 0x0a];

/// A certificate's subject CN and its O values. A certificate whose subject
/// cannot be read is `None`, and matches no rule.
pub fn subject(der: &[u8]) -> Option<(String, Vec<String>)> {
    let (_, cert, _) = tlv(der)?;
    let (_, mut tbs, _) = tlv(cert)?;
    // version [0], serial, signature algorithm, issuer, validity, subject.
    if tbs.first() == Some(&0xa0) {
        tbs = tlv(tbs)?.2;
    }
    for _ in 0..4 {
        tbs = tlv(tbs)?.2;
    }
    let (tag, mut names, _) = tlv(tbs)?;
    if tag != 0x30 {
        return None;
    }
    let (mut cn, mut orgs) = (String::new(), Vec::new());
    while !names.is_empty() {
        let (_, set, rest) = tlv(names)?;
        names = rest;
        let mut atvs = set;
        while !atvs.is_empty() {
            let (_, atv, rest) = tlv(atvs)?;
            atvs = rest;
            let (_, oid, value) = tlv(atv)?;
            let (vtag, v, _) = tlv(value)?;
            // UTF8String, PrintableString, IA5String, T61String.
            if !matches!(vtag, 0x0c | 0x13 | 0x16 | 0x14) {
                continue;
            }
            let v = String::from_utf8_lossy(v).into_owned();
            if oid == OID_CN && cn.is_empty() {
                cn = v;
            } else if oid == OID_O {
                orgs.push(v);
            }
        }
    }
    Some((cn, orgs))
}

// ---- the connection

enum Io {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

/// One accepted connection, and what is known about who is on it.
pub struct Conn {
    io: Io,
    pub peer: Peer,
}

impl AsyncRead for Conn {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Conn {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().io {
            Io::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Plain(s) => Pin::new(s).poll_flush(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut self.get_mut().io {
            Io::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match &mut self.get_mut().io {
            Io::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Io::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match &self.io {
            Io::Plain(s) => s.is_write_vectored(),
            Io::Tls(s) => s.is_write_vectored(),
        }
    }
}

/// The listener `axum::serve` takes: connections arrive here once they are
/// plain or have finished their handshake.
pub struct Listener {
    rx: mpsc::Receiver<(Conn, SocketAddr)>,
    local: SocketAddr,
}

impl axum::serve::Listener for Listener {
    type Io = Conn;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Conn, SocketAddr) {
        match self.rx.recv().await {
            Some(c) => c,
            // The accept loop is gone: nothing more will arrive.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }
}

/// Listen on `tcp`, TLS when `tls` is given.
pub fn listen(tcp: TcpListener, tls: Option<Arc<Tls>>) -> io::Result<Listener> {
    let local = tcp.local_addr()?;
    let (tx, rx) = mpsc::channel(128);
    tokio::spawn(async move {
        loop {
            let (stream, addr) = match tcp.accept().await {
                Ok(c) => c,
                Err(e) => {
                    // Out of descriptors, mostly: wait, then keep serving.
                    warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let (tx, tls) = (tx.clone(), tls.clone());
            tokio::spawn(async move {
                if let Some(conn) = open(stream, tls).await {
                    let _ = tx.send((conn, addr)).await;
                }
            });
        }
    });
    Ok(Listener { rx, local })
}

/// A connection, ready for HTTP: plain as it came, or after its handshake.
async fn open(stream: TcpStream, tls: Option<Arc<Tls>>) -> Option<Conn> {
    let Some(tls) = tls else {
        return Some(Conn { io: Io::Plain(stream), peer: Peer::default() });
    };
    // The first byte of a TLS connection is a handshake record's (0x16).
    let mut first = [0u8; 1];
    let n = tokio::time::timeout(HANDSHAKE, stream.peek(&mut first)).await.ok()?.ok()?;
    if n == 0 || first[0] != 0x16 {
        return Some(Conn { io: Io::Plain(stream), peer: Peer { tls_on: true, tls: false, cert: None } });
    }
    let (server, clients) = tls.current();
    let server = server?;
    let s = tokio::time::timeout(HANDSHAKE, tokio_rustls::TlsAcceptor::from(server).accept(stream)).await.ok()?.ok()?;
    let cert = s
        .get_ref()
        .1
        .peer_certificates()
        .filter(|c| !c.is_empty())
        .map(|chain| tls.identify(chain, clients.as_ref()));
    Some(Conn { io: Io::Tls(Box::new(s)), peer: Peer { tls_on: true, tls: true, cert } })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256
    /// -subj "/O=stormcos/O=forge/CN=stormcentral"`, as DER. No key: a
    /// certificate is public.
    fn der() -> Vec<u8> {
        include_bytes!("testdata/client.der").to_vec()
    }

    #[test]
    fn the_subject_is_read_from_the_certificate() {
        let (cn, orgs) = subject(&der()).unwrap();
        assert_eq!(cn, "stormcentral");
        assert_eq!(orgs, vec!["stormcos".to_string(), "forge".to_string()]);
    }

    #[test]
    fn a_truncated_certificate_has_no_subject() {
        let d = der();
        assert!(subject(&d[..40]).is_none());
        assert!(subject(&[]).is_none());
    }

    fn rule(cn: Option<&str>, o: Option<&str>, role: &str) -> ClientRole {
        ClientRole { cn: cn.map(String::from), o: o.map(String::from), role: role.into() }
    }

    #[test]
    fn the_first_matching_rule_gives_the_role() {
        let rules = vec![rule(Some("ops-laptop"), None, "admin"), rule(None, Some("forge"), "viewer"), rule(Some("stormcentral"), None, "admin")];
        let orgs = vec!["stormcos".to_string(), "forge".to_string()];
        assert_eq!(role_for(&rules, "stormcentral", &orgs).as_deref(), Some("viewer"));
        assert_eq!(role_for(&rules, "ops-laptop", &[]).as_deref(), Some("admin"));
        assert_eq!(role_for(&rules, "someone", &[]), None);
    }

    #[test]
    fn only_a_verified_mapped_certificate_has_a_role() {
        let ok = CertIdentity { cn: "stormcentral".into(), orgs: vec![], refused: None, role: Some("viewer".into()) };
        let peer = Peer { tls_on: true, tls: true, cert: Some(ok.clone()) };
        assert_eq!(peer.cert_role(), Some(("stormcentral", "viewer")));
        let refused = CertIdentity { refused: Some("UnknownIssuer".into()), ..ok.clone() };
        assert_eq!(Peer { cert: Some(refused), ..peer.clone() }.cert_role(), None);
        let unmapped = CertIdentity { role: None, ..ok };
        assert_eq!(Peer { cert: Some(unmapped), ..peer }.cert_role(), None);
        assert_eq!(Peer::default().cert_role(), None);
    }
}
