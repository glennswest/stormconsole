//! Two different addresses have been wearing the same string.
//!
//! The **dial address** is what this process connects to. The console runs
//! on the node, so it is almost always loopback — `https://127.0.0.1:6443`,
//! `http://127.0.0.1:9092` — and that is correct.
//!
//! The **viewer address** is what a browser on somebody's laptop could
//! use. It is never loopback, because loopback there is the laptop.
//!
//! Printing the first where the second belongs is issue #10: the card said
//! `http://127.0.0.1:9092`, which is a real address that resolves to
//! entirely the wrong machine. So a card says *where* an upstream is in
//! words a reader can act on, and anything meant to be clicked goes
//! through `/api/plugins/{name}/proxy/…` on the console's own origin.

/// Host and port as written in a base URL. Neither is validated — this is
/// for describing a configured string, not for dialling it.
pub fn host_port(url: &str) -> (&str, Option<&str>) {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // IPv6 literals are bracketed: [::1]:9092.
    if let Some(end) = authority.strip_prefix('[').and_then(|_| authority.find(']')) {
        let host = &authority[1..end];
        let port = authority[end + 1..].strip_prefix(':').filter(|p| !p.is_empty());
        return (host, port);
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (authority, None),
    }
}

/// Is this upstream on the machine the console is running on?
pub fn is_loopback(url: &str) -> bool {
    let (host, _) = host_port(url);
    host == "127.0.0.1"
        || host == "localhost"
        || host == "::1"
        || host.starts_with("127.")
}

/// How a card should name an upstream. Node-local endpoints are described,
/// not addressed, because their address means nothing to a reader:
///
/// ```
/// # use console_core::upstream::describe;
/// assert_eq!(describe("http://127.0.0.1:9092"), "on this node :9092");
/// assert_eq!(describe("https://192.168.8.106:6443"), "192.168.8.106:6443");
/// ```
pub fn describe(url: &str) -> String {
    let (host, port) = host_port(url);
    match (is_loopback(url), port) {
        (true, Some(p)) => format!("on this node :{p}"),
        (true, None) => "on this node".to_string(),
        (false, Some(p)) => format!("{host}:{p}"),
        (false, None) => host.to_string(),
    }
}

/// One line naming a daemon and where it is, for a plugin's `detail`.
/// `what` is the daemon ("stormdrive", "rustkube"); `rest` is whatever the
/// plugin has to say about it.
pub fn detail(what: &str, url: &str, rest: &str) -> String {
    let where_ = describe(url);
    if rest.is_empty() {
        format!("{what} {where_}")
    } else {
        format!("{what} {where_} · {rest}")
    }
}

/// Is a service on this node simply **not started** (#60)?
///
/// stormcos carries services on every node and starts some only by role —
/// stormipmi on a single-node cluster (`roles=sno`), the datastore on the
/// control plane (`roles=sno,master`) — so a node that does not run one is
/// as designed, not broken. The tell is that nothing listens on the
/// service's port **and** nothing on the stormd that would supervise it.
/// Anything else — a refusal while its stormd is up, a timeout, an answer
/// that is an error — is a started service that is not answering, which is
/// a fault. Only a loopback URL can be judged: another machine's ports are
/// not this node's to read.
pub async fn not_started(url: &str, stormd_port: u16) -> bool {
    if !is_loopback(url) {
        return false;
    }
    let (host, port) = host_port(url);
    let port = match port.and_then(|p| p.parse::<u16>().ok()) {
        Some(p) => p,
        None if url.starts_with("https://") => 443,
        None => 80,
    };
    let host = if host == "localhost" { "127.0.0.1" } else { host };
    refused(host, port).await && refused(host, stormd_port).await
}

/// Nothing listens there: the connection is refused, not slow.
async fn refused(host: &str, port: u16) -> bool {
    let dial = tokio::net::TcpStream::connect((host, port));
    matches!(
        tokio::time::timeout(std::time::Duration::from_secs(1), dial).await,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port nothing listens on: bound, then dropped.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn not_started_is_nothing_on_the_port_and_nothing_supervising_it() {
        let (svc, stormd) = (free_port(), free_port());
        let url = format!("http://127.0.0.1:{svc}");
        assert!(not_started(&url, stormd).await, "neither listens: not started");

        // Its stormd is up, the service is not: started and silent, a fault.
        let sd = tokio::net::TcpListener::bind(("127.0.0.1", stormd)).await.unwrap();
        assert!(!not_started(&url, stormd).await);
        drop(sd);

        // The service listens: whatever it answers is its own story.
        let l = tokio::net::TcpListener::bind(("127.0.0.1", svc)).await.unwrap();
        assert!(!not_started(&url, stormd).await);
        drop(l);

        // Another machine is never judged from here.
        assert!(!not_started(&format!("http://192.0.2.1:{svc}"), stormd).await);
    }

    #[test]
    fn host_and_port_come_out_of_any_base_url() {
        assert_eq!(host_port("http://127.0.0.1:9092"), ("127.0.0.1", Some("9092")));
        assert_eq!(host_port("https://k.example:6443/"), ("k.example", Some("6443")));
        assert_eq!(host_port("http://[::1]:9092"), ("::1", Some("9092")));
        assert_eq!(host_port("http://registry.gt.lo"), ("registry.gt.lo", None));
        assert_eq!(host_port("192.168.8.106:9090"), ("192.168.8.106", Some("9090")));
    }

    #[test]
    fn loopback_is_never_offered_as_an_address() {
        for url in ["http://127.0.0.1:9092", "https://localhost:6443", "http://[::1]:5100", "http://127.0.1.1:1"] {
            assert!(is_loopback(url), "{url}");
            assert!(!describe(url).contains("127."), "{url} → {}", describe(url));
            assert!(describe(url).starts_with("on this node"), "{url}");
        }
    }

    #[test]
    fn a_real_address_is_kept_because_a_browser_can_use_it() {
        assert_eq!(describe("https://192.168.8.106:6443"), "192.168.8.106:6443");
        assert_eq!(describe("http://sptest.g8.lo:9092"), "sptest.g8.lo:9092");
    }

    #[test]
    fn detail_reads_as_a_sentence() {
        assert_eq!(detail("rustkube", "https://127.0.0.1:6443", ""), "rustkube on this node :6443");
        assert_eq!(
            detail("stormdrive", "http://127.0.0.1:9092", "1 drives"),
            "stormdrive on this node :9092 · 1 drives"
        );
    }
}
