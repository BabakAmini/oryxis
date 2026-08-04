//! `oryxis://` deep links: the OS-registered URL scheme (issue #118's
//! theme sharing follow-up) and its in-app routing.
//!
//! Two halves:
//!
//! - **Parsing** ([`parse`]): pure, size-capped, and strict. A link
//!   that doesn't parse is dropped (with a log line), never "best
//!   effort" handled: these URLs arrive from browsers, i.e. from any
//!   web page the user clicked.
//! - **Routing** ([`Oryxis::handle_deep_link`]): every route lands on
//!   an EXISTING confirm surface with the payload prefilled, never on
//!   a side effect. A theme link opens the import panel (Apply ->
//!   editor -> Save stays the user's call); a pairing link opens
//!   Settings > Sync with the join field filled. Nothing installs or
//!   joins on its own, so a hostile link can at worst open a screen.
//!
//! Delivery paths into this module:
//!
//! - Cold start: the OS hands the URL on argv; `main.rs` stashes it in
//!   [`crate::app::PENDING_DEEP_LINK`] and boot routes it (post-unlock
//!   via `pending_deep_link`, mirroring `--connect`).
//! - Running instance: the OS spawns a second process, which forwards
//!   the URL through `tray_ipc::write_deeplink` and exits; the
//!   `deep_link_stream` subscription in every window claims and routes
//!   it (`TrayMessage::DeepLink`).
//!
//! macOS is NOT wired yet: LaunchServices delivers URLs as Apple
//! Events (`kAEGetURL`), not argv, so it needs an event handler whose
//! interaction with winit's NSApplication delegate is unverified from
//! this machine. The scheme is therefore not declared in Info.plist
//! either; both land together when they can be QA'd on hardware.

use base64::Engine as _;

use crate::messages::Message;
use iced::Task;

/// Hard cap on an incoming URL. A theme payload is ~1 KiB of base64;
/// anything near this size is hostile or corrupt.
const MAX_URL_LEN: usize = 128 * 1024;

/// A parsed, shape-validated deep link. Payload validation beyond the
/// shape (does the theme import? is the pairing peer reachable?) stays
/// with the flows the link routes into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLink {
    /// `oryxis://pair/<device_id>/<code>`: the sync pairing link the
    /// hosting device displays. Carried verbatim; the join flow's own
    /// `oryxis_sync::parse_pairing_link` re-validates at join time.
    Pair(String),
    /// `oryxis://theme/<base64url JSON>`: a theme file to import, the
    /// same bytes the gallery's Copy button yields. `ui` mirrors the
    /// `oryxis_ui_theme` marker and picks which import panel opens.
    ThemeInstall { json: String, ui: bool },
}

/// Parse a raw `oryxis://` URL. `None` means "not ours / malformed":
/// callers log and drop, they never surface parse errors to the user
/// (the user didn't type this, a web page did).
pub fn parse(url: &str) -> Option<DeepLink> {
    if url.len() > MAX_URL_LEN {
        return None;
    }
    // Browsers and the Windows shell like to append a trailing slash
    // to protocol launches. Neither route's payload can legitimately
    // end in one (pairing codes are digits, base64url has no `/`), so
    // strip it before the strict parsers see the link.
    let url = url.trim().trim_end_matches('/');
    let rest = url.strip_prefix("oryxis://")?;
    if rest.starts_with("pair/") {
        oryxis_sync::parse_pairing_link(url)?;
        return Some(DeepLink::Pair(url.to_string()));
    }
    let payload = rest.strip_prefix("theme/")?;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let bytes = engine.decode(payload).ok()?;
    let json = String::from_utf8(bytes).ok()?;
    // Shape gate only: is it a JSON object, and which kind of theme.
    // `parse_theme` / `parse_ui_theme` do the real validation when the
    // user presses Apply in the import panel this link opens.
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    let obj = value.as_object()?;
    let ui = obj.contains_key("oryxis_ui_theme");
    Some(DeepLink::ThemeInstall { json, ui })
}

