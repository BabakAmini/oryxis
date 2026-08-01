#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Prevent NVIDIA/AMD GPU drivers from treating this app as a game
// (disables automatic overlay activation on Windows)
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static NvOptimusEnablement: u32 = 0;
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub static AmdPowerXpressRequestHighPerformance: u32 = 0;

mod ai;
mod app;
mod biometric;
mod boot;
mod jumplist;
mod color_picker;
mod chat_persist;
mod command_capture;
mod connect_methods;
mod dispatch;
mod dispatch_ai;
mod dispatch_editor;
mod dispatch_keynav;
mod dispatch_keynav_modal;
mod dispatch_keynav_panel;
mod dispatch_keynav_sidebar;
mod dispatch_keys;
mod dispatch_known_hosts;
mod dispatch_tray;
mod dispatch_vault;
mod dispatch_snippets;
mod dispatch_navigation;
mod dispatch_onboarding;
mod dispatch_global;
mod dispatch_history;
mod dispatch_player;
mod gif_export;
mod dispatch_command_history;
mod dispatch_monitor;
mod dispatch_mcp;
mod dispatch_sync;
mod dispatch_proxy_identity;
mod dispatch_cloud;
mod dispatch_plugins;
mod dispatch_port_forwards;
mod dispatch_session_group;
mod dispatch_settings;
mod dispatch_sftp;
mod dispatch_sidebar_files;
mod dispatch_sftp_archive;
mod dispatch_sftp_files;
mod dispatch_sftp_sync;
mod dispatch_sftp_transfers;
mod dispatch_share;
mod dispatch_remote_desktop;
mod dispatch_serial;
mod dispatch_ssh;
mod dispatch_tabs;
mod dispatch_telnet;
mod dispatch_terminal;
mod dispatch_update;
mod dispatch_zmodem;
mod fonts;
#[cfg(feature = "harness")]
mod harness;
mod i18n;
mod key_encode;
mod keynav;
mod logging;
mod stall_watchdog;
mod agent_server;
mod dispatch_agent;
mod net_mirror;
mod mcp;
mod mcp_install;
mod messages;
mod mime_types;
mod os_icon;
// MSIX / Microsoft Store container probe. Gates the self-updater and
// the explicit AppUserModelID, both of which are wrong inside a package.
mod packaged;
// Split-anchor geometry for dropping a dragged tab into a pane grid.
mod pane_drop;
// Cloud-provider plugin subsystem. Inert until the cloud dispatch
// path is rewired onto it in a later PR, the `allow` keeps the
// clippy `-D warnings` gate green while the infra (and its public
// re-exports) sit unused.
#[allow(dead_code, unused_imports)]
mod plugins;
mod renderer_probe;
mod root_view;
// Locates the AWS `session-manager-plugin` system binary. Pure
// path-finding, no SDK, relocated here from `oryxis-cloud-aws` when
// the AWS provider moved into its plugin subprocess.
mod session_manager_plugin;
mod hotkeys;
mod ansi_render;
mod session_group_helpers;
mod palette;
mod paste_guard;
mod settings_index;
mod remote_desktop;
mod session_redact;
mod sftp_helpers;
mod sftp_methods;
mod shortcuts;
mod smart_tabs;
mod ssh_config;
mod state;
mod subscription;
mod sync_runtime;
mod tab_cycle;
mod theme;
mod monitor;
mod theme_export;
mod theme_import;
mod tray;
mod tray_ipc;
mod update;
mod util;
mod views;
mod widgets;

use iced::{window, Size};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const WINDOW_WIDTH: f32 = 1200.0;
const WINDOW_HEIGHT: f32 = 750.0;
const MIN_WIDTH: f32 = 800.0;
const MIN_HEIGHT: f32 = 500.0;

