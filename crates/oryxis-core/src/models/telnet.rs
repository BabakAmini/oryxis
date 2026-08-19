//! Per-host Telnet options, carried by a `Connection` whose `protocol`
//! is `Telnet`. Today that is the TLS pair (RFC 2941-era `telnets`,
//! conventionally port 992), kept in one struct rather than two flat
//! columns so the next Telnet-only option lands without another
//! migration.
//!
//! `None` on the connection means plain Telnet, which is what every
//! payload written before this existed carries, so old vaults, sync
//! peers and portable exports keep meaning exactly what they meant.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TelnetOptions {
    /// Wrap the TCP stream in TLS before any option negotiation. The
    /// handshake runs first and the Telnet NVT rides inside it, which
    /// is how `telnets` (992) works: there is no in-band STARTTLS
    /// upgrade to negotiate.
    #[serde(default)]
    pub tls: bool,
    /// Accept a server certificate the trust store rejects (expired,
    /// self-signed, wrong name). Off by default and per host on
    /// purpose: appliances ship self-signed certs, but a global escape
    /// would silently downgrade every other host to no verification at
    /// all. Meaningless while `tls` is false.
    #[serde(default)]
    pub tls_insecure: bool,
}

impl TelnetOptions {
    /// Whether this carries anything worth storing. An all-default
    /// value is written back as `None` so a host the user merely
    /// opened keeps a NULL column instead of growing a JSON blob.
    pub fn is_default(self) -> bool {
        self == TelnetOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use super::TelnetOptions;

    #[test]
    fn legacy_payload_defaults_to_plain_telnet() {
        // A payload written before the field existed has no keys at
        // all: it must decode as plain Telnet with verification on,
        // never as "TLS with verification skipped".
        let de: TelnetOptions = serde_json::from_str("{}").expect("empty object decodes");
        assert!(!de.tls);
        assert!(!de.tls_insecure);
        assert!(de.is_default());
    }

    #[test]
    fn insecure_alone_is_not_default() {
        let opts = TelnetOptions { tls: false, tls_insecure: true };
        assert!(!opts.is_default(), "a stored escape must survive a save");
    }
}
