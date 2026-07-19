//! `Oryxis::update`, the master message-dispatch table. ~5k lines of
//! match arms; pulled out of `app.rs` so the wiring file stays trim.
//! All `pub(crate)` helpers it relies on live in sibling modules
//! (`sftp_helpers`, `sftp_methods`, `connect_methods`, `util`,
//! `boot`, `mcp`, `state`).

#![allow(clippy::collapsible_if)]
#![allow(clippy::collapsible_match)]
#![allow(clippy::too_many_lines)]

use iced::Task;


use crate::app::{SftpMessage, TabsMessage, TerminalMessage, Message, Oryxis};

/// How long a dynamic group's resolved host list stays "fresh" before
/// re-opening the group triggers a background re-resolve. Cloud
/// resources (ECS tasks especially) recycle, so a list older than this
/// is likely to contain dead rows that fail on click. 60s balances
/// freshness against hammering the cloud API on every navigation.
pub(crate) const DYNAMIC_GROUP_CACHE_TTL_SECS: i64 = 60;

/// Chain `message` through a domain handler. If the handler claims it
/// (returns `Ok`), short-circuit and return the resulting task.
/// Otherwise, the message is handed back unchanged for the next link.
macro_rules! try_handler {
    ($self:ident, $msg:ident, $handler:ident) => {
        match $self.$handler($msg) {
            Ok(task) => return task,
            Err(m) => m,
        }
    };
}

