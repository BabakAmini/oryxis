//! Headless end-to-end harness (feature `harness`).
//!
//! Runs the real application (vault, subscriptions, SSH tasks, side
//! effects) inside `iced_test`'s [`Emulator`], with no window and no
//! display server, and exposes three entry points parsed from the CLI
//! before anything else in `main()`:
//!
//! - `oryxis --harness-run <dir>`: batch-runs every `.ice` test file
//!   in `<dir>` (see [`iced_test::Ice`] for the format). A failing
//!   instruction dumps a PNG screenshot plus a reproduction `.ice`
//!   into `<dir>/errors/` and exits non-zero. This is the CI mode.
//! - `oryxis --harness-repl`: an interactive line protocol on
//!   stdin/stdout for driving the app step by step (see `repl.rs`).
//! - `oryxis --harness-mcp`: an MCP (Model Context Protocol) server
//!   over stdio exposing the same driving surface as typed tools, so
//!   an AI agent can interact with the app, validate it visually
//!   (screenshots come back inline as image content) and save the
//!   interaction as a replayable `.ice` test (see `mcp.rs`).
//!
//! Both interactive modes execute `.ice` instructions (`click
//! "Hosts"`, `type "ls"`, `type enter`, `type ctrl+shift+f`,
//! `scroll (0, -3)`, `expect "Connected"`, `move (100, 200)`, ...)
//! and record the ones that execute into a session history that
//! `save_ice` turns into a test file.
//!
//! Isolation: every mode redirects `$HOME` to a sandbox directory
//! (default `<tmp>/oryxis-harness`, override with `--home <dir>`)
//! *before* anything reads the vault, so a harness run can never
//! touch the real `~/.oryxis`. The sandbox persists across runs by
//! design: a master password set in one session is still there in
//! the next, which keeps iterative QA cheap. `reset` (REPL command /
//! MCP tool) reboots the emulated app in place, optionally wiping
//! the sandbox's `.oryxis` first for a first-run state.
//!
//! Flags shared by all modes: `--home <dir>`, `--shots <dir>`,
//! `--viewport <WxH>` (default 1200x750), `--scale <factor>`
//! (default 1), `--mode zen|patient|immediate` (default zen, see
//! [`emulator::Mode`]) and `--timeout-ms <ms>` (default 20000).
//!
//! Fonts: the emulator path never runs the shell's boot-time font
//! loading, so [`run`] pushes `fonts::BUNDLED_FONTS` straight into
//! the global font system before booting; without this every icon
//! renders as tofu and text selectors still match but screenshots
//! are useless.

mod mcp;
mod repl;

use std::borrow::Cow;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::{Program, Size};
use iced_test::Instruction;
use iced_test::core::Rectangle;
use iced_test::core::theme;
use iced_test::core::widget::Id;
use iced_test::core::widget::operation::Operation;
use iced_test::emulator::{self, Emulator, Event};
use iced_test::futures::futures::channel::mpsc::{self, TryRecvError};

/// How long the initial boot (vault open, font tasks, update check)
/// may take before the interactive modes give up waiting and hand
/// control over anyway. Generous because a cold tokio +
/// headless-renderer start under a software rasterizer can be slow.
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);

/// Which harness front-end was requested on the CLI.
enum Frontend {
    /// `--harness-repl`: line protocol on stdin/stdout.
    Repl,
    /// `--harness-run <dir>`: batch `.ice` runner.
    Batch(PathBuf),
    /// `--harness-mcp`: MCP server over stdio.
    Mcp,
}

/// A parsed harness invocation. Returned by [`options_from_args`]
/// and consumed by [`run`].
pub struct Options {
    frontend: Frontend,
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
    /// Per-instruction completion timeout in the interactive modes.
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
    let mut mcp = false;
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
            "--harness-mcp" => mcp = true,
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

    let requested = usize::from(repl) + usize::from(mcp) + usize::from(batch.is_some());
    if requested == 0 {
        if let Some(flag) = harness_flags.first() {
            return Err(format!(
                "{flag} only makes sense with --harness-repl, --harness-mcp or --harness-run"
            ));
        }
        return Ok(None);
    }
    if requested > 1 {
        return Err(
            "--harness-repl, --harness-mcp and --harness-run are mutually exclusive".into(),
        );
    }