fn main() -> iced::Result {
    // Headless E2E harness argument pickup (feature `harness`). Must
    // run before anything else touches the vault or the environment:
    // when a harness mode is requested it redirects $HOME to a sandbox
    // directory, so a harness run can never read or write the real
    // ~/.oryxis. Still single-threaded here, so the env mutation is
    // sound under the Rust 2024 contract.
    #[cfg(feature = "harness")]
    let harness_options = harness::options_from_args();
    #[cfg(feature = "harness")]
    let harness_active = harness_options.is_some();
    #[cfg(not(feature = "harness"))]
    let harness_active = false;
    // A binary built WITHOUT the harness feature must refuse `--harness-*`
    // instead of silently falling through to the windowed app: that
    // fallthrough boots against the caller's REAL $HOME (an agent driving
    // QA right after a featureless rebuild would spawn stray windowed
    // instances on the real vault, which actually happened).
    #[cfg(not(feature = "harness"))]
    if std::env::args().any(|a| a.starts_with("--harness")) {
        eprintln!(
            "error: this binary was built without the `harness` feature; \
             rebuild with `cargo build -p oryxis-app --features harness`"
        );
        std::process::exit(2);
    }

    // rustls 0.23 requires a crypto provider to be installed before
    // any TLS connection, without it, the AWS SDK's HTTPS client
    // fails with a generic "dispatch failure". The workspace pins
    // `ring` as the rustls crypto, so install ring's default provider
    // here at process start. `install_default` returns Err if a
    // provider was already set (cheap re-entry from tests / repeated
    // calls), which we deliberately ignore.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Sweep the `.old.exe` left behind by a Windows nightly self-update
    // (no-op elsewhere). Done before anything else touches the binary.
    update::sweep_stale_binary();

    // Renderer escape hatch. Some GPU/driver stacks (seen on GNOME +
    // Mesa) corrupt the wgpu surface, bleeding other windows' pixels
    // into our chrome while a terminal session forces frequent redraws.
    // The corruption lives below iced (swapchain/present in the driver),
    // so we can't repaint our way out; instead we let the user pick a
    // different render path. Both knobs are read once while iced builds
    // its compositor, so they must be set before `iced::application(..)
    // .run()`. The setting lives in the vault's `settings` table, which
    // reads without the master password, so we resolve it here at
    // process start.
    //   - "opengl"   -> force wgpu's GL backend instead of Vulkan,
    //                   still hardware-accelerated; fixes most Vulkan-
    //                   on-Mesa corruption without the software cost.
    //   - "software" -> force iced's tiny-skia (CPU) renderer; the
    //                   terminal is a plain `canvas` widget so it renders
    //                   identically off the GPU.
    //   - "auto" / missing -> on Windows, probe the Vulkan/DX12 adapters
    //                   first and redirect to the software renderer when
    //                   the default pick has no healthy hardware path
    //                   (WARP-only setups, Haswell iGPUs whose EOL
    //                   drivers present undecorated windows offset on
    //                   every GPU backend). See renderer_probe.
    let mut renderer_mode: Option<String> = None;
    // Window geometry persisted by `persist_window_geometry` on every
    // exit path. Same unlocked settings read as the renderer knob: the
    // values must be known before `iced::application(..).run()` builds
    // the window, so they're resolved here rather than in boot().
    let mut window_size = Size::new(WINDOW_WIDTH, WINDOW_HEIGHT);
    let mut window_position = window::Position::Default;
    let mut window_maximized = false;
    let mut window_fullscreen = false;
    if let Ok(vault) = oryxis_vault::VaultStore::open_default() {
        renderer_mode = vault.get_setting("renderer_backend").ok().flatten();
        if let (Some(w), Some(h)) = (
            vault.get_setting("window_width").ok().flatten(),
            vault.get_setting("window_height").ok().flatten(),
        ) && let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>())
            && w.is_finite()
            && h.is_finite()
        {
            // Clamp so a hand-edited vault (or a corrupt row) can't
            // restore an unusably small window; oversized values are
            // capped by the window manager against the actual desktop.
            window_size = Size::new(
                w.clamp(MIN_WIDTH, 16384.0),
                h.clamp(MIN_HEIGHT, 16384.0),
            );
        }
        // Outer position in logical desktop coordinates. Legitimately
        // negative on monitors left of / above the primary, which is
        // exactly how the "same monitor" part round-trips; a stale
        // position (that monitor unplugged since) is rescued after boot
        // by `WindowEnsureOnScreen`. Absent rows (fresh install, or a
        // Wayland session where positions don't exist) fall through to
        // the platform default placement.
        if let (Some(x), Some(y)) = (
            vault.get_setting("window_pos_x").ok().flatten(),
            vault.get_setting("window_pos_y").ok().flatten(),
        ) && let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>())
            && x.is_finite()
            && y.is_finite()
        {
            window_position =
                window::Position::Specific(iced::Point::new(x, y));
        }
        window_maximized = vault
            .get_setting("window_maximized")
            .ok()
            .flatten()
            .as_deref()
            == Some("true");
        window_fullscreen = vault
            .get_setting("window_fullscreen")
            .ok()
            .flatten()
            .as_deref()
            == Some("true");
        // Debug logging (Settings > Advanced). Armed before the tracing
        // subscriber below is built so the earliest boot lines land in
        // the file too; same unlocked settings read as the renderer knob.
        if let Ok(Some(v)) = vault.get_setting("debug_logging")
            && v == "true"
            && let Err(e) = logging::enable()
        {
            eprintln!("oryxis: failed to enable debug logging: {e}");
        }
    }
    // SAFETY (both set_var sites): still single-threaded here (tracing
    // not yet initialized, no threads spawned), so mutating the process
    // environment is sound under the Rust 2024 contract.
    let mut renderer_probe_note: Option<String> = None;
    match renderer_mode.as_deref() {
        Some("opengl") => {
            // "OpenGL (GPU)" promises hardware acceleration. When the
            // GL stack can only offer a software rasterizer (llvmpipe
            // after a WSLg vGPU death, SwiftShader), wgpu-GL-on-software
            // is the one combination that MISRENDERS (missing background
            // quads, clip leaks), so honor the option's intent instead
            // of its letter and fall back to the correct software
            // renderer. Cross-platform: this stack class lives on
            // Linux/WSL, not just Windows.
            if let Some(redirect) = renderer_probe::opengl_backend_override() {
                unsafe { std::env::set_var(redirect.env_key, redirect.env_value) };
                renderer_probe::mark_redirected();
                renderer_probe_note = Some(redirect.reason);
            } else {
                unsafe { std::env::set_var("WGPU_BACKEND", "gl") }
            }
        }
        Some("software") => unsafe { std::env::set_var("ICED_BACKEND", "tiny-skia") },
        _ => {
            // The probe is Windows-only: that's where the broken-DX12
            // class of hardware lives, and where GL (WGL) is a reliable
            // hardware fallback. It respects pre-set env overrides.
            if cfg!(windows)
                && let Some(redirect) = renderer_probe::auto_backend_override()
            {
                unsafe { std::env::set_var(redirect.env_key, redirect.env_value) };
                renderer_probe::mark_redirected();
                renderer_probe_note = Some(redirect.reason);
            }
        }
    }

    // Self-heal a renderer crash on GPU/driver stacks that can't satisfy
    // iced_wgpu's shader capabilities (VMs, old drivers, software Vulkan):
    // catch the wgpu panic and relaunch with a safer backend. Installed after
    // the vault-driven backend env above so it escalates from whatever backend
    // is currently active. The debug-log hook goes in first so every panic is
    // stamped into the log file before any self-heal relaunch happens.
    install_panic_log_hook();
    install_renderer_fallback_hook();

    // CLI arg pickup, flags set when another Oryxis instance spawned
    // us via "Duplicate in New Window". Unknown flags are silently
    // ignored so future flags / OS double-click args don't crash boot.
    //   --connect <uuid>     : auto-open this saved connection
    //   --inherit-vault      : read the master password from stdin and
    //                          use it to unlock the vault on boot
    //   --relaunch           : this process replaces a prior one (e.g. a
    //                          renderer-change restart); wait briefly for
    //                          the old single-instance mutex to release so
    //                          we boot as primary, not as a tray-less child
    let mut args = std::env::args().skip(1);
    let mut inherit_vault = false;
    let mut relaunching = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--connect" => {
                if let Some(value) = args.next()
                    && let Ok(uuid) = uuid::Uuid::parse_str(&value)
                {
                    let _ = app::AUTO_CONNECT.set(uuid);
                }
            }
            "--inherit-vault" => {
                inherit_vault = true;
            }
            "--relaunch" => {
                relaunching = true;
            }
            _ => {}
        }
    }
    if inherit_vault {
        // Parent writes a single line to our stdin and closes the pipe;
        // anything past that line is ignored.
        use std::io::BufRead as _;
        let stdin = std::io::stdin();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).is_ok() {
            let pw = line.trim_end_matches(['\n', '\r']).to_string();
            if !pw.is_empty() {
                let _ = app::AUTO_PASSWORD.set(pw);
            }
        }
    }

    tracing_subscriber::registry()
        // `arboard` logs a WARN on every clipboard op when the Wayland
        // data-control protocol is unavailable (common under WSL / some
        // compositors) and it falls back to X11, which works fine. Quiet
        // that one target so copy-on-select doesn't spam the log on every
        // click; everything else stays at info.
        .with(tracing_subscriber::EnvFilter::new("oryxis=debug,info,arboard=error"))
        // In harness mode stdout belongs to the driving protocol (the
        // REPL's `== ` lines, MCP's JSON-RPC messages), so logs go to
        // stderr there; the normal app keeps stdout.
        .with((!harness_active).then(tracing_subscriber::fmt::layer))
        .with(harness_active.then(|| {
            tracing_subscriber::fmt::layer().with_writer(std::io::stderr)
        }))
        // Second sink for the Settings > Advanced debug-log file. Always
        // installed; the writer discards everything while the feature is
        // off, so the toggle works at runtime without rebuilding the
        // subscriber. No ANSI, the file is read in editors, not a tty.
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(logging::DebugFileWriter),
        )
        .init();

    tracing::info!("Starting Oryxis");
    if let Some(note) = renderer_probe_note {
        // Deferred from the probe above, which runs before the
        // subscriber exists.
        tracing::info!("renderer auto-probe: {note}");
    }

    // Single-instance + multi-window IPC roles. The first process to
    // boot grabs the mutex and owns the tray icon ("primary"); every
    // subsequent process becomes a "child" that registers with the
    // primary via the filesystem-based tray_ipc registry and skips
    // tray installation entirely. The primary's tray menu aggregates
    // all known windows into a single "Hidden windows" section so
    // the user sees one tray ruling them all instead of N duplicates.
    // On a relaunch the old process may not have released the
    // single-instance mutex yet, which would demote us to a tray-less
    // child. Retry for up to ~2 s so the intended in-place restart
    // comes back as primary. No-op where there is no mutex (Linux/macOS
    // report not-running immediately).
    let is_primary = if harness_active {
        // Headless harness: no tray, no IPC registry, no single-instance
        // coordination. The emulator process is self-contained and must
        // never demote a real running Oryxis to a child (or vice versa).
        true
    } else if relaunching {
        let mut primary = !tray::another_instance_running();
        let mut waited = 0;
        while !primary && waited < 2000 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            waited += 100;
            primary = !tray::another_instance_running();
        }
        primary
    } else {
        !tray::another_instance_running()
    };
    app::APP_IS_PRIMARY.store(is_primary, std::sync::atomic::Ordering::Relaxed);

    if !harness_active {
        tray_ipc::init_runtime_dirs();

        if is_primary {
            // Install the Windows system tray icon. No-op on macOS/Linux
            // until those platforms get their own backends. Failure here
            // is logged but non-fatal: the app still runs without a tray.
            if let Err(e) = tray::install() {
                tracing::warn!("tray icon registration failed: {e}");
            }
        } else {
            // Child: announce ourselves to the primary's registry.
            // Title is the default app title; per-window state updates
            // refine it later via tray_ipc::Child::write_state.
            tray_ipc::Child::register("Oryxis");
            tracing::info!("running as tray IPC child (primary already up)");
        }
    }

    // Load window icon from PNG
    let icon = load_icon();

    let mut application =
        iced::application(app::Oryxis::boot, app::Oryxis::update, app::Oryxis::view)
            .title(app::Oryxis::title)
            .theme(app::Oryxis::theme)
            .subscription(app::Oryxis::subscription);
    // Every bundled font, from the Lucide icon glyphs to the Nerd Font
    // terminal faces. The list (and the rationale for each entry) lives
    // in `fonts::BUNDLED_FONTS` so the headless harness can load the
    // exact same set into its windowless renderer.
    for font in fonts::BUNDLED_FONTS {
        application = application.font(*font);
    }
    let application = application
        // Default UI font is the bundled Noto Sans on every platform, so
        // the UI looks identical everywhere and never depends on a system
        // font being installed.
        .default_font(theme::SYSTEM_UI)
        .window(window::Settings {
            size: window_size,
            // The saved outer position also selects the monitor: winit
            // maximizes / fullscreens onto the monitor containing the
            // window's position, so a maximized restore lands on the
            // same display it was closed on.
            position: window_position,
            // Reopen maximized / fullscreen when the user left it that
            // way; `size` still carries the floating size underneath so
            // un-maximizing lands where the user last had the window.
            maximized: window_maximized,
            fullscreen: window_fullscreen,
            min_size: Some(Size::new(MIN_WIDTH, MIN_HEIGHT)),
            icon,
            decorations: false, // native title bar off, our own chrome in the tab bar
            // Take ownership of every close verb. With iced's default
            // (`true`) the winit shell closes the window itself on
            // `CloseRequested` and never forwards the event, so the
            // `window::close_requests()` subscription never fires and
            // Alt+F4 / taskbar Close bypass `handle_window_close`
            // entirely: close-to-tray is skipped, and so are the
            // session-log flush and the window-geometry save. Set to
            // `false` so every close path (our chrome X, Alt+F4, the
            // taskbar) lands on the same `Message::Tabs(TabsMessage::WindowClose)`
            // handler, which decides between hide-to-tray and a real
            // `window::close`. Not Windows-gated: the flush + geometry
            // recovery matter on every platform.
            exit_on_close_request: false,
            #[cfg(target_os = "windows")]
            platform_specific: window::settings::platform::PlatformSpecific {
                // Win11+ rounds corners only when DWM has a frame to
                // composite. Undecorated windows lose that by default,
                // so opt back in via the DWM corner-preference API and
                // re-enable the drop shadow that brings the rounded
                // mask along.
                corner_preference:
                    window::settings::platform::CornerPreference::Round,
                undecorated_shadow: true,
                ..Default::default()
            },
            #[cfg(target_os = "linux")]
            platform_specific: window::settings::PlatformSpecific {
                // Sets the X11 WM_CLASS and the Wayland app_id. GNOME
                // (and other desktops) match a running window to its
                // installed `.desktop` entry by this id to resolve the
                // taskbar / dock icon. The id must equal the .desktop
                // basename. For the .deb / AppImage that is "oryxis"; inside
                // a Flatpak the runtime exports FLATPAK_ID (the app id, e.g.
                // "app.oryxis.Oryxis"), which is also the installed .desktop
                // basename there, so honor it when present.
                application_id: std::env::var("FLATPAK_ID")
                    .unwrap_or_else(|_| "oryxis".to_string()),
                ..Default::default()
            },
            ..Default::default()
        })
        .antialiasing(true);

    // Headless E2E harness (feature `harness`): hand the fully
    // configured application to the emulator-backed driver instead of
    // opening a window. See `harness.rs` / docs/HARNESS.md.
    #[cfg(feature = "harness")]
    if let Some(options) = harness_options {
        return harness::run(application, options);
    }

    application.run()
}

