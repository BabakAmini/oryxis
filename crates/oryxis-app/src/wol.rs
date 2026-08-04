//! Wake-on-LAN: MAC parsing/normalization and the magic packet.
//!
//! The packet is 6 bytes of 0xFF followed by the target MAC repeated
//! 16 times (AMD's original spec), sent as a UDP broadcast to port 9
//! (discard). No crate needed: `std::net::UdpSocket` with
//! `SO_BROADCAST` covers it on every OS we ship.

use std::net::UdpSocket;

/// Parse a user-typed MAC address. Accepts the three widespread
/// notations: colon / hyphen separated pairs ("aa:bb:cc:dd:ee:ff",
/// "AA-BB-CC-DD-EE-FF"), Cisco dotted quads ("aabb.ccdd.eeff"), and
/// bare 12-digit hex. Returns `None` for anything else, including
/// mixed separators with wrong grouping.
pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let s = s.trim();
    let hex: String = s
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.'))
        .collect();
    if hex.len() != 12 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    // Whatever separators appeared must have formed a known grouping;
    // "aa:bbcc:ddeeff" is a typo, not a MAC.
    let sep_count = s.len() - hex.len();
    let valid_shape = match sep_count {
        0 => true,
        2 => s.split('.').count() == 3 && s.split('.').all(|g| g.len() == 4),
        5 => {
            let sep = if s.contains(':') { ':' } else { '-' };
            !(s.contains(':') && s.contains('-'))
                && s.split(sep).count() == 6
                && s.split(sep).all(|g| g.len() == 2)
        }
        _ => false,
    };
    if !valid_shape {
        return None;
    }
    let mut mac = [0u8; 6];
    for (i, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

/// Canonical display/storage form: uppercase colon-separated pairs.
pub fn format_mac(mac: [u8; 6]) -> String {
    mac.map(|b| format!("{b:02X}")).join(":")
}

/// Build the 102-byte magic packet for `mac`.
fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut pkt = [0xFFu8; 102];
    for rep in 0..16 {
        pkt[6 + rep * 6..12 + rep * 6].copy_from_slice(&mac);
    }
    pkt
}

/// Send the magic packet as a limited broadcast (255.255.255.255:9).
/// WoL is a same-segment protocol: the target is off, so it has no IP
/// and cannot be routed to; the broadcast reaches every NIC on the
/// local link and only the owner of `mac` wakes.
pub fn send(mac: [u8; 6]) -> std::io::Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))?;
    socket.set_broadcast(true)?;
    socket.send_to(&magic_packet(mac), ("255.255.255.255", 9))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_common_notations() {
        let expected = [0xAA, 0xBB, 0xCC, 0x00, 0x11, 0xF2];
        for s in [
            "AA:BB:CC:00:11:F2",
            "aa:bb:cc:00:11:f2",
            "AA-BB-CC-00-11-F2",
            "aabb.cc00.11f2",
            "AABBCC0011F2",
            "  aa:bb:cc:00:11:f2  ",
        ] {
            assert_eq!(parse_mac(s), Some(expected), "notation: {s}");
        }
    }

    #[test]
    fn rejects_malformed_input() {
        for s in [
            "",
            "AA:BB:CC:00:11",          // too short
            "AA:BB:CC:00:11:F2:33",    // too long
            "AA:BB:CC:00:11:G2",       // non-hex
            "aa:bbcc:ddeeff",          // wrong grouping
            "aa:bb-cc:00-11:f2",       // mixed separators
            "aabb.ccdd.ee.ff",         // wrong dotted grouping
            "hello world",
        ] {
            assert_eq!(parse_mac(s), None, "should reject: {s}");
        }
    }

    #[test]
    fn canonical_form_roundtrips() {
        let mac = parse_mac("aabb.cc00.11f2").unwrap();
        assert_eq!(format_mac(mac), "AA:BB:CC:00:11:F2");
        assert_eq!(parse_mac(&format_mac(mac)), Some(mac));
    }

    #[test]
    fn magic_packet_layout() {
        let mac = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
        let pkt = magic_packet(mac);
        assert_eq!(pkt.len(), 102);
        assert!(pkt[..6].iter().all(|&b| b == 0xFF), "6-byte sync header");
        for rep in 0..16 {
            assert_eq!(&pkt[6 + rep * 6..12 + rep * 6], &mac, "repetition {rep}");
        }
    }
}
