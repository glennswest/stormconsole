//! The multicast receiver: join the stormcast group, parse each datagram,
//! store it and fan it out to live followers. SO_REUSEADDR so the console
//! can share the group with any other listener on the host.
//!
//! The store decides what reaches followers. A line that is repeating
//! thousands of times a second is one entry with a rising count, and only
//! its throttled updates go out — otherwise the flood is a flood on the
//! wire and in every open viewer, which is how a failing node made the log
//! view unusable.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::parse::parse;
use crate::store::{Store, StoredEvent};

pub fn parse_group(group: &str) -> Result<(Ipv4Addr, u16), String> {
    let addr: SocketAddr = group.parse().map_err(|e| format!("bad group {group}: {e}"))?;
    match addr {
        SocketAddr::V4(v4) => Ok((*v4.ip(), v4.port())),
        SocketAddr::V6(_) => Err("multicast group must be IPv4".to_string()),
    }
}

fn open_socket(ip: Ipv4Addr, port: u16) -> std::io::Result<UdpSocket> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)).into())?;
    sock.join_multicast_v4(&ip, &Ipv4Addr::UNSPECIFIED)?;
    sock.set_nonblocking(true)?;
    UdpSocket::from_std(sock.into())
}

/// Receive until shutdown. Errors surface through the returned Result so
/// the plugin can show them as component health.
pub async fn run(
    group: &str,
    store: Arc<Store>,
    tail: broadcast::Sender<StoredEvent>,
    shutdown: CancellationToken,
) -> Result<(), String> {
    let (ip, port) = parse_group(group)?;
    let sock = open_socket(ip, port).map_err(|e| format!("join {group}: {e}"))?;
    info!(group, "fleet log collector listening");
    let failures = std::sync::Mutex::new(0u64);
    let mut buf = vec![0u8; 65536];
    loop {
        let (len, src) = tokio::select! {
            r = sock.recv_from(&mut buf) => match r {
                Ok(x) => x,
                Err(e) => {
                    warn!(error = %e, "recv failed");
                    continue;
                }
            },
            _ = shutdown.cancelled() => return Ok(()),
        };
        let line = String::from_utf8_lossy(&buf[..len]);
        let now = chrono::Utc::now();
        let event = parse(&line, &src.ip().to_string(), || {
            now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        });
        match store.insert(&event, now.timestamp_millis().max(0) as u64) {
            Ok(inserted) => {
                if inserted.notify {
                    let _ = tail.send(inserted.event);
                }
            }
            // A store that cannot take a line must not become a line: the
            // console's own warnings go out over this same group, so logging
            // every failure is how one broken ring floods the whole fleet.
            // The plugin already reports the fault as component health.
            Err(e) => {
                let mut seen = failures.lock().unwrap();
                *seen += 1;
                if *seen == 1 || *seen % 10_000 == 0 {
                    warn!(error = %e, failures = *seen, "store insert failed");
                }
            }
        }
    }
}
