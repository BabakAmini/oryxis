//! Headless end-to-end harness (feature `harness`).
//!
//! Runs the real application (vault, subscriptions, SSH tasks, side
//! effects) inside `iced_test`'s [`Emulator`], with no window and no
//! display server, and exposes two entry points parsed from the CLI
//! before anything else in `main()`:
//!
//! - `oryxis --harness-run <dir>`: batch-runs every `.ice` test file
//!   in `<dir>` (see [`iced_test::Ice`] for the format). A failing
//!   instruction dumps a PNG screenshot plus a reproduction `.ice`
//!   into `<dir>/errors/` and exits non-zero. This is the CI mode.
//! - `oryxis --harness-repl`: an interactive line protocol on
//!   stdin/stdout for driving the app step by step. Every `.ice`
//!   instruction works as a command (`click "Hosts"`, `type "ls"`,
//!   `type enter`, `expect "Connected"`, `move (100, 200)`, ...),
//!   plus harness meta-commands:
//!
//!   - `screenshot [name]`: render the current UI to a PNG under the
//!     shots directory and print its path.
//!   - `texts`: dump every visible text widget with its bounds, a
//!     poor man's DOM inspector for picking click targets.
//!   - `find "text"`: bounds of the text widgets containing `text`.
//!   - `wait <ms>`: pump emulator events for a fixed duration (lets
//!     async work like a vault unlock or an SSH dial settle).
//!   - `settle [idle_ms]`: pump until the event stream stays quiet
//!     for `idle_ms` (default 250, capped at 5000).
//!   - `timeout <ms>`: set the per-instruction completion timeout.
//!   - `help`, `quit`.
//!
//!   Responses are single lines prefixed with `== ` so they can be
//!   told apart from tracing output sharing stdout: `== ok`,
//!   `== fail <instruction>`, `== timeout ...`, `== shot <path>`,
//!   `== error <reason>`.
//!
//! Isolation: both modes redirect `$HOME` to a sandbox directory
//! (default `<tmp>/oryxis-harness`, override with `--home <dir>`)
//! *before* anything reads the vault, so a harness run can never
//! touch the real `~/.oryxis`. The sandbox persists across runs by
//! design: a master password set in one REPL session is still there
//! in the next, which keeps iterative QA cheap.
//!
//! Flags shared by both modes: `--home <dir>`, `--shots <dir>`,
//! `--viewport <WxH>` (default 1200x750), `--scale <factor>`
//! (default 1), `--mode zen|patient|immediate` (default zen, see
//! [`emulator::Mode`]) and `--timeout-ms <ms>` (default 20000).
//!
//! Fonts: the emulator path never runs the shell's boot-time font
//! loading, so [`run`] pushes `fonts::BUNDLED_FONTS` straight into
//! the global font system before booting; without this every icon
//! renders as tofu and text selectors still match but screenshots
//! are useless.

use std::borrow::Cow;
use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::{Program, Size};
use iced_test::Instruction;
use iced_test::core::renderer::{self as core_renderer, Headless as _};
use iced_test::core::theme;
use iced_test::core::widget::Id;
use iced_test::core::widget::operation::Operation;
use iced_test::core::{Rectangle, mouse, shell, window};
use iced_test::emulator::{self, Emulator, Event};
use iced_test::futures::futures::channel::mpsc::{self, TryRecvError};
use iced_test::futures::futures::executor::block_on;
use iced_test::runtime::{UserInterface, user_interface};

/// How long the initial boot (vault open, font tasks, update check)
/// may take before the REPL gives up waiting and hands over control
/// anyway. Generous because a cold tokio + headless-renderer start
/// under a software rasterizer can be slow.
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);

/// A parsed harness invocation. Returned by [`options_from_args`]
/// and consumed by [`run`].
pub struct Options {
    /// `--harness-run <dir>`: batch mode over `.ice` files. `None`
    /// means REPL mode.
    batch: Option<PathBuf>,
    /// The sandbox directory `$HOME` was redirected to.
    home: PathBuf,
    /// Where `screenshot` PNGs land.
    shots: PathBuf,
    /// Logical viewport of the emulated window.
    viewport: Size,
    /// Screenshot scale factor (1.0 = one pixel per logical unit).
    scale: f32,
    /// Task-waiting strategy for the emulator.
    mode: emulator::Mode,
    /// Per-instruction completion timeout in the REPL.
    timeout: Duration,
}

