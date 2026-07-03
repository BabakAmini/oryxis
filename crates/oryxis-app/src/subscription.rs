//! `Oryxis::subscription`, the iced event/timer multiplexer. Pulled
//! out of `app.rs` so the message-loop module is more browsable.

use std::sync::atomic::{AtomicI32, Ordering};

use iced::Subscription;

use crate::app::{Message, Oryxis};

// Coarse-grained record of the last cursor position forwarded to the
// message loop. The subscription closure quantises to a 4 px grid and
// drops events that resolve to the same cell as the previous forward,
// so iced's bounded subscription channel can't be drowned by 100 Hz
// mouse-move bursts on dense pages (keychain grid, SFTP listing).
// Using i32 lets us store the snapped coords with one atomic each
// rather than reaching for a Mutex<Point>.
static LAST_MOUSE_X: AtomicI32 = AtomicI32::new(i32::MIN);
static LAST_MOUSE_Y: AtomicI32 = AtomicI32::new(i32::MIN);

// Interest gate for cursor-move forwarding. In iced, every forwarded
// message goes through `update()` and forces a full view() rebuild +
// layout + redraw, so streaming CursorMoved into the app re-renders the
// whole UI at mouse-move frequency (60-125 Hz) even when nothing the
// view draws depends on the position. Only a handful of app states
// genuinely consume continuous positions (active drags, the fullscreen
// top-zone reveal, the post-keyboard-nav hover restore), so the end of
// every `update()` recomputes this flag from that state
// (`Oryxis::mouse_interest`) and the listener below drops CursorMoved
// before it ever becomes a message while the flag is off. Widget-level
// hover (buttons, tooltips, the terminal canvas) rides iced's internal
// event path and keeps working regardless; this gate only affects the
// app-message lane.
static MOUSE_INTEREST: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// The live (raw, unsnapped) cursor position, updated on every
// CursorMoved even while `MOUSE_INTEREST` is off, stored as f32 bits.
// `Oryxis::update` syncs `self.mouse_position` from here at the top of
// every message, so click-time readers (drag press anchors, the kebab
// menu position) always see a fresh position without the app paying a
// re-render per mouse move. The same sync doubles as the activity
// signal: a position change since the previous message counts as user
// input for the vault auto-lock idle clock.
static LIVE_MOUSE_X: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static LIVE_MOUSE_Y: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Publish whether the app currently needs continuous cursor positions.
/// Called at the end of every `update()` pass.
pub(crate) fn set_mouse_interest(on: bool) {
    MOUSE_INTEREST.store(on, Ordering::Relaxed);
}

/// The most recent cursor position seen by the event listener, whether
/// or not it was forwarded as a message.
pub(crate) fn live_mouse_position() -> iced::Point {
    iced::Point {
        x: f32::from_bits(LIVE_MOUSE_X.load(Ordering::Relaxed)),
        y: f32::from_bits(LIVE_MOUSE_Y.load(Ordering::Relaxed)),
    }
}

