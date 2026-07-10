# Headless E2E harness

Oryxis ships a headless end-to-end harness behind the `harness` cargo
feature. It runs the **real application**, real vault, real
subscriptions, real SSH side effects, inside `iced_test`'s `Emulator`
(from the same wilsonglasser/iced fork the app renders with), with no
window and no display server, and renders PNG screenshots on demand.
Think "Playwright for the iced UI".

The feature is dev-only: release and CI artifact builds never enable
it, so it adds zero weight to shipped binaries.

## Isolation

Both modes redirect `$HOME` (and `%USERPROFILE%` on Windows) to a
sandbox directory **before anything reads the vault**, so a harness
run can never touch your real `~/.oryxis`.

- Default sandbox: `<system tmp>/oryxis-harness`. It persists across
  runs on purpose: a master password set in one session is still
  there in the next, which keeps iterative QA cheap.
- Override with `--home <dir>`. Batch runs in CI should always pass a
  fresh directory so first-boot flows (onboarding) are reproducible.

## Batch mode (CI): `--harness-run <dir>`

```bash
cargo run -p oryxis-app --features harness -- \
    --harness-run crates/oryxis-app/tests/e2e --home "$(mktemp -d)"
```

Runs every `.ice` file in `<dir>`. A failing instruction dumps a PNG
screenshot plus a truncated reproduction `.ice` into `<dir>/errors/`
and exits non-zero.

The `.ice` format (experimental upstream, syntax may change):

```text
viewport: 1200x750
mode: Zen
-----
expect "Welcome to Oryxis"
click "Skip"
expect "Protect your vault"
click "Continue without password"
expect "Create host"
```

`mode` is required: `Zen` waits for all tasks an instruction spawns
(including indirect ones), `Patient` only for direct ones,
`Immediate` never waits. See `crates/oryxis-app/tests/e2e/` for the
committed suite.

## Interactive mode (agent/manual QA): `--harness-repl`

```bash
cargo run -p oryxis-app --features harness -- --harness-repl
```

A line protocol on stdin/stdout. Every response line is prefixed with
`== ` so it can be told apart from tracing output on the same stream.
A convenient way to drive it from another process is a `tail -f`'d
command file:

```bash
: > /tmp/cmds.txt
tail -f -n +1 /tmp/cmds.txt | oryxis --harness-repl > /tmp/out.log 2> /tmp/err.log &
echo 'screenshot boot' >> /tmp/cmds.txt
grep '^== ' /tmp/out.log
```

### Commands

Any `.ice` instruction works as a command:

| Command | Meaning |
|---------|---------|
| `click "Text"` / `click #id` / `click (x, y)` | click a target (`click right ...` for right-click) |
| `press` / `release` / `move <target>` | lower-level mouse steps |
| `type "some text"` | typewrite into the focused widget |
| `type enter` / `escape` / `tab` / `backspace` | named keys |
| `expect "Text"` | fail unless a widget currently shows `Text` |

Plus harness meta-commands:

| Command | Meaning |
|---------|---------|
| `screenshot [name]` | render the UI to `<shots>/<name>.png`, print the path |
| `texts` | dump every visible text widget with bounds (reading order) |
| `find "Text"` | like `texts`, filtered to matches |
| `wait <ms>` | pump emulator events for a fixed duration |
| `settle [idle_ms]` | pump until the event stream stays quiet (default 250 ms, 30 s cap) |
| `timeout <ms>` | set the per-instruction completion timeout (default 20 s) |
| `help` / `quit` | self-explanatory |

Responses: `== ok`, `== fail <instruction>`, `== timeout ...`,
`== shot <path>`, `== error <reason>`, plus `== text ...` entry lines
for `texts`/`find`. Lines starting with `#` and blank lines are
ignored, so a command file can be annotated.

### Flags (both modes)

| Flag | Default | Meaning |
|------|---------|---------|
| `--home <dir>` | `<tmp>/oryxis-harness` | sandbox `$HOME` |
| `--shots <dir>` | `<home>/shots` | where screenshots land |
| `--viewport <WxH>` | `1200x750` | logical window size |
| `--scale <f>` | `1` | screenshot scale factor (0.25..=4) |
| `--mode zen\|patient\|immediate` | `zen` | task-waiting strategy |
| `--timeout-ms <ms>` | `20000` | REPL per-instruction timeout |

## How it works / limitations

- The emulator boots `Oryxis::boot` through the same
  `iced::application(...)` builder `main()` uses (fonts, theme,
  subscriptions), so behavior matches the windowed app. Tray,
  single-instance IPC and the window itself are skipped.
- Rendering picks wgpu-headless when a GPU adapter exists and falls
  back to tiny-skia (CPU) otherwise, no display needed either way.
- Text selectors see iced text widgets only. The terminal grid is a
  custom canvas, so `expect` cannot match terminal output; verify
  terminal content visually via `screenshot`. Typing into the PTY
  works normally (events flow through the widget).
- `screenshot`/`texts`/`find` render through a harness-owned probe
  renderer + widget-state cache, not the emulator's (upstream
  `Emulator::screenshot` loses the widget cache, poisoning the next
  instruction; the probe pair sidesteps that). Consequence: widget
  state that lives outside app state, scroll offsets, focus rings,
  carets, does not show in screenshots. App-state-driven content
  (values, layout, text) is always truthful.
- Clipboard and a few runtime actions are still TODO upstream in the
  emulator; instructions relying on them will time out rather than
  work.
- Real window/WM concerns (multi-monitor geometry, DPI, tray) stay
  manual QA.

## Recording tests from the real app

The iced fork also ships a `tester` feature (`iced/tester`) that
overlays a record/play panel on F12 in a real windowed run. It can
record interactions into `.ice` files that this harness replays
headless. Not wired into a cargo feature here yet; enable it ad hoc
by adding `"tester"` to the iced features in the workspace
`Cargo.toml` for a local dev build.