/// Parses harness flags out of the process arguments. Returns `None`
/// when no harness mode was requested (the normal app path). On a
/// malformed harness invocation it prints the problem and exits,
/// a test tool must never silently mis-run.
///
/// When a mode IS requested, this redirects `$HOME` (and
/// `%USERPROFILE%` on Windows) to the sandbox before returning, so
/// every later `dirs::home_dir()` call in the process, the vault,
/// plugin cache, fonts, logs, resolves inside the sandbox.
pub fn options_from_args() -> Option<Options> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(options) => options?,
        Err(reason) => {
            eprintln!("oryxis harness: {reason}");
            std::process::exit(2);
        }
    };

    if let Err(e) = std::fs::create_dir_all(&options.home)
        .and_then(|()| std::fs::create_dir_all(&options.shots))
    {
        eprintln!("oryxis harness: cannot create sandbox dirs: {e}");
        std::process::exit(2);
    }

    // SAFETY: called at the very top of `main()`, still
    // single-threaded, so mutating the process environment is sound
    // under the Rust 2024 contract (same rationale as the renderer
    // env knobs in `main.rs`).
    unsafe {
        std::env::set_var("HOME", &options.home);
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", &options.home);
    }

    Some(options)
}

/// Pure argument parser, split from [`options_from_args`] so it can
/// be unit-tested without touching the environment.
fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut repl = false;
    let mut batch: Option<PathBuf> = None;
    let mut home: Option<PathBuf> = None;
    let mut shots: Option<PathBuf> = None;
    let mut viewport = Size::new(1200.0, 750.0);
    let mut scale = 1.0f32;
    let mut mode = emulator::Mode::Zen;
    let mut timeout = Duration::from_millis(20_000);
    // Harness-only flags seen without a harness mode are an error,
    // not a silent no-op: `--home` on a normal launch means the user
    // thought they were sandboxed when they weren't.
    let mut harness_flags: Vec<&str> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &'static str| {
            it.next().ok_or_else(|| format!("{flag} requires a value"))
        };
        match arg.as_str() {
            "--harness-repl" => repl = true,
            "--harness-run" => {
                batch = Some(PathBuf::from(value("--harness-run")?));
            }
            "--home" => {
                harness_flags.push("--home");
                home = Some(PathBuf::from(value("--home")?));
            }
            "--shots" => {
                harness_flags.push("--shots");
                shots = Some(PathBuf::from(value("--shots")?));
            }
            "--viewport" => {
                harness_flags.push("--viewport");
                let raw = value("--viewport")?;
                let (w, h) = raw
                    .split_once(['x', 'X'])
                    .ok_or_else(|| format!("--viewport wants WxH, got {raw:?}"))?;
                let (w, h) = (
                    w.parse::<f32>().map_err(|e| format!("--viewport width: {e}"))?,
                    h.parse::<f32>().map_err(|e| format!("--viewport height: {e}"))?,
                );
                if !(w.is_finite() && h.is_finite() && w >= 200.0 && h >= 200.0) {
                    return Err(format!("--viewport {raw:?} is out of range"));
                }
                viewport = Size::new(w, h);
            }
            "--scale" => {
                harness_flags.push("--scale");
                let raw = value("--scale")?;
                scale = raw.parse::<f32>().map_err(|e| format!("--scale: {e}"))?;
                if !(scale.is_finite() && (0.25..=4.0).contains(&scale)) {
                    return Err(format!("--scale {raw:?} is out of range (0.25..=4)"));
                }
            }
            "--mode" => {
                harness_flags.push("--mode");
                mode = match value("--mode")?.as_str() {
                    "zen" => emulator::Mode::Zen,
                    "patient" => emulator::Mode::Patient,
                    "immediate" => emulator::Mode::Immediate,
                    other => {
                        return Err(format!(
                            "--mode {other:?} (want zen, patient or immediate)"
                        ));
                    }
                };
            }
            "--timeout-ms" => {
                harness_flags.push("--timeout-ms");
                let ms = value("--timeout-ms")?
                    .parse::<u64>()
                    .map_err(|e| format!("--timeout-ms: {e}"))?;
                timeout = Duration::from_millis(ms.clamp(100, 600_000));
            }
            // Anything else belongs to the normal CLI (`--connect`,
            // `--relaunch`, ...) and is left for `main()`'s own loop.
            _ => {}
        }
    }

    if !repl && batch.is_none() {
        if let Some(flag) = harness_flags.first() {
            return Err(format!(
                "{flag} only makes sense with --harness-repl or --harness-run"
            ));
        }
        return Ok(None);
    }
    if repl && batch.is_some() {
        return Err("--harness-repl and --harness-run are mutually exclusive".into());
    }

    let home = home.unwrap_or_else(|| std::env::temp_dir().join("oryxis-harness"));
    let shots = shots.unwrap_or_else(|| home.join("shots"));

    Ok(Some(Options {
        batch,
        home,
        shots,
        viewport,
        scale,
        mode,
        timeout,
    }))
}

