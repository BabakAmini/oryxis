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

/// A message routed to the sub-handler that owns its variant came back
/// declined: the router's group list and the sub's match went out of
/// sync (the variant was listed under the wrong group). Loud so the
/// drift is caught in development instead of silently dropping the
/// message, which is the exact bug class the exhaustive routers exist
/// to eliminate.
pub(crate) fn unrouted<M: std::fmt::Debug>(message: M) -> Task<Message> {
    debug_assert!(false, "message declined by its owning sub-handler: {message:?}");
    tracing::error!("message declined by its owning sub-handler: {message:?}");
    Task::none()
}

impl Oryxis {
    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Stall watchdog (#104): heartbeat + in-flight marker for this
        // message, dropped on every exit path. No-op unless the debug
        // logging toggle is on.
        let _stall_guard = crate::stall_watchdog::message_guard(&message);
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
        // An armed drag-out (issue #167) whose cursor crossed the
        // threshold becomes an OS drag NOW, while the button is still
        // down (the global left-release message disarms below, so a
        // finished click can never fire this late). The armed flag
        // keeps `mouse_interest` on, which is what streams the
        // CursorMoved messages this check rides on.
        let drag_out_started: Option<Task<Message>> = match self.drag_out_arm.as_ref() {
            Some(arm)
                if arm.press.distance(self.mouse_position)
                    >= crate::drag_out::DRAG_THRESHOLD =>
            {
                let arm = self.drag_out_arm.take().expect("checked above");
                Some(Task::perform(
                    crate::drag_out::prepare(arm.payload),
                    |result| {
                        Message::SidebarFiles(
                            crate::messages::SidebarFilesMessage::SidebarFilesDragOutReady(
                                result,
                            ),
                        )
                    },
                ))
            }
            _ => None,
        };
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
        // The drag-out task rides along with whatever the triggering
        // message returned (usually a MouseMoved's Task::none()).
        let task = match drag_out_started {
            Some(started) => Task::batch([task, started]),
            None => task,
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
        // Clipboard work the terminal crate queued for us this cycle
        // (copy-on-select, the copy chord, right-click copy, OSC 52). The
        // widget layer has no clipboard of its own on purpose: every access
        // in the process has to be serialized by the runtime, or two
        // concurrent Win32 clipboard opens kill the app outright. See
        // `oryxis_terminal::host_clipboard`.
        let mut extra: Vec<Task<Message>> =
            crate::dispatch_global::serve_terminal_clipboard_requests()
                .into_iter()
                .collect();
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
            extra.push(self.show_toast_secs(crate::i18n::t("privacy_hint_toast").to_string(), 6));
        }
        if extra.is_empty() {
            return task;
        }
        extra.insert(0, task);
        Task::batch(extra)
    }

    /// Whether anything in the app currently consumes continuous cursor
    /// positions, i.e. whether `CursorMoved` events should be forwarded
    /// as messages at all. Mirrors the `needs_drag_update` set in the
    /// `MouseMoved` handler, plus the two level-triggered readers in
    /// `view()`: the fullscreen top-zone reveal and the
    /// post-keyboard-nav hover restore (which needs exactly one move to
    /// clear `suppress_hover`, after which the gate closes again).
    fn mouse_interest(&self) -> bool {
        self.chat_ui.sidebar_drag.is_some()
            || self.sftp_chrome.split_drag.is_some()
            || self.sftp_chrome.log_drag.is_some()
            || self.sftp_chrome.col_resize.is_some()
            || self.sftp_chrome.col_drag.is_some()
            || self.sftp.drag.is_some()
            || self.tab_drag.is_some()
            || self.drag_out_arm.is_some()
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
        self.keys_ui.context_menu = None;
        self.identity_context_menu = None;
        self.port_forward_context_menu = None;
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
        // Exhaustive routing table: every domain's sub-enum goes straight to
        // its type-safe `handle_*(XMessage) -> Task` handler; the cross-cutting
        // globals go to `handle_global`. No catch-all, so the compiler enforces
        // that every message wrapper is routed (add an arm for a new domain).
        // Replaced the old 34-deep `try_handler!` fall-through chain (Step C of
        // the Message sub-enum conversion).
        match message {
            Message::KnownHost(m) => self.handle_known_hosts(m),
            Message::RemoteDesktop(m) => self.handle_remote_desktop(m),
            Message::SessionGroup(m) => self.handle_session_group(m),
            Message::Zmodem(m) => self.handle_zmodem(m),
            Message::Update(m) => self.handle_update(m),
            Message::PortForward(m) => self.handle_port_forwards(m),
            Message::Agent(m) => self.handle_agent(m),
            Message::ProxyIdentity(m) => self.handle_proxy_identity(m),
            Message::CommandHistory(m) => self.handle_command_history(m),
            Message::Navigation(m) => self.handle_navigation(m),
            Message::Ai(m) => self.handle_ai(m),
            Message::Plugin(m) => self.handle_plugins(m),
            Message::Snippet(m) => self.handle_snippets(m),
            Message::Vault(m) => self.handle_vault(m),
            Message::Sync(m) => self.handle_sync(m),
            Message::Mcp(m) => self.handle_mcp(m),
            Message::Editor(m) => self.handle_editor(m),
            Message::Share(m) => self.handle_share(m),
            Message::Tray(m) => self.handle_tray(m),
            Message::Onboarding(m) => self.handle_onboarding(m),
            Message::Player(m) => self.handle_player(m),
            Message::SidebarFiles(m) => self.handle_sidebar_files(m),
            Message::Monitor(m) => self.handle_monitor(m),
            Message::Tmux(m) => self.handle_tmux(m),
            Message::History(m) => self.handle_history(m),
            Message::Settings(m) => self.handle_settings(m),
            Message::Keys(m) => self.handle_keys(m),
            Message::Cloud(m) => self.handle_cloud(m),
            Message::Ssh(m) => self.handle_ssh(m),
            Message::Tabs(m) => self.handle_tabs(m),
            Message::Sftp(m) => self.handle_sftp_domain(m),
            // `SftpFor` is always unwrapped by `route_sftp_async` (in
            // `update`), which hoists the OWNING tab's state before
            // re-dispatching the inner message. One reaching this match
            // skipped that hoist, so executing it here would mutate
            // whatever tab's state happens to be in the `self.sftp`
            // buffer -- the wrong tab. Drop it loudly instead.
            Message::SftpFor(owner, inner) => {
                debug_assert!(false, "SftpFor({owner}) reached dispatch_message: {inner:?}");
                tracing::error!(
                    "SftpFor({owner}) reached dispatch_message without the \
                     route_sftp_async owner hoist; dropping: {inner:?}"
                );
                Task::none()
            }
            // SFTP type-ahead / list-nav peek runs before the terminal owns
            // the key (preserves the old try-chain ordering).
            Message::Terminal(TerminalMessage::KeyboardEvent(ke)) => {
                match self.sftp_type_ahead(ke) {
                    Ok(task) => task,
                    Err(ke) => self.handle_terminal(TerminalMessage::KeyboardEvent(ke)),
                }
            }
            Message::Terminal(m) => self.handle_terminal(m),
            // Cross-cutting globals (handled outside any single domain).
            Message::OpenUrl(_)
            | Message::CopyToClipboard(_)
            | Message::ClipboardWritten(_)
            | Message::ToastClear
            | Message::ToastDismiss
            | Message::ErrorDialogDismiss
            | Message::ErrorDialogRunAction
            | Message::TogglePrivacyReveal
            | Message::NoOp => self.handle_global(message),
        }
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
