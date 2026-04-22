//! Trusted-proxy gate for AppGW-terminated mTLS.
//!
//! When `CMTRACE_PEER_CERT_HEADER` is set, the api-server reads the client
//! certificate from an HTTP header forwarded by Azure Application Gateway
//! instead of performing in-process TLS termination. Because any client that
//! can reach the api-server could forge that header, we must only honour it
//! when the request arrives from a trusted reverse-proxy IP.
//!
//! [`is_trusted_proxy`] is the single gate: it checks whether a `SocketAddr`
//! falls within the operator-configured CIDR ([`crate::config::TlsConfig::
//! trusted_proxy_cidr`]). The [`crate::auth::DeviceIdentity`] extractor calls
//! this before reading the peer-cert header.

use std::net::IpAddr;

use ipnet::IpNet;

/// Returns `true` when `peer_addr` is contained within `cidr`.
///
/// Handles the IPv4-mapped-IPv6 representation that Linux stacks emit when
/// accepting connections on a dual-stack socket (e.g. `::ffff:10.224.0.1`
/// should match an IPv4 CIDR `10.224.0.0/16`).
pub fn is_trusted_proxy(peer_addr: IpAddr, cidr: &IpNet) -> bool {
    // Normalise IPv4-mapped-IPv6 to plain IPv4 so a `10.224.0.0/16` CIDR
    // matches both `10.224.0.1` and `::ffff:10.224.0.1`.
    let normalised = match peer_addr {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(peer_addr),
        other => other,
    };
    cidr.contains(&normalised)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::*;

    fn net(s: &str) -> IpNet {
        s.parse().unwrap()
    }

    #[test]
    fn ipv4_in_cidr() {
        assert!(is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(10, 224, 0, 5)),
            &net("10.224.0.0/16"),
        ));
    }

    #[test]
    fn ipv4_outside_cidr() {
        assert!(!is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
            &net("10.224.0.0/16"),
        ));
    }

    #[test]
    fn ipv4_mapped_ipv6_matches_ipv4_cidr() {
        // ::ffff:10.224.0.5 is the dual-stack form of 10.224.0.5.
        let mapped: IpAddr = IpAddr::V6("::ffff:10.224.0.5".parse::<Ipv6Addr>().unwrap());
        assert!(is_trusted_proxy(mapped, &net("10.224.0.0/16")));
    }

    #[test]
    fn single_host_cidr() {
        assert!(is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            &net("10.0.0.1/32"),
        ));
        assert!(!is_trusted_proxy(
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            &net("10.0.0.1/32"),
        ));
    }
}