/// Entry point called from `main()` with the fully configured
/// application builder (`iced::Application` implements
/// [`Program`], so the emulator runs the exact program the shell
/// would, window settings aside).
pub fn run<P>(program: P, options: Options) -> iced::Result
where
    P: Program + 'static,
{
    load_bundled_fonts();

    if let Some(dir) = options.batch.clone() {
        return match iced_test::run(program, &dir) {
            Ok(()) => {
                println!("== ok all ice tests passed in {}", dir.display());
                Ok(())
            }
            Err(error) => {
                eprintln!("oryxis harness: ice run failed: {error}");
                std::process::exit(1);
            }
        };
    }

    repl(program, options)
}

/// The emulator never runs the shell's boot-time font loading (and
/// `runtime::Action::Font` is a TODO upstream), so push the bundled
/// fonts straight into the global cosmic-text font system the
/// headless renderers resolve glyphs through.
fn load_bundled_fonts() {
    let mut font_system = iced::advanced::graphics::text::font_system()
        .write()
        .expect("write font system");
    for font in crate::fonts::BUNDLED_FONTS {
        font_system.load_font(Cow::Borrowed(*font));
    }
}

/// Outcome of pumping the emulator event channel while an
/// instruction is in flight.
enum Pump {
    Ready,
    Failed(Instruction),
    Timeout,
    Closed,
}

/// One interactive REPL session over a booted emulator.
struct Session<P>
where
    P: Program + 'static,
{
    emulator: Emulator<P>,
    receiver: mpsc::Receiver<Event<P>>,
    /// Window id handed to `Program::view`. Single-window
    /// applications ignore it, any unique id works.
    window: window::Id,
    /// Lazily created second headless renderer that backs
    /// `screenshot`, `texts` and `find`. The emulator has its own
    /// renderer, but its `screenshot()` never restores the widget
    /// cache it takes (upstream bug: `cache.take().unwrap()` with no
    /// write-back poisons the next instruction), so the harness keeps
    /// probing strictly on its own renderer + cache pair and never
    /// touches the emulator's.
    probe_renderer: Option<P::Renderer>,
    /// Widget-state cache paired with `probe_renderer`. Persistent
    /// across probe calls so consecutive screenshots stay coherent;
    /// note it does NOT see widget state the emulator's interactions
    /// build up (scroll offsets, focus rings), those live in the
    /// emulator's private cache. Values, layout and text all come
    /// from app state, so screenshots stay truthful for QA.
    probe_cache: Option<user_interface::Cache>,
    viewport: Size,
    scale: f32,
    shots: PathBuf,
    timeout: Duration,
    shot_counter: u32,
}