impl Oryxis {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Sync the cursor position from the event listener's atomics.
        // CursorMoved is only forwarded as a message while something
        // consumes continuous positions (see `mouse_interest` below), so
        // this top-of-update sync is what keeps click-time readers (drag
        // press anchors, the kebab-menu position) fresh the rest of the
        // time. A change since the previous message means the user
        // physically moved the mouse: restore the hover highlight that
        // keyboard navigation muted and count it as activity for the
        // vault auto-lock idle clock (the 30 s AutoLockTick is itself a
        // message, so a moving-but-not-clicking user is registered here
        // before the lock decision runs).
        let live_mouse = crate::subscription::live_mouse_position();
        if live_mouse != self.mouse_position {
            self.mouse_position = live_mouse;
            self.sftp.suppress_hover = false;
            self.last_user_activity = std::time::Instant::now();
        }
        // Any user input event resets the vault auto-lock idle clock.
        // These are the raw-event messages `subscription.rs` maps from
        // iced's global listener, so presence is detected app-wide
        // without touching individual handlers.
        if matches!(
            message,
            Message::Terminal(TerminalMessage::KeyboardEvent(_))
                | Message::Tabs(TabsMessage::MouseMoved(_))
                | Message::Sftp(SftpMessage::SftpMouseLeftPressed)
                | Message::Terminal(TerminalMessage::TerminalImeCommit(_))
        ) {
            self.last_user_activity = std::time::Instant::now();
        }
        // SFTP async-continuation messages target a specific tab that may no
        // longer be focused. Swap the owning tab's state into `self.sftp` for
        // the duration so the (unchanged) handlers route to the right tab,
        // then swap back. See `route_sftp_async`.
        let task = if let Some(id) = message.sftp_async_owner() {
            self.route_sftp_async(id, message)
        } else {
            self.dispatch_message(message)
        };
        // Keep the unified strip order (terminal + SFTP) in sync with the live
        // tabs after every message: new tabs appended, closed ones dropped,
        // drag-reordered order preserved.
        self.reconcile_tab_order();
        // Repair the hybrid-tab SFTP ownership invariant: drop a dangling
        // owner (tab removed by a path that bypasses CloseTab) and hoist
        // an active Files-mode tab that some direct `active_tab = ...`
        // assignment focused without going through SelectTab.
        self.reconcile_hybrid_sftp();
        // Track most-recently-used tab order for Ctrl+Tab. Must run after
        // `reconcile_tab_order`: the cycle's fallback walk order reads
        // `ordered_tab_refs`, which is derived from the freshly-synced
        // `tab_order`.
        self.reconcile_tab_mru();
        // Republish the cursor-forwarding gate from the post-update
        // state. Doing it here, once, after every message, means the
        // flag can never drift from the drag/fullscreen state that
        // demands continuous positions: the press that arms a drag is
        // itself a message, so the gate is already open by the time the
        // first CursorMoved of that drag arrives.
        crate::subscription::set_mouse_interest(self.mouse_interest());
        // One-shot Privacy Mode hint (issue #78): the first time a
        // redaction bar actually draws, spell out how the reveal works
        // ("hover to peek, click to pin"); getting silently masked with
        // no affordance is exactly how the #53 confusion happened. The
        // widget's draw pass has no message path, so it raises a
        // process-wide flag this loop swaps. Fires once per install
        // (`hint_` settings are per-install bookkeeping, excluded from
        // portable export).
        if oryxis_terminal::take_privacy_mask_drawn() && !self.privacy.hint_shown {
            self.privacy.hint_shown = true;
            self.persist_setting("hint_privacy_mask", "true");
            let hint =
                self.show_toast_secs(crate::i18n::t("privacy_hint_toast").to_string(), 6);
            return Task::batch([task, hint]);
        }
        task
    }

    /// Whether anything in the app currently consumes continuous cursor
    /// positions, i.e. whether `CursorMoved` events should be forwarded
    /// as messages at all. Mirrors the `needs_drag_update` set in the
    /// `MouseMoved` handler, plus the two level-triggered readers in
    /// `view()`: the fullscreen top-zone reveal and the
    /// post-keyboard-nav hover restore (which needs exactly one move to
    /// clear `suppress_hover`, after which the gate closes again).
    fn mouse_interest(&self) -> bool {
        self.chat_sidebar_drag.is_some()
            || self.sftp_split_drag.is_some()
            || self.sftp_log_drag.is_some()
            || self.sftp_col_resize.is_some()
            || self.sftp_col_drag.is_some()
            || self.sftp.drag.is_some()
            || self.tab_drag.is_some()
            || self.window_fullscreen
            || self.sftp.suppress_hover
    }

    /// Show a generic "remove this?" confirmation. Confirming dispatches
    /// `action` (the real `Delete*` message). Routes destructive removals
    /// (host, key, identity, snippet, session group) through an explicit
    /// confirm, mirroring the known-hosts / SFTP delete guards so a stray
    /// click can't silently drop an entry. Closes any open card menu first
    /// so it doesn't linger behind the dialog scrim.
    pub(crate) fn confirm_remove(&mut self, name: String, action: Message) {
        self.card_context_menu = None;
        self.snippet_context_menu = None;
        self.key_context_menu = None;
        self.identity_context_menu = None;
        self.overlay = None;
        self.error_dialog = Some(crate::state::ErrorDialog {
            title: crate::i18n::t("remove_confirm_title").to_string(),
            body: format!("\"{name}\""),
            link: None,
            action: Some(crate::state::ErrorDialogAction {
                label: crate::i18n::t("remove").to_string(),
                message: Box::new(action),
                danger: true,
            }),
        });
    }

    pub(crate) fn dispatch_message(&mut self, message: Message) -> Task<Message> {
        // Converted domains (Step C): each sub-enum routes straight to its
        // type-safe handler; everything else falls through to the shrinking
        // `try_handler!` chain below. New domains land in this match.
        let message = match message {
            Message::KnownHost(m) => return self.handle_known_hosts(m),
            Message::RemoteDesktop(m) => return self.handle_remote_desktop(m),
            Message::SessionGroup(m) => return self.handle_session_group(m),
            Message::Zmodem(m) => return self.handle_zmodem(m),
            other => other,
        };
        // Domain-specific handlers each claim a slice of `Message`
        // variants and return `Err(message)` for everything else, so
        // the chain naturally falls through to the inline match below.
        let message = try_handler!(self, message, handle_sftp_transfers);
        let message = try_handler!(self, message, handle_sftp_files);
        let message = try_handler!(self, message, handle_sftp_archive);
        let message = try_handler!(self, message, handle_sftp);
        let message = try_handler!(self, message, handle_ssh);
        let message = try_handler!(self, message, handle_update);
        let message = try_handler!(self, message, handle_port_forwards);
        let message = try_handler!(self, message, handle_settings);
        let message = try_handler!(self, message, handle_keys);
        let message = try_handler!(self, message, handle_agent);
        let message = try_handler!(self, message, handle_proxy_identity);
        let message = try_handler!(self, message, handle_plugins);
        let message = try_handler!(self, message, handle_cloud);
        let message = try_handler!(self, message, handle_ai);
        let message = try_handler!(self, message, handle_editor);
        let message = try_handler!(self, message, handle_tabs);
        let message = try_handler!(self, message, handle_terminal);
        let message = try_handler!(self, message, handle_command_history);
        let message = try_handler!(self, message, handle_sidebar_files);
        let message = try_handler!(self, message, handle_share);
        let message = try_handler!(self, message, handle_tray);
        let message = try_handler!(self, message, handle_vault);
        let message = try_handler!(self, message, handle_onboarding);
        let message = try_handler!(self, message, handle_snippets);
        let message = try_handler!(self, message, handle_navigation);
        let message = try_handler!(self, message, handle_history);
        let message = try_handler!(self, message, handle_player);
        let message = try_handler!(self, message, handle_mcp);
        let message = try_handler!(self, message, handle_sync);

        // Every Message variant is now claimed by one of the domain handlers
        // in the `try_handler!` chain above. Anything reaching here is an
        // unclaimed variant we forgot to wire up; treat as a no-op so we don't
        // crash on it (the handlers each fall through with `Err(message)`).
        let _ = message;
        Task::none()
    }

    /// Push the current window state (hidden + tab labels) into the
    /// tray_ipc registry so the primary's tray menu picks it up on
    /// its next scan. No-op for the primary itself (its tray rebuild
    /// reads from in-process Oryxis state directly, not via the
    /// filesystem registry).
    ///
    /// Signature-gated so 100 ms TrayPoll ticks don't churn the
    /// filesystem when nothing changed; explicit hide/show handlers
    /// also call this so the registry refreshes within one tick of
    /// the user action instead of waiting for the polling tick.
    pub(crate) fn broadcast_ipc_state_if_child(&mut self) {
        if crate::app::APP_IS_PRIMARY.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        self.is_window_hidden.hash(&mut h);
        self.tabs.len().hash(&mut h);
        for t in &self.tabs {
            t.label.hash(&mut h);
        }
        let sig = h.finish();
        if sig == self.ipc_state_signature {
            return;
        }
        self.ipc_state_signature = sig;
        let tabs: Vec<String> = self.tabs.iter().map(|t| t.label.clone()).collect();
        // Title: when the user has an active tab the label is what
        // they're staring at, otherwise fall back to a generic
        // "Oryxis" so the primary's submenu still has something to
        // show.
        let title = self
            .active_tab
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.label.clone())
            .unwrap_or_else(|| "Oryxis".to_string());
        crate::tray_ipc::Child::write_state(crate::tray_ipc::InstanceState {
            pid: std::process::id(),
            title,
            tabs,
            is_hidden: self.is_window_hidden,
        });
    }
}