impl crate::app::Oryxis {
    /// Route a parsed deep link. Locked vault stashes the link in
    /// `pending_deep_link` (the `--connect` pattern): the unlock
    /// handler and boot both drain it, so a link clicked at the lock
    /// screen lands right after the master password.
    pub(crate) fn handle_deep_link(&mut self, link: DeepLink) -> Task<Message> {
        use crate::messages::TabsMessage;
        if self.vault_ui.state != crate::state::VaultState::Unlocked {
            self.pending_deep_link = Some(link);
            return Task::none();
        }
        match link {
            DeepLink::Pair(url) => {
                // Prefill the join field and land on Settings > Sync;
                // the user presses Join (and confirms the code) as if
                // they had pasted the link themselves.
                self.sync.pairing.join_link_input = url;
                Task::done(Message::Tabs(TabsMessage::OpenSettingsSection(
                    crate::state::SettingsSection::Sync,
                )))
            }
            DeepLink::ThemeInstall { json, ui } => {
                // Mirror the ThemeImportOpen / UiThemeImportOpen
                // handlers (which reset these fields) and then prefill
                // the pasted-content editor, so the panel comes up as
                // if the user had pasted the file: Apply -> editor ->
                // Save keeps every existing validation and confirm.
                let section = if ui {
                    self.panels.ui_theme_import = true;
                    self.ui_theme_import_content =
                        iced::widget::text_editor::Content::with_text(&json);
                    self.ui_theme_import_name.clear();
                    self.ui_theme_import_error = None;
                    crate::state::SettingsSection::Interface
                } else {
                    self.panels.theme_import = true;
                    self.theme_ui.import_content =
                        iced::widget::text_editor::Content::with_text(&json);
                    self.theme_ui.import_name.clear();
                    self.theme_ui.import_error = None;
                    crate::state::SettingsSection::Terminal
                };
                Task::done(Message::Tabs(TabsMessage::OpenSettingsSection(section)))
            }
        }
    }

    /// Handle a raw URL claimed from the cross-process inbox while
    /// this window is already running. On Windows the window may be
    /// hidden to the tray, so surface it first; the link's own route
    /// then decides what to show.
    pub(crate) fn handle_deep_link_url(&mut self, url: &str) -> Task<Message> {
        let Some(link) = parse(url) else {
            tracing::warn!("deep link: ignoring malformed forwarded URL");
            return Task::none();
        };
        let route = self.handle_deep_link(link);
        #[cfg(target_os = "windows")]
        {
            return Task::batch([
                Task::done(Message::Tray(crate::messages::TrayMessage::Show)),
                route,
            ]);
        }
        #[cfg(not(target_os = "windows"))]
        route
    }
}

#[cfg(test)]
mod tests {
    use super::{DeepLink, parse};

    /// Encode a theme file the way the site's future Install button
    /// will: the inverse of the `theme/` arm of [`parse`].
    fn format_theme_link(json: &str) -> String {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("oryxis://theme/{}", engine.encode(json.as_bytes()))
    }

    const TERMINAL_JSON: &str =
        r##"{"name":"Night","author":"a","license":"MIT","background":"#000000"}"##;
    const UI_JSON: &str =
        r#"{"oryxis_ui_theme":1,"name":"Night","author":"a","license":"MIT","colors":{}}"#;

    #[test]
    fn theme_link_round_trips() {
        let link = format_theme_link(TERMINAL_JSON);
        assert_eq!(
            parse(&link),
            Some(DeepLink::ThemeInstall {
                json: TERMINAL_JSON.to_string(),
                ui: false,
            })
        );
    }

    #[test]
    fn ui_marker_selects_the_ui_panel() {
        let link = format_theme_link(UI_JSON);
        assert_eq!(
            parse(&link),
            Some(DeepLink::ThemeInstall { json: UI_JSON.to_string(), ui: true })
        );
    }

    #[test]
    fn browser_trailing_slash_is_tolerated() {
        let link = format!("{}/", format_theme_link(TERMINAL_JSON));
        assert!(parse(&link).is_some());
        let pair = "oryxis://pair/8f7a1c8e-3b1f-4e0e-9d3c-2b1a0f9e8d7c/123456/";
        assert_eq!(
            parse(pair),
            Some(DeepLink::Pair(pair.trim_end_matches('/').to_string()))
        );
    }

    #[test]
    fn pair_links_stay_strict() {
        // Bad code length / non-digits / broken uuid all fail shape
        // validation here, exactly like the join field would reject.
        assert_eq!(parse("oryxis://pair/not-a-uuid/123456"), None);
        assert_eq!(
            parse("oryxis://pair/8f7a1c8e-3b1f-4e0e-9d3c-2b1a0f9e8d7c/12345"),
            None
        );
        assert_eq!(
            parse("oryxis://pair/8f7a1c8e-3b1f-4e0e-9d3c-2b1a0f9e8d7c/abcdef"),
            None
        );
    }

    #[test]
    fn hostile_payloads_are_dropped() {
        // Wrong scheme / route.
        assert_eq!(parse("https://oryxis.app/themes"), None);
        assert_eq!(parse("oryxis://themes/abc"), None);
        // Not base64url.
        assert_eq!(parse("oryxis://theme/%%%"), None);
        // Valid base64 of invalid UTF-8.
        assert_eq!(parse("oryxis://theme/_w"), None);
        // Valid base64 of non-object JSON.
        let engine = {
            use base64::Engine as _;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"[1,2]")
        };
        assert_eq!(parse(&format!("oryxis://theme/{engine}")), None);
        // Oversized URL.
        let huge = format!("oryxis://theme/{}", "A".repeat(200 * 1024));
        assert_eq!(parse(&huge), None);
    }
}