fn load_icon() -> Option<window::Icon> {
    let bytes = include_bytes!("../../../resources/logo_64.png");
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    window::icon::from_rgba(img.into_raw(), w, h).ok()
}

/// Decide how to recover from a panic, given the retry count, the panic text
/// (message + source file), and which backend is currently forced. Returns the
/// `(renderer_backend setting, env key, env value)` to relaunch with, or `None`
/// to let the process crash. Pure so it can be unit-tested without a GPU.
///
/// Ladder: auto -> GL -> software. `retry >= 2` and "already on software" both
/// stop, so a genuinely unrenderable setup can't loop forever.
fn renderer_fallback_action(
    retry: u32,
    panic_text: &str,
    on_gl: bool,
    on_software: bool,
) -> Option<(&'static str, &'static str, &'static str)> {
    // Hard loop guard.
    if retry >= 2 {
        return None;
    }
    // Only act on renderer-related panics.
    const RENDERER_MARKERS: &[&str] = &[
        "wgpu",
        "naga",
        "create_shader",
        "Validation Error",
        "surface",
        "Surface",
        "adapter",
        "Backends",
        "iced_wgpu",
    ];
    if !RENDERER_MARKERS.iter().any(|k| panic_text.contains(k)) {
        return None;
    }
    if on_software {
        None // already on the software renderer, nothing safer to try
    } else if on_gl {
        Some(("software", "ICED_BACKEND", "tiny-skia"))
    } else {
        Some(("opengl", "WGPU_BACKEND", "gl"))
    }
}