fn repl<P>(program: P, options: Options) -> iced::Result
where
    P: Program + 'static,
{
    let (sender, receiver) = mpsc::channel(256);
    let mut session = Session {
        emulator: Emulator::new(sender, &program, options.mode, options.viewport),
        receiver,
        window: window::Id::unique(),
        probe_renderer: None,
        probe_cache: Some(user_interface::Cache::default()),
        viewport: options.viewport,
        scale: options.scale,
        shots: options.shots,
        timeout: options.timeout,
        shot_counter: 0,
    };

    match session.pump_until_ready(&program, BOOT_TIMEOUT) {
        Pump::Ready => {}
        Pump::Timeout => respond("boot timeout (continuing; try `settle` or `wait`)"),
        Pump::Failed(instruction) => respond(format!("boot fail {instruction}")),
        Pump::Closed => {
            respond("error emulator channel closed during boot");
            return Ok(());
        }
    }
    respond(format!(
        "harness ready home={} shots={} viewport={}x{} mode={:?}",
        options.home.display(),
        session.shots.display(),
        options.viewport.width,
        options.viewport.height,
        options.mode,
    ));

    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        // Absorb whatever the subscriptions produced while we were
        // blocked on stdin, so commands act on fresh state.
        session.drain(&program);

        line.clear();
        match stdin.lock().read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let command = line.trim();
        if command.is_empty() || command.starts_with('#') {
            continue;
        }

        let (head, rest) = match command.split_once(char::is_whitespace) {
            Some((head, rest)) => (head, rest.trim()),
            None => (command, ""),
        };

        match head {
            "quit" | "exit" => break,
            "help" => {
                for line in HELP.lines() {
                    respond(line);
                }
            }
            "screenshot" => match session.screenshot(&program, rest) {
                Ok(path) => respond(format!("shot {}", path.display())),
                Err(reason) => respond(format!("error {reason}")),
            },
            "texts" => match session.texts(&program) {
                Ok(entries) => {
                    let count = entries.len();
                    for (text, bounds) in entries {
                        respond(format_text_entry(&text, bounds));
                    }
                    respond(format!("ok {count} texts"));
                }
                Err(reason) => respond(format!("error {reason}")),
            },
            "find" => match parse_quoted(rest) {
                Some(needle) => match session.texts(&program) {
                    Ok(entries) => {
                        let matches: Vec<_> = entries
                            .into_iter()
                            .filter(|(text, _)| text.contains(&needle))
                            .collect();
                        let count = matches.len();
                        for (text, bounds) in matches {
                            respond(format_text_entry(&text, bounds));
                        }
                        respond(format!("ok {count} matches"));
                    }
                    Err(reason) => respond(format!("error {reason}")),
                },
                None => respond("error find wants a quoted string: find \"Hosts\""),
            },
            "wait" => match rest.parse::<u64>() {
                Ok(ms) => {
                    session.wait(&program, Duration::from_millis(ms.min(600_000)));
                    respond("ok");
                }
                Err(_) => respond("error wait wants milliseconds: wait 500"),
            },
            "settle" => {
                let idle = rest.parse::<u64>().unwrap_or(250).clamp(10, 5_000);
                session.settle(
                    &program,
                    Duration::from_millis(idle),
                    Duration::from_secs(30),
                );
                respond("ok");
            }
            "timeout" => match rest.parse::<u64>() {
                Ok(ms) => {
                    session.timeout = Duration::from_millis(ms.clamp(100, 600_000));
                    respond("ok");
                }
                Err(_) => respond("error timeout wants milliseconds: timeout 30000"),
            },
            _ => match Instruction::parse(command) {
                Ok(instruction) => {
                    session.emulator.run(&program, &instruction);
                    match session.pump_until_ready(&program, session.timeout) {
                        Pump::Ready => respond("ok"),
                        Pump::Failed(instruction) => respond(format!("fail {instruction}")),
                        Pump::Timeout => {
                            respond("timeout (tasks still pending; `settle` may absorb them)");
                        }
                        Pump::Closed => {
                            respond("error emulator channel closed");
                            break;
                        }
                    }
                }
                Err(error) => respond(format!("error {error}")),
            },
        }
    }

    respond("bye");
    Ok(())
}

const HELP: &str = "\
instructions: click [right] \"Text\"|#id|(x, y) / press / release / move <target>
              type \"text\" / type enter|escape|tab|backspace / expect \"Text\"
harness:      screenshot [name] / texts / find \"Text\" / wait <ms>
              settle [idle_ms] / timeout <ms> / help / quit
responses:    == ok | == fail <instr> | == timeout | == shot <path> | == error <..>";