impl Oryxis {
    pub fn subscription(&self) -> Subscription<Message> {
        let events = iced::event::listen_with(|event, _status, _window| {
            match event {
                iced::event::Event::Keyboard(ke) => Some(Message::KeyboardEvent(ke)),
                // Text committed by the OS IME (composed CJK characters,
                // etc.). Routed to the active PTY in dispatch_terminal,
                // behind the same focus guards as KeyboardEvent. Preedit /
                // open / close phases are handled by the OS overlay; only
                // the final commit needs forwarding.
                iced::event::Event::InputMethod(
                    iced::advanced::input_method::Event::Commit(text),
                ) => Some(Message::TerminalImeCommit(text)),
                iced::event::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    // Always record the raw position (cheap, no message):
                    // `update()` syncs `self.mouse_position` from these on
                    // the next message, so click-time consumers stay fresh
                    // even while forwarding is gated off.
                    LIVE_MOUSE_X.store(position.x.to_bits(), Ordering::Relaxed);
                    LIVE_MOUSE_Y.store(position.y.to_bits(), Ordering::Relaxed);
                    // Quantise to a 4 px grid. Same cell as last forward
                    // → drop the event before it hits the subscription
                    // channel. Drag handlers that need pixel precision
                    // recover the exact cursor coord from iced's own
                    // event state on the next non-debounced sample.
                    const SNAP: f32 = 4.0;
                    let sx = (position.x / SNAP).round() as i32;
                    let sy = (position.y / SNAP).round() as i32;
                    let prev_x = LAST_MOUSE_X.swap(sx, Ordering::Relaxed);
                    let prev_y = LAST_MOUSE_Y.swap(sy, Ordering::Relaxed);
                    if sx == prev_x && sy == prev_y {
                        return None;
                    }
                    // Nothing in the app consumes continuous positions
                    // right now: drop the event before it becomes a
                    // message (and a full view rebuild).
                    if !MOUSE_INTEREST.load(Ordering::Relaxed) {
                        return None;
                    }
                    Some(Message::MouseMoved(position))
                }
                // Global Left press, used to start a potential SFTP
                // internal drag. Doesn't capture the event, so widget-
                // level handlers (button click, etc.) still fire.
                iced::event::Event::Mouse(iced::mouse::Event::ButtonPressed(
                    iced::mouse::Button::Left,
                )) => Some(Message::SftpMouseLeftPressed),
                // Global mouse-up so the sidebar resize stops even when the
                // cursor leaves the resize handle while the user is dragging.
                // Same handler also closes any active SFTP internal drag.
                iced::event::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )) => Some(Message::ChatSidebarResizeStop),
                iced::event::Event::Window(iced::window::Event::Resized(size)) => {
                    Some(Message::WindowResized(size))
                }
                iced::event::Event::Window(iced::window::Event::Focused) => {
                    Some(Message::WindowFocusChanged(true))
                }
                iced::event::Event::Window(iced::window::Event::Unfocused) => {
                    Some(Message::WindowFocusChanged(false))
                }
                // OS-level file drag-and-drop. iced fires one event per
                // file, so multi-file drops produce a sequence of
                // `FileDropped` messages, they're just queued through
                // the SFTP upload handler.
                iced::event::Event::Window(iced::window::Event::FileHovered(_)) => {
                    Some(Message::SftpFileHovered)
                }
                iced::event::Event::Window(iced::window::Event::FilesHoveredLeft) => {
                    Some(Message::SftpFilesHoveredLeft)
                }
                iced::event::Event::Window(iced::window::Event::FileDropped(path)) => {
                    Some(Message::SftpFileDropped(path))
                }
                _ => None,
            }
        });
        let mut subs = vec![events];

        // 30-second poll for silent auto-reconnect of disconnected SSH
        // tabs. Unmounted while the vault is locked (soft auto-lock keeps
        // sessions alive): a reconnect needs credentials from the sealed
        // vault and would only burn retry attempts.
        if self.vault_ui.state == crate::state::VaultState::Unlocked {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(30))
                    .map(|_| Message::AutoReconnectTick),
            );
        }

        // 100 ms tick that drives the pulsing "loading" ring on the active
        // connection step. Only runs while a connection is in progress and
        // hasn't failed, no perpetual re-renders on idle.
        let is_connecting = self
            .connecting
            .as_ref()
            .map(|p| !p.failed)
            .unwrap_or(false);
        if is_connecting {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(100))
                    .map(|_| Message::ConnectAnimTick),
            );
        }
        // 2s mtime poll on the edit-in-place temp file, only ticks
        // while a session is actually active, otherwise idle. Scans every
        // SFTP tab (active buffer + parked) so a backgrounded edit-session
        // keeps watching for external saves.
        if self.sftp.edit_session.is_some()
            || self.sftp_tabs.iter().any(|t| t.state.edit_session.is_some())
        {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(2))
                    .map(|_| Message::SftpEditWatchTick),
            );
        }
        // Live transfer bar: poll the shared byte counter a few times a
        // second while a transfer runs, so the progress bar advances
        // smoothly. Idle otherwise. Scans every SFTP tab so a backgrounded
        // transfer keeps the bar live when refocused.
        if self.sftp.transfer.is_some()
            || self.sftp_tabs.iter().any(|t| t.state.transfer.is_some())
        {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(120))
                    .map(|_| Message::SftpTransferTick),
            );
        }
        // Intercept the user's close verb (Alt+F4, OS taskbar Close,
        // any path that produces a winit CloseRequested). iced 0.14
        // exposes a dedicated subscription for this; we route it
        // through the existing WindowClose dispatcher so the close-
        // to-tray check lives in one place.
        subs.push(iced::window::close_requests().map(|_| Message::WindowClose));

        // Tray icon event drain. On Windows the tray-icon crate runs
        // its own thread that pushes menu / icon events into a pair
        // of crossbeam channels; the dispatcher's `TrayPoll` handler
        // calls `tray::poll_*` to drain them. 100 ms is the same
        // cadence Tauri uses internally for the same job. Windows only.
        //
        // Split into two: a slow heartbeat and an event-driven click
        // path. The old design was a single 100 ms timer, but every tick
        // is a Message through `update()`, which forces a full
        // view()+layout+redraw of the entire app, 10x/s, forever, even
        // idle. On weak GPUs / slow CPUs that constant churn makes the
        // whole UI feel sluggish (scrolling especially).
        //
        // - Heartbeat (500 ms): the multi-window IPC housekeeping that
        //   genuinely needs a timer (rebuild the dynamic submenu when
        //   state changed, poll the primary's IPC commands from a child,
        //   promotion when the primary dies). 500 ms is plenty for those
        //   and cuts the idle re-render rate 5x.
        // - Clicks (event-driven): `tray_event_stream` polls the
        //   tray-icon crate's channels inside its own async task and only
        //   yields a Message when a real click arrives, so a menu / icon
        //   click still wakes the UI instantly while an idle tray never
        //   re-renders anything.
        #[cfg(target_os = "windows")]
        {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(500))
                    .map(|_| Message::TrayPoll),
            );
            subs.push(Subscription::run(tray_event_stream));
        }

        // Port forward liveness sweep. Only mounts while at least one
        // forward is active; a 5 s tick is enough to flip a row's toggle
        // back to off shortly after its connection drops, without polling
        // when nothing is forwarding.
        if !self.active_forwards.is_empty() {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(5))
                    .map(|_| Message::PortForwardLivenessTick),
            );
        }

        // Session-log flush ticker. Only mounts while at least one pane
        // is recording; drains the per-pane output buffers into the vault
        // every 2 s so an idle-but-trickling session still persists
        // promptly without a write per SSH chunk. Also unmounted while
        // the vault is locked (the log key is zeroized, a drain would
        // discard data): buffers accumulate and flush after unlock.
        if self.vault_ui.state == crate::state::VaultState::Unlocked
            && self
                .tabs
                .iter()
                .any(|t| t.pane_grid.panes.values().any(|p| p.session_log_id.is_some()))
        {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(2))
                    .map(|_| Message::SessionLogFlushTick),
            );
        }

        // Cloud auto-refresh ticker. Only mounts the subscription when
        // the user enabled the toggle in Settings; otherwise zero
        // background API calls. Interval reads the persisted setting
        // and falls back to 30 min on any parse failure so a malformed
        // value doesn't pin the ticker at 1 ms.
        if self.setting_cloud_auto_refresh_enabled && !self.cloud_profiles.is_empty() {
            let minutes = self
                .setting_cloud_auto_refresh_interval_minutes
                .parse::<u64>()
                .ok()
                .filter(|m| *m > 0)
                .unwrap_or(30);
            subs.push(
                iced::time::every(std::time::Duration::from_secs(minutes * 60))
                    .map(|_| Message::CloudAutoRefreshTick),
            );
        }
        // Cloud SSM/ECS idle keepalive. The SSM websocket drops the
        // session after ~20 min of inactivity, which bites when the user
        // alt-tabs away and comes back much later. We only mount the
        // ticker while the window is unfocused (an in-focus session has
        // the user's own input resetting the idle timer, and resizing a
        // visible terminal would be jarring) and only when at least one
        // SSM/ECS tab is open. 4 min comfortably beats the 20 min
        // default even allowing for a missed tick; users who lowered the
        // SSM idle timeout below ~5 min would need the server-side
        // setting raised instead.
        if !self.window_focused
            && self.tabs.iter().any(|t| t.ssm_keepalive)
        {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(240))
                    .map(|_| Message::SsmKeepaliveTick),
            );
        }
        // SFTP-sync auto cadence. The P2P transport runs its own timer
        // inside the engine; the SFTP transport has no engine, so the
        // cadence lives here. Only mounts in sftp + enabled + auto; the
        // tick is a no-op while a round is already in flight. 5 min
        // matches the P2P `auto_interval_secs` default.
        if self.sync.enabled && self.sync.transport == "sftp" && self.sync.mode == "auto" {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(300))
                    .map(|_| Message::SftpSyncTick),
            );
        }

        // Vault auto-lock idle check. Only mounts while unlocked with a
        // non-zero threshold configured, so the common case (feature off)
        // costs nothing. The 30 s cadence bounds the overshoot past the
        // configured idle threshold; the handler does the actual
        // elapsed-time comparison against `last_user_activity`.
        if self.vault_ui.state == crate::state::VaultState::Unlocked
            && self
                .setting_auto_lock_minutes
                .parse::<u64>()
                .ok()
                .filter(|m| *m > 0)
                .is_some()
        {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(30))
                    .map(|_| Message::AutoLockTick),
            );
        }

        Subscription::batch(subs)
    }
}

/// Event-driven tray click source (Windows only). Polls the tray-icon
/// crate's menu / icon channels inside this async task and yields a
/// `Message` only when a real event arrives, so an idle tray never
/// forces a UI re-render. The internal `try_recv` poll is cheap and,
/// crucially, does NOT go through `update()`, so it costs nothing on
/// the render side; only a yielded event wakes the app. Replaces the
/// old 100 ms `TrayPoll` timer that re-rendered the whole app 10x/s.
#[cfg(target_os = "windows")]
fn tray_event_stream() -> impl iced::futures::Stream<Item = Message> {
    iced::futures::stream::unfold((), |()| async {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Some(id) = crate::tray::poll_menu_event() {
                return Some((Message::TrayMenuEvent(id), ()));
            }
            // Left-click / double-click on the icon body restores the
            // window; other icon events (move, right-click, which
            // Windows handles by popping the menu itself) are ignored
            // and the loop keeps waiting.
            if let Some(ev) = crate::tray::poll_icon_event()
                && matches!(ev, tray_icon::TrayIconEvent::DoubleClick { .. })
            {
                return Some((Message::TrayIconDoubleClick, ()));
            }
        }
    })
}