    let frontend = if let Some(dir) = batch {
        Frontend::Batch(dir)
    } else if mcp {
        Frontend::Mcp
    } else {
        Frontend::Repl
    };

    let home = home.unwrap_or_else(|| std::env::temp_dir().join("oryxis-harness"));
    let shots = shots.unwrap_or_else(|| home.join("shots"));

    Ok(Some(Options {
        frontend,
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

    match &options.frontend {
        Frontend::Batch(dir) => {
            let dir = dir.clone();
            match iced_test::run(program, &dir) {
                Ok(()) => {
                    println!("== ok all ice tests passed in {}", dir.display());
                    Ok(())
                }
                Err(error) => {
                    eprintln!("oryxis harness: ice run failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        Frontend::Repl => repl::serve(program, options),
        Frontend::Mcp => mcp::serve(program, options),
    }
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

/// Outcome of executing one instruction line.
enum RunOutcome {
    /// Executed and quiesced.
    Done,
    /// The instruction could not be executed (target not found /
    /// expectation not met).
    Failed(Instruction),
    /// Executed, but tasks were still pending when the instruction
    /// timeout passed (normal once a live terminal session exists).
    Timeout,
    /// The emulator died.
    Closed,
    /// The line is not a valid `.ice` instruction.
    Parse(String),
}

/// One interactive session over a booted emulator, shared by the
/// REPL and MCP front-ends.
struct Session<P>
where
    P: Program + 'static,
{
    emulator: Emulator<P>,
    receiver: mpsc::Receiver<Event<P>>,
    home: PathBuf,
    shots: PathBuf,
    viewport: Size,
    scale: f32,
    mode: emulator::Mode,
    timeout: Duration,
    shot_counter: u32,
    /// Every instruction that executed (including timed-out ones,
    /// which did run; excluding failures and parse errors), in
    /// order. [`Session::save_ice`] turns this into a test file.
    history: Vec<Instruction>,
}

impl<P> Session<P>
where
    P: Program + 'static,
{
    /// Boots the emulated application and pumps until the boot task
    /// settles (or [`BOOT_TIMEOUT`] passes).
    fn new(program: &P, options: &Options) -> (Self, Pump) {
        let (sender, receiver) = mpsc::channel(256);
        let mut session = Self {
            emulator: Emulator::new(sender, program, options.mode, options.viewport),
            receiver,
            home: options.home.clone(),
            shots: options.shots.clone(),
            viewport: options.viewport,
            scale: options.scale,
            mode: options.mode,
            timeout: options.timeout,
            shot_counter: 0,
            history: Vec::new(),
        };
        let boot = session.pump_until_ready(program, BOOT_TIMEOUT);
        (session, boot)
    }

    /// Reboots the emulated application in place. With `wipe`, the
    /// sandbox's `.oryxis` directory is removed first so the app
    /// comes back in its first-run state (onboarding). Clears the
    /// instruction history.
    ///
    /// The old emulator (and its runtime) is dropped before the new
    /// one boots; on Unix the wipe also works while the old vault
    /// handle is still open, since unlinking open files is allowed.
    fn reset(&mut self, program: &P, wipe: bool) -> Result<Pump, String> {
        if wipe {
            let oryxis_dir = self.home.join(".oryxis");
            if oryxis_dir.exists() {
                std::fs::remove_dir_all(&oryxis_dir)
                    .map_err(|e| format!("wiping {}: {e}", oryxis_dir.display()))?;
            }
        }

        let (sender, receiver) = mpsc::channel(256);
        self.receiver = receiver;
        self.emulator = Emulator::new(sender, program, self.mode, self.viewport);
        self.history.clear();
        Ok(self.pump_until_ready(program, BOOT_TIMEOUT))
    }

    /// Parses and executes one `.ice` instruction line, recording it
    /// in the session history when it actually ran.
    fn run_line(&mut self, program: &P, line: &str) -> RunOutcome {
        let instruction = match Instruction::parse(line) {
            Ok(instruction) => instruction,
            Err(error) => return RunOutcome::Parse(error.to_string()),
        };

        self.emulator.run(program, &instruction);
        match self.pump_until_ready(program, self.timeout) {
            Pump::Ready => {
                self.history.push(instruction);
                RunOutcome::Done
            }
            Pump::Timeout => {
                self.history.push(instruction);
                RunOutcome::Timeout
            }
            Pump::Failed(instruction) => RunOutcome::Failed(instruction),
            Pump::Closed => RunOutcome::Closed,
        }
    }

    /// Serializes the session history as an `.ice` test file and
    /// writes it to `path`, returning the content.
    fn save_ice(&self, path: &std::path::Path) -> Result<String, String> {
        use std::fmt::Write as _;

        if self.history.is_empty() {
            return Err("no instructions recorded yet".into());
        }

        let mut content = String::new();
        let _ = writeln!(
            content,
            "viewport: {}x{}",
            self.viewport.width as u32, self.viewport.height as u32
        );
        let _ = writeln!(content, "mode: {}", self.mode);
        let _ = writeln!(content, "-----");
        for instruction in &self.history {
            let _ = writeln!(content, "{instruction}");
        }

        std::fs::write(path, &content).map_err(|e| format!("writing {}: {e}", path.display()))?;
        Ok(content)
    }

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

    /// Renders the current UI straight from the emulator's own
    /// renderer and widget-state cache (scroll offsets, focus,
    /// carets all included), writes it as a PNG under the shots
    /// directory and returns both the path and the encoded bytes.
    fn screenshot(&mut self, program: &P, name: &str) -> Result<(PathBuf, Vec<u8>), String> {
        self.shot_counter += 1;
        let name = if name.is_empty() {
            format!("shot-{:04}", self.shot_counter)
        } else {
            sanitize_name(name)
        };

        let theme = self
            .emulator
            .theme(program)
            .unwrap_or_else(|| <P::Theme as theme::Base>::default(theme::Mode::None));
        let shot = self.emulator.screenshot(program, &theme, self.scale);

        let image =
            image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec())
                .ok_or("screenshot buffer size mismatch")?;
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;

        let path = self.shots.join(format!("{name}.png"));
        std::fs::write(&path, &png).map_err(|e| e.to_string())?;
        Ok((path, png))
    }

    /// Collects every visible text widget with its bounds, sorted in
    /// reading order, from the emulator's live widget tree.
    fn texts(&mut self, program: &P) -> Result<Vec<(String, Rectangle)>, String> {
        let mut dump = DumpTexts::default();
        self.emulator.operate(program, &mut dump);

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
        assert!(matches!(options.frontend, Frontend::Repl));
        assert_eq!(options.viewport, Size::new(1200.0, 750.0));
        assert_eq!(options.mode, emulator::Mode::Zen);
    }

    #[test]
    fn mcp_mode_parses() {
        let options = parse(&args(&["--harness-mcp"])).unwrap().unwrap();
        assert!(matches!(options.frontend, Frontend::Mcp));
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
        assert!(matches!(
            &options.frontend,
            Frontend::Batch(dir) if dir == std::path::Path::new("tests/e2e")
        ));
        assert_eq!(options.viewport, Size::new(800.0, 600.0));
        assert_eq!(options.mode, emulator::Mode::Immediate);
    }

    #[test]
    fn harness_flags_without_a_mode_are_an_error() {
        assert!(parse(&args(&["--home", "/tmp/x"])).is_err());
    }

    #[test]
    fn frontends_are_mutually_exclusive() {
        assert!(parse(&args(&["--harness-repl", "--harness-run", "d"])).is_err());
        assert!(parse(&args(&["--harness-repl", "--harness-mcp"])).is_err());
        assert!(parse(&args(&["--harness-mcp", "--harness-run", "d"])).is_err());
    }

    #[test]
    fn quoted_parsing() {
        assert_eq!(parse_quoted("\"New Host\""), Some("New Host".into()));
        assert_eq!(parse_quoted("Hosts"), Some("Hosts".into()));
        assert_eq!(parse_quoted(""), None);
    }
}