/// Install a panic hook that self-heals a renderer crash by relaunching with a
/// safer wgpu backend. Some GPU/driver stacks (VMs, old drivers, software
/// Vulkan) expose an adapter whose shader capabilities don't match what
/// iced_wgpu requires (e.g. `SHADER_FLOAT16_IN_FLOAT32`), so wgpu panics during
/// shader validation *after* the device is created, which is past the point
/// where iced would fall back to its tiny-skia software renderer. We catch that
/// panic, escalate the renderer (see [`renderer_fallback_action`]), persist the
/// choice in the `renderer_backend` setting so the next cold launch skips the
/// crash, and re-exec.
/// Chain a hook that stamps every panic into the debug-log file (when
/// the Settings > Advanced toggle is on). See [`logging::log_panic`]
/// for why stderr alone isn't enough on Windows GUI builds. Both this
/// and the renderer fallback hook call the previous hook, so they
/// compose in installation order.
fn install_panic_log_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        logging::log_panic(info);
        prev(info);
    }));
}

fn install_renderer_fallback_hook() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Surface the panic first (stderr / logs), then try to heal.
        prev(info);

        let retry: u32 = std::env::var("ORYXIS_RENDER_RETRY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let file = info
            .location()
            .map(|l| l.file().to_string())
            .unwrap_or_default();
        let panic_text = format!("{msg} {file}");
        let on_software = std::env::var("ICED_BACKEND").ok().as_deref() == Some("tiny-skia");
        let on_gl = std::env::var("WGPU_BACKEND").ok().as_deref() == Some("gl");

        let Some((setting, env_key, env_val)) =
            renderer_fallback_action(retry, &panic_text, on_gl, on_software)
        else {
            return;
        };

        // Persist so future cold launches skip the crash, then relaunch with
        // the safer backend forced directly (works even if the write failed).
        if let Ok(vault) = oryxis_vault::VaultStore::open_default() {
            let _ = vault.set_setting("renderer_backend", setting);
        }
        let _ = std::process::Command::new(&exe)
            .args(&args)
            .env(env_key, env_val)
            .env("ORYXIS_RENDER_RETRY", (retry + 1).to_string())
            .spawn();
    }));
}

