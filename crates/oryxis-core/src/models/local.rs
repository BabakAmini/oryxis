//! Per-host local-shell settings, carried by a `Connection` whose
//! `protocol` is `Local`. A local host is a saved way to open a shell
//! on THIS machine: the label, folder and startup command of a session
//! the user runs often ("Claude", opening the local terminal straight
//! into `claude`).
//!
//! What to execute is NOT stored here. The curated local-terminal list
//! (Settings > Terminal) already owns program + arguments, and it is
//! machine-local on purpose because an executable path only means
//! something on the machine that has it. A local host REFERENCES one of
//! those entries, so the two never disagree about how to spawn a shell.
//!
//! The reference is an id plus the label it had when saved. The id is
//! authoritative on the machine that wrote it; the label is what lets a
//! synced host still resolve on a second machine, where the same shell
//! exists under a freshly generated id. Neither resolving is a real
//! state (the machine simply lacks that shell), reported at connect
//! time rather than guessed around.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LocalConfig {
    /// Curated local-terminal entry to spawn. `None` means "the user's
    /// default shell", the same thing the local-terminal picker opens
    /// when nothing is chosen.
    #[serde(default)]
    pub terminal_id: Option<Uuid>,
    /// That entry's label when the host was saved, used to re-resolve
    /// the shell on a machine where the id doesn't exist (a synced or
    /// imported host). Never authoritative over the id.
    #[serde(default)]
    pub terminal_label: Option<String>,
    /// Directory the shell starts in. `None` (and the empty string)
    /// mean the process default, i.e. whatever the shell picks. `~` is
    /// expanded at spawn time, so the value stays portable between a
    /// Unix and a Windows machine that both understand it.
    #[serde(default)]
    pub cwd: Option<String>,
}

impl LocalConfig {
    /// Whether this carries anything worth storing, so an untouched
    /// local host keeps a NULL column instead of an empty JSON blob.
    pub fn is_default(&self) -> bool {
        self == &LocalConfig::default()
    }

    /// The stored working directory, minus the empty string (which the
    /// editor writes for a cleared field and which must mean the same
    /// as never having set one).
    pub fn effective_cwd(&self) -> Option<&str> {
        self.cwd.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::LocalConfig;

    #[test]
    fn legacy_payload_decodes_to_defaults() {
        let de: LocalConfig = serde_json::from_str("{}").expect("empty object decodes");
        assert!(de.is_default());
        assert_eq!(de.effective_cwd(), None);
    }

    #[test]
    fn blank_cwd_reads_as_unset() {
        // The editor writes "" for a cleared field; a spawn must not
        // try to chdir into nothing.
        let cfg = LocalConfig { cwd: Some("   ".to_string()), ..LocalConfig::default() };
        assert_eq!(cfg.effective_cwd(), None);
    }

    #[test]
    fn cwd_is_trimmed() {
        let cfg = LocalConfig { cwd: Some(" ~/work ".to_string()), ..LocalConfig::default() };
        assert_eq!(cfg.effective_cwd(), Some("~/work"));
    }
}