impl<P> Session<P>
where
    P: Program + 'static,
{
    /// Pumps the event channel, performing emulator actions, until a
    /// Ready/Failed for the in-flight instruction arrives or the
    /// timeout passes. Mirrors the loop in `iced_test::run`, with a
    /// deadline so a hung task hands control back to the driver.
    fn pump_until_ready(&mut self, program: &P, timeout: Duration) -> Pump {
        let deadline = Instant::now() + timeout;
        loop {
            match self.receiver.try_recv() {
                Ok(Event::Action(action)) => self.emulator.perform(program, action),
                Ok(Event::Ready) => return Pump::Ready,
                Ok(Event::Failed(instruction)) => return Pump::Failed(instruction),
                Err(TryRecvError::Closed) => return Pump::Closed,
                Err(TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return Pump::Timeout;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    /// Single pass over whatever is already queued. Stale
    /// Ready/Failed events (an earlier timed-out instruction finally
    /// settling) are deliberately dropped here so they can't be
    /// mistaken for the *next* instruction's completion.
    fn drain(&mut self, program: &P) {
        while let Ok(event) = self.receiver.try_recv() {
            if let Event::Action(action) = event {
                self.emulator.perform(program, action);
            }
        }
    }

    /// Pumps for a fixed wall-clock duration.
    fn wait(&mut self, program: &P, duration: Duration) {
        let deadline = Instant::now() + duration;
        loop {
            match self.receiver.try_recv() {
                Ok(Event::Action(action)) => self.emulator.perform(program, action),
                Ok(_) => {}
                Err(TryRecvError::Closed) => return,
                Err(TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    /// Pumps until the channel stays quiet for `idle` (capped at
    /// `cap` total), a pragmatic "let the async dust settle".
    fn settle(&mut self, program: &P, idle: Duration, cap: Duration) {
        let start = Instant::now();
        let mut last_event = Instant::now();
        loop {
            match self.receiver.try_recv() {
                Ok(event) => {
                    if let Event::Action(action) = event {
                        self.emulator.perform(program, action);
                    }
                    last_event = Instant::now();
                }
                Err(TryRecvError::Closed) => return,
                Err(TryRecvError::Empty) => {
                    let now = Instant::now();
                    if now.duration_since(last_event) >= idle || now.duration_since(start) >= cap
                    {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    /// Creates the probe renderer on first use. Split from the
    /// callers so they can re-borrow `self` field-by-field afterwards.
    fn ensure_probe_renderer(&mut self, program: &P) -> Result<(), String> {
        if self.probe_renderer.is_none() {
            let settings = program.settings();
            let renderer = block_on(P::Renderer::new(
                core_renderer::Settings::from(&settings),
                None,
            ))
            .ok_or("could not create a headless probe renderer")?;
            self.probe_renderer = Some(renderer);
        }
        Ok(())
    }

    /// Renders the current UI into a PNG under the shots directory.
    ///
    /// Mirrors `Emulator::screenshot` (build, redraw-update, draw,
    /// read pixels) but on the probe renderer/cache pair, see the
    /// `probe_renderer` field docs for why the emulator's own
    /// screenshot path is off limits.
    fn screenshot(&mut self, program: &P, name: &str) -> Result<PathBuf, String> {
        self.shot_counter += 1;
        let name = if name.is_empty() {
            format!("shot-{:04}", self.shot_counter)
        } else {
            sanitize_name(name)
        };

        self.ensure_probe_renderer(program)?;
        let Session {
            emulator,
            probe_renderer,
            probe_cache,
            window,
            viewport,
            scale,
            shots,
            ..
        } = self;
        let renderer = probe_renderer.as_mut().expect("probe renderer ensured");

        let theme = emulator
            .theme(program)
            .unwrap_or_else(|| <P::Theme as theme::Base>::default(theme::Mode::None));
        let style = program.style(emulator.state(), &theme);

        let mut ui = UserInterface::build(
            program.view(emulator.state(), *window),
            *viewport,
            probe_cache.take().unwrap_or_default(),
            renderer,
        );
        let _ = ui.update(
            &window::Headless,
            &shell::Waker::noop(),
            &[iced_test::core::Event::Window(window::Event::RedrawRequested(
                Instant::now(),
            ))],
            mouse::Cursor::Unavailable,
            renderer,
            &mut Vec::new(),
        );
        ui.draw(
            renderer,
            &theme,
            &core_renderer::Style {
                text_color: style.text_color,
            },
            mouse::Cursor::Unavailable,
        );
        *probe_cache = Some(ui.into_cache());

        let physical = iced_test::core::Size::new(
            (viewport.width * *scale).round() as u32,
            (viewport.height * *scale).round() as u32,
        );
        let rgba = renderer.screenshot(physical, *scale, style.background_color);

        let path = shots.join(format!("{name}.png"));
        let image = image::RgbaImage::from_raw(physical.width, physical.height, rgba)
            .ok_or("screenshot buffer size mismatch")?;
        image.save(&path).map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Lays the current view out with the probe renderer and collects
    /// every visible text widget with its bounds, sorted in reading
    /// order.
    fn texts(&mut self, program: &P) -> Result<Vec<(String, Rectangle)>, String> {
        self.ensure_probe_renderer(program)?;
        let Session {
            emulator,
            probe_renderer,
            probe_cache,
            window,
            viewport,
            ..
        } = self;
        let renderer = probe_renderer.as_mut().expect("probe renderer ensured");

        let mut ui = UserInterface::build(
            program.view(emulator.state(), *window),
            *viewport,
            probe_cache.take().unwrap_or_default(),
            renderer,
        );
        let mut dump = DumpTexts::default();
        ui.operate(renderer, &mut dump);
        *probe_cache = Some(ui.into_cache());

        let mut entries = dump.entries;
        entries.sort_by(|(_, a), (_, b)| {
            (a.y, a.x)
                .partial_cmp(&(b.y, b.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(entries)
    }
}

/// Widget-tree operation collecting every non-empty text with its
/// layout bounds.
#[derive(Default)]
struct DumpTexts {
    entries: Vec<(String, Rectangle)>,
}

impl Operation for DumpTexts {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
        operate(self);
    }

    fn text(&mut self, _id: Option<&Id>, bounds: Rectangle, text: &str) {
        let text = text.trim();
        if !text.is_empty() {
            self.entries.push((text.to_owned(), bounds));
        }
    }
}

fn format_text_entry(text: &str, bounds: Rectangle) -> String {
    format!(
        "text {text:?} @ ({x}, {y}) {w}x{h}",
        x = bounds.x.round(),
        y = bounds.y.round(),
        w = bounds.width.round(),
        h = bounds.height.round(),
    )
}

/// Extracts the payload of a double-quoted argument, tolerating an
/// unquoted single word for convenience.
fn parse_quoted(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        (!inner.is_empty()).then(|| inner.to_owned())
    } else {
        (!raw.is_empty() && !raw.contains('"')).then(|| raw.to_owned())
    }
}

/// Keeps screenshot names filesystem-safe.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Protocol response: one line, `== ` prefixed (so it can't be
/// confused with tracing output on the same stream), flushed
/// immediately because stdout is block-buffered when piped.
fn respond(message: impl AsRef<str>) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "== {}", message.as_ref());
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_harness_flags_means_normal_run() {
        assert!(parse(&args(&["--connect", "abc"])).unwrap().is_none());
        assert!(parse(&[]).unwrap().is_none());
    }

    #[test]
    fn repl_mode_with_defaults() {
        let options = parse(&args(&["--harness-repl"])).unwrap().unwrap();
        assert!(options.batch.is_none());
        assert_eq!(options.viewport, Size::new(1200.0, 750.0));
        assert_eq!(options.mode, emulator::Mode::Zen);
    }

    #[test]
    fn run_mode_takes_a_directory_and_flags() {
        let options = parse(&args(&[
            "--harness-run",
            "tests/e2e",
            "--viewport",
            "800x600",
            "--mode",
            "immediate",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(options.batch.as_deref(), Some(std::path::Path::new("tests/e2e")));
        assert_eq!(options.viewport, Size::new(800.0, 600.0));
        assert_eq!(options.mode, emulator::Mode::Immediate);
    }

    #[test]
    fn harness_flags_without_a_mode_are_an_error() {
        assert!(parse(&args(&["--home", "/tmp/x"])).is_err());
    }

    #[test]
    fn repl_and_run_are_mutually_exclusive() {
        assert!(parse(&args(&["--harness-repl", "--harness-run", "d"])).is_err());
    }

    #[test]
    fn quoted_parsing() {
        assert_eq!(parse_quoted("\"New Host\""), Some("New Host".into()));
        assert_eq!(parse_quoted("Hosts"), Some("Hosts".into()));
        assert_eq!(parse_quoted(""), None);
    }
}