#[cfg(test)]
mod renderer_fallback_tests {
    use super::renderer_fallback_action;

    #[test]
    fn ignores_non_renderer_panics() {
        // A normal panic (no renderer marker) must not relaunch anything.
        assert_eq!(
            renderer_fallback_action(0, "index out of bounds: the len is 3 at app.rs", false, false),
            None
        );
    }

    #[test]
    fn auto_escalates_to_gl() {
        // The real-world float16 crash text, on the default (auto) backend.
        let text = "wgpu error: Validation Error in Device::create_shader_module wgpu_core.rs";
        assert_eq!(
            renderer_fallback_action(0, text, false, false),
            Some(("opengl", "WGPU_BACKEND", "gl"))
        );
    }

    #[test]
    fn gl_escalates_to_software() {
        assert_eq!(
            renderer_fallback_action(1, "wgpu_core surface error", true, false),
            Some(("software", "ICED_BACKEND", "tiny-skia"))
        );
    }

    #[test]
    fn software_is_the_end_of_the_ladder() {
        assert_eq!(
            renderer_fallback_action(1, "wgpu Validation Error", false, true),
            None
        );
    }

    #[test]
    fn retry_cap_stops_the_loop() {
        assert_eq!(
            renderer_fallback_action(2, "wgpu Validation Error", false, false),
            None
        );
    }

    #[test]
    fn detects_each_renderer_marker() {
        for marker in [
            "wgpu",
            "naga",
            "create_shader",
            "Validation Error",
            "iced_wgpu",
            "Surface",
            "adapter",
            "Backends",
        ] {
            let text = format!("thread 'main' panicked: {marker}");
            assert!(
                renderer_fallback_action(0, &text, false, false).is_some(),
                "marker {marker:?} should be detected as a renderer panic"
            );
        }
    }
}
