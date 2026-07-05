//! Small networking helpers shared by the dialing crates (SSH engine,
//! Telnet).

/// `host:port` in the form resolvers accept, bracketing bare IPv6
/// literals (`2001:db8::1` -> `[2001:db8::1]:22`). Hostnames, IPv4
/// literals and already-bracketed input pass through untouched. Without
/// this, an IPv6 address typed into the host field produces
/// `2001:db8::1:22`, which `ToSocketAddrs` rejects (or worse,
/// misparses), so the per-host IPv6 preference would be unusable with
/// the very literals it invites.
pub fn host_port(host: &str, port: u16) -> String {
    if !host.starts_with('[') && host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Keep the addresses `family` allows, preserving resolver order
/// (`Auto` keeps everything). Shared by the SSH engine and the Telnet
/// dial so the per-host IPv4/IPv6 preference behaves identically on
/// both. Pure, so it's testable without a live resolver.
pub fn filter_addrs(
    addrs: &[std::net::SocketAddr],
    family: crate::models::connection::AddressFamily,
) -> Vec<std::net::SocketAddr> {
    use crate::models::connection::AddressFamily;
    addrs
        .iter()
        .copied()
        .filter(|a| match family {
            AddressFamily::Auto => true,
            AddressFamily::V4 => a.is_ipv4(),
            AddressFamily::V6 => a.is_ipv6(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::connection::AddressFamily;

    #[test]
    fn filter_addrs_honors_the_family_preference() {
        let v4: std::net::SocketAddr = "192.0.2.1:22".parse().unwrap();
        let v6: std::net::SocketAddr = "[2001:db8::1]:22".parse().unwrap();
        let both = vec![v6, v4];
        assert_eq!(
            filter_addrs(&both, AddressFamily::Auto),
            both,
            "Auto keeps resolver order untouched"
        );
        assert_eq!(filter_addrs(&both, AddressFamily::V4), vec![v4]);
        assert_eq!(filter_addrs(&both, AddressFamily::V6), vec![v6]);
        assert!(
            filter_addrs(&[v4], AddressFamily::V6).is_empty(),
            "a v4-only name under a v6 preference yields nothing (honest failure upstream)"
        );
    }

    #[test]
    fn ipv6_literals_get_bracketed() {
        assert_eq!(host_port("2001:db8::1", 22), "[2001:db8::1]:22");
        assert_eq!(host_port("::1", 2222), "[::1]:2222");
    }

    #[test]
    fn hostnames_and_ipv4_pass_through() {
        assert_eq!(host_port("example.com", 22), "example.com:22");
        assert_eq!(host_port("192.0.2.7", 23), "192.0.2.7:23");
    }

    #[test]
    fn already_bracketed_input_is_not_double_wrapped() {
        assert_eq!(host_port("[2001:db8::1]", 22), "[2001:db8::1]:22");
    }
}
