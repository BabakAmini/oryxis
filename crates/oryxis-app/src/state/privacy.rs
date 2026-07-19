//! Privacy Mode state (issue #78): the global toggle, the volatile
//! session override, the per-class mask gates and the reveal toggle.
//! Pure data; the resolution helpers (`privacy_on`, `privacy_terms`,
//! `privacy_display_label`) stay on `Oryxis` in `dispatch_settings`.

/// All Privacy Mode state, one field on `Oryxis` (`self.privacy`).
pub(crate) struct PrivacyState {
    /// Global Privacy Mode default: when on, sensitive data (host / ip /
    /// user / port / proxy on cards and logs, plus IP and `user@host`
    /// prompt tokens in the terminal) is auto-hidden behind muted blocks
    /// and revealed on hover. Off by default. A per-host
    /// `Connection.privacy_mode` override wins over this. Mirrors the
    /// `privacy_mode` setting.
    pub mode: bool,
    /// Privacy Mode session override (issue #78): `Some(v)` forces the
    /// mode to `v` everywhere, above the global setting AND the
    /// per-host overrides; `None` follows the configuration. Volatile
    /// by design (never persisted): the use case is "I'm about to
    /// share my screen", not "change my configuration". Toggled by
    /// the Ctrl+Shift+M hotkey and the status-bar chip.
    pub session_override: Option<bool>,
    /// Whether the one-shot "Privacy Mode is masking output" hint toast
    /// already fired (issue #78). Mirrors the per-install
    /// `hint_privacy_mask` setting; in-memory so the draw-flag check in
    /// `update` stays a branch, not a vault read.
    pub hint_shown: bool,
    /// Privacy Mode always-mask list (issue #78): user-edited,
    /// comma-separated literals masked wherever they appear, on top of
    /// the vault-derived hostnames + usernames. Raw as typed; parsing
    /// happens in `privacy_terms()`. Mirrors the `privacy_always_mask`
    /// setting.
    pub always_mask: String,
    /// Privacy Mode never-mask list (issue #78): user-edited,
    /// comma-separated words the derived terms must NOT include,
    /// seeded with `PRIVACY_NEVER_MASK_DEFAULT` (root, ubuntu, ...) so
    /// shared usernames keep everyday output readable. Raw as typed.
    /// Mirrors the `privacy_never_mask` setting.
    pub never_mask: String,
    /// Per-class Privacy Mode gates (issue #78 block 1), all default
    /// on; each mirrors a `privacy_mask_*` setting. Public IPs get
    /// their own switch because documentation screenshots sometimes
    /// NEED the public address visible while everything else masks.
    pub mask_public_ips: bool,
    /// Private / loopback / link-local addresses (v4 + v6).
    pub mask_private_ips: bool,
    /// Username shapes (`user@host`, `/home/<u>`, `C:\Users\<u>`) AND
    /// the saved-connection usernames inside the terms list.
    pub mask_usernames: bool,
    /// Saved-connection hostnames inside the terms list.
    pub mask_hostnames: bool,
    /// Privacy Mode reveal toggle for the Logs view. When `false` (the
    /// default) sensitive data in the timeline + session-log viewer is
    /// masked behind muted blocks; the toolbar / viewer "Reveal" button
    /// flips this to show the raw values. Reset whenever the view is left.
    pub revealed: bool,
    /// Multi-line edit buffer behind the always-mask textarea. The
    /// `String` mirror above stays the read side (`privacy_terms()`
    /// runs per frame; `Content::text()` allocates), synced on every
    /// edit action. `text_editor::Content` is not `Clone`, same
    /// arrangement as `AiState::system_prompt`.
    pub always_mask_editor: iced::widget::text_editor::Content,
    /// Multi-line edit buffer behind the never-mask textarea; see
    /// `always_mask_editor`.
    pub never_mask_editor: iced::widget::text_editor::Content,
}

/// Manual impl (not derived) because the mask gates default ON and the
/// never-mask list is pre-seeded; must stay byte-identical to what the
/// old loose fields initialized in `boot`.
impl Default for PrivacyState {
    fn default() -> Self {
        let never_mask = crate::app::Oryxis::privacy_never_mask_default();
        Self {
            mode: false,
            session_override: None,
            hint_shown: false,
            always_mask: String::new(),
            mask_public_ips: true,
            mask_private_ips: true,
            mask_usernames: true,
            mask_hostnames: true,
            revealed: false,
            always_mask_editor: iced::widget::text_editor::Content::new(),
            never_mask_editor: iced::widget::text_editor::Content::with_text(
                &never_mask,
            ),
            never_mask,
        }
    }
}
