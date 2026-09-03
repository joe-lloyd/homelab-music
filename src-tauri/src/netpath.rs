// How we reach home right now: straight over the LAN, or through the tunnel.
//
// At home the LAN is gigabit and the tunnel is pointless overhead. Away, the
// tunnel is the only way in. Measured throughput over wg0 after the server
// moved to pi-server is about 7.9 Mbit/s, which fits a FLAC stream plus the
// player's ~20s prefetch but not much else -- so taking the tunnel when we
// did not have to would be a real, audible downgrade, not just untidy.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpStream;

/// pi-server on the LAN. Caddy terminates TLS here.
pub const HOME_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 2, 23)), 443);

/// The vhost Caddy routes on. Sent as SNI and Host regardless of which path we
/// take, because the certificate is issued for this name and nothing else.
pub const HOME_HOST: &str = "music.home.arpa";

/// Short on purpose. This runs at startup and on every network change, and a
/// slow answer here is a slow app launch. If the LAN is so degraded that
/// pi-server cannot answer a SYN in this long, the tunnel is the better path
/// anyway.
const PROBE_TIMEOUT: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// Direct to 192.168.2.23 -- we are on the home LAN.
    Lan,
    /// Through the userspace WireGuard tunnel to vpn.jia-lab.cc.
    Tunnel,
}

impl Path {
    pub fn label(self) -> &'static str {
        match self {
            Path::Lan => "at home",
            Path::Tunnel => "tunnelled",
        }
    }
}

/// Decide which path to use.
///
/// Deliberately a TCP connect rather than an ICMP ping or a DNS lookup: DNS
/// for music.home.arpa resolves through AdGuard on pi-server, so a name that
/// resolves proves nothing about whether we can reach the box, and a stale
/// resolver entry would strand us on the wrong path. Opening the socket we
/// actually intend to use is the only probe that cannot lie.
pub async fn detect() -> Path {
    match tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(HOME_ADDR)).await {
        Ok(Ok(_stream)) => Path::Lan,
        // Refused, unreachable, or timed out -- all mean "not on the LAN".
        _ => Path::Tunnel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unroutable_address_is_never_reported_as_lan() {
        // 192.0.2.0/24 is TEST-NET-1: guaranteed not to be a live host.
        let dead = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), 443);
        let reachable = tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(dead))
            .await
            .is_ok_and(|r| r.is_ok());
        assert!(!reachable, "TEST-NET-1 must not be reachable");
    }
}
