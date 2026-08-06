//! Per-host terminal appearance overrides: how translucent the terminal
//! is and which picture sits behind the grid.
//!
//! Every field is optional and `None` means "inherit", the same contract
//! as the per-host terminal theme next to it: a host that was never
//! touched carries no appearance at all and follows the global settings,
//! so changing the global still moves every host that did not opt out.
//!
//! The picture is stored as a path, not as bytes. A vault that carried
//! wallpapers would grow without bound and sync would ship megabytes per
//! host; a path that no longer resolves simply falls back to the plain
//! background, which is the same failure a moved file has in every other
//! terminal.

use serde::{Deserialize, Serialize};

/// Per-host overrides for the terminal's backdrop. Absent (`None` on the
/// connection) is the normal state and is byte-identical to the payloads
/// that existed before this type.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TerminalAppearance {
    /// Background opacity in percent (100 = opaque). `None` inherits the
    /// global `terminal_opacity` setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<u8>,
    /// Absolute path to the picture. `Some("")` is the explicit "no
    /// picture on this host" that overrides a global one; `None`
    /// inherits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// How the picture is laid into the pane
    /// (`cover` / `contain` / `stretch` / `center` / `tile`). Kept as a
    /// string so an unknown value from a newer build degrades to the
    /// default fit instead of failing the whole connection payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<String>,
    /// How far the picture is faded towards the background colour, in
    /// percent (0 = untouched, 100 = invisible).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<u8>,
}

impl TerminalAppearance {
    /// True when nothing is actually overridden. Used to store `None`
    /// instead of an empty object, so a host that had its overrides
    /// cleared goes back to being byte-identical to one that never had
    /// any.
    pub fn is_empty(&self) -> bool {
        self.opacity.is_none()
            && self.image.is_none()
            && self.fit.is_none()
            && self.dim.is_none()
    }

    /// Drop the value if nothing is set, keeping the "absent means
    /// inherit" invariant at every write site.
    pub fn into_option(self) -> Option<Self> {
        if self.is_empty() { None } else { Some(self) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_overrides_serialize_to_an_empty_object() {
        // The skip_serializing_if pair is what keeps a host that only
        // overrides one field from writing three nulls into the vault
        // (and shipping them over sync).
        let only_dim = TerminalAppearance {
            dim: Some(40),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&only_dim).unwrap(),
            r#"{"dim":40}"#,
        );
    }

    #[test]
    fn an_empty_appearance_is_dropped_rather_than_stored() {
        assert!(TerminalAppearance::default().into_option().is_none());
        assert!(
            TerminalAppearance {
                opacity: Some(80),
                ..Default::default()
            }
            .into_option()
            .is_some()
        );
    }

    #[test]
    fn a_payload_from_a_newer_build_still_loads() {
        // Unknown fields are ignored and a fit this build does not know
        // still parses: the host keeps its picture and renders it with
        // the default fit rather than failing to load.
        let json = r#"{"opacity":70,"fit":"parallax","future_knob":true}"#;
        let parsed: TerminalAppearance = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.opacity, Some(70));
        assert_eq!(parsed.fit.as_deref(), Some("parallax"));
    }
}
