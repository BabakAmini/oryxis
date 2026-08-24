//! Per-host mosh options, carried by a `Connection` whose `protocol` is
//! `Ssh`.
//!
//! mosh is not a protocol a host is reached BY, it is one a session is
//! carried ON, which is why this is options on an SSH host rather than
//! a seventh entry in the protocol picker. It has to be: the server
//! does not exist until an SSH session starts it, and the port and key
//! it answers with come back over that same channel. So a mosh host
//! needs the username, the key, the jump chain, the proxy and the
//! host-key policy the SSH side already has, and duplicating all of
//! them under a protocol of their own would buy nothing. Same shape as
//! Telnet-over-TLS being a toggle on the Telnet form.
//!
//! `None` on the connection means an ordinary SSH shell, which is what
//! every payload written before this existed carries, so old vaults,
//! sync peers and portable exports keep meaning exactly what they
//! meant.
//!
//! **Every field here travels**, and that is worth stating because two
//! of them become words in a command line. They are not the case
//! `strip_local_trust` exists for: what they run runs on the REMOTE
//! host, which is the host the session is opening anyway, so they are
//! the same class as `initial_command`. `ProxyType::Command` is gated
//! because it spawns a LOCAL process before any handshake, on the
//! machine the user is sitting at. Nothing here does that.
//!
//! They still reach a shell, so `oryxis_mosh::ServerCommand`
//! single-quotes the two that are single VALUES (`server_path` and
//! `port_range`): one arriving from a sync peer is a value nobody here
//! typed, and neither has any business becoming more than one word.
//! `command` is deliberately NOT quoted, because it is a command LINE
//! rather than a value: quoting `tmux new -A -s work` whole would hand
//! the host one impossible word. That puts it in exactly the class
//! `initial_command` is in, which is the paragraph above and not an
//! exception to it.

use serde::{Deserialize, Serialize};

/// What a host needs to be reached with mosh instead of a plain shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MoshOptions {
    /// Carry the session over mosh. Off means the SSH shell, which is
    /// what an untouched host does.
    #[serde(default)]
    pub enabled: bool,
    /// Where `mosh-server` lives on the host. Empty means find it on
    /// `PATH`, which is right almost everywhere; a path is what makes a
    /// host with it installed somewhere unusual work at all, and that
    /// is per host because it is a fact about the host.
    #[serde(default)]
    pub server_path: String,
    /// UDP ports the server may bind, in mosh's own `-p` spelling
    /// (`60000` or `60000:60010`). Empty lets it choose from its
    /// default range, which is what a host with nothing in the way
    /// wants; a range is what makes a host behind a firewall that only
    /// opens a few ports reachable.
    #[serde(default)]
    pub port_range: String,
    /// What to run instead of the login shell. Empty is the login
    /// shell. This is separate from the connection's own startup
    /// command: that one is TYPED at a shell once it is up, and this
    /// one REPLACES the shell, which is what survives a disconnect.
    #[serde(default)]
    pub command: String,
}

impl MoshOptions {
    /// Whether this carries anything worth storing. An all-default
    /// value is written back as `None` so a host the user merely
    /// opened keeps a NULL column instead of growing a JSON blob.
    pub fn is_default(&self) -> bool {
        self == &MoshOptions::default()
    }

    /// Whether a session on this host should be carried over mosh.
    ///
    /// Asks the option rather than its presence: a host whose mosh
    /// settings were filled in and then turned off keeps the settings,
    /// so turning it back on does not mean typing the server path
    /// again.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_host_is_not_a_mosh_host() {
        let options = MoshOptions::default();
        assert!(!options.is_enabled());
        assert!(options.is_default(), "and is not worth a column");
    }

    #[test]
    fn settings_survive_being_turned_off() {
        // The path was typed once. Turning mosh off is not a reason to
        // make the user find it again.
        let options = MoshOptions {
            enabled: false,
            server_path: "/opt/mosh/bin/mosh-server".into(),
            ..Default::default()
        };
        assert!(!options.is_enabled());
        assert!(!options.is_default(), "so it still has to be stored");
    }

    #[test]
    fn a_payload_from_before_this_existed_is_a_plain_ssh_host() {
        // Every field defaulted, which is what a peer that never heard
        // of mosh sends, and what an old vault row decodes to.
        let options: MoshOptions = serde_json::from_str("{}").expect("an empty object decodes");
        assert_eq!(options, MoshOptions::default());
        assert!(!options.is_enabled(), "silence is never an opt-in");
    }

    #[test]
    fn a_payload_from_a_newer_peer_keeps_what_it_understands() {
        // Unknown fields are ignored rather than refused, so a peer
        // that grows an option does not break this one.
        let options: MoshOptions =
            serde_json::from_str(r#"{"enabled":true,"predict":"always"}"#).expect("decodes");
        assert!(options.is_enabled());
    }

    #[test]
    fn it_round_trips() {
        let options = MoshOptions {
            enabled: true,
            server_path: "/usr/local/bin/mosh-server".into(),
            port_range: "60000:60010".into(),
            command: "tmux new -A -s main".into(),
        };
        let json = serde_json::to_string(&options).expect("encodes");
        let back: MoshOptions = serde_json::from_str(&json).expect("decodes");
        assert_eq!(back, options);
    }
}
