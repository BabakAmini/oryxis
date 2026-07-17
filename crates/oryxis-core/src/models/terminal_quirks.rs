//! Per-host legacy keyboard modes and terminal feature toggles (C5).
//!
//! Someone on network appliances, AIX boxes or serial consoles needs the
//! terminal to speak the dialect the far end expects: what Backspace
//! sends, how Home/End and the function keys are encoded, and whether the
//! remote may drive mouse reporting, the window title, or the local
//! clipboard. These are per-host quirks; the defaults reproduce today's
//! xterm behaviour byte-for-byte (`TerminalQuirks::default()` ==
//! `DEFAULT_QUIRKS`), so an untouched host is unchanged.
//!
//! Encodings follow PuTTY's Keyboard-panel semantics (see the unit
//! vectors in `oryxis-app/src/util.rs`).

use serde::{Deserialize, Serialize};

/// What the Backspace key sends. Ctrl+Backspace conventionally sends the
/// *other* code (PuTTY's "Control-? / Control-H" swap), so each mode
/// defines both the plain and the Ctrl byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BackspaceMode {
    /// Plain Backspace = DEL (`0x7f`), Ctrl+Backspace = BS (`0x08`). The
    /// modern default and today's behaviour.
    #[default]
    Del127,
    /// Plain Backspace = BS (`0x08`), Ctrl+Backspace = DEL (`0x7f`). What
    /// older Unix / appliance line disciplines expect.
    CtrlH,
}

impl BackspaceMode {
    /// The byte Backspace sends for this mode, given whether Ctrl is held
    /// (the plain and Ctrl encodings are swapped between the two modes).
    pub fn byte(self, ctrl: bool) -> u8 {
        match (self, ctrl) {
            (BackspaceMode::Del127, false) => 0x7f,
            (BackspaceMode::Del127, true) => 0x08,
            (BackspaceMode::CtrlH, false) => 0x08,
            (BackspaceMode::CtrlH, true) => 0x7f,
        }
    }
}

/// How Home / End are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum HomeEndMode {
    /// xterm: Home/End are cursor-style keys (`CSI H` / `CSI F`, or the
    /// SS3 form under DECCKM application-cursor mode). Today's default.
    #[default]
    Standard,
    /// rxvt: Home = `ESC [ 7 ~`, End = `ESC [ 8 ~`, independent of
    /// application-cursor mode.
    Rxvt,
}

/// Function-key encoding flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FunctionKeyMode {
    /// xterm: F1-F4 as SS3 (`ESC O P..S`) with the CSI modified form when
    /// a modifier is held; F5-F12 as `ESC [ n ~`. Today's default.
    #[default]
    Xterm,
    /// Linux console: F1-F5 send `ESC [ [ A..E`; F6-F12 keep the xterm
    /// tilde form.
    LinuxConsole,
    /// VT400: F1-F4 always send SS3 (`ESC O P..S`) with no CSI modified
    /// form, even under a modifier.
    Vt400,
    /// rxvt: F1-F4 use the rxvt tilde numbers (`ESC [ 11 ~` .. `ESC [ 14
    /// ~`); F5-F12 keep the xterm tilde numbers.
    Rxvt,
}

/// Per-host override for OSC 52 clipboard access, layered over the global
/// `clipboard_access` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Osc52Override {
    /// Never let this host read or write the local clipboard, regardless
    /// of the global setting.
    Off,
    /// Let this host WRITE the local clipboard (the OSC 52 store the
    /// global default already permits), regardless of the global setting.
    /// Never enables clipboard READ: a remote reading the local clipboard
    /// is the exfiltration risk the global default refuses, and a per-host
    /// toggle must not silently re-enable it.
    On,
}

impl Osc52Override {
    /// Resolve to per-host `(write, read)` overrides, where `None` means
    /// "inherit the global policy" and `Some(bool)` forces that direction:
    ///
    /// - `On`  => `(Some(true), None)` — force write on; read still follows
    ///   the global (a per-host toggle never GRANTS clipboard read, the
    ///   exfiltration risk the global default refuses).
    /// - `Off` => `(Some(false), Some(false))` — block both directions for
    ///   this host, stricter than the global, so "Off" is a true block.
    pub fn overrides(self) -> (Option<bool>, Option<bool>) {
        match self {
            Osc52Override::On => (Some(true), None),
            Osc52Override::Off => (Some(false), Some(false)),
        }
    }
}

/// Which Option (Alt) keys act as Meta on macOS, sending `ESC <char>`
/// instead of letting the OS compose the AltGr-layer character. The
/// default lets both sides compose, matching Terminal.app / iTerm /
/// alacritty / kitty: on European Mac layouts Option is the only way to
/// type `|`, `{`, `}`, `[`, `]`, `@`. Meta is the opt-in for readline /
/// emacs users, with per-side granularity (alacritty's `option_as_alt`).
/// Ignored outside macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OptionAsMeta {
    /// Both Options compose characters (the macOS-native behaviour).
    #[default]
    None,
    /// Left Option is Meta, right Option composes (WezTerm's default).
    OnlyLeft,
    /// Right Option is Meta, left Option composes.
    OnlyRight,
    /// Both Options are Meta; no Option-layer characters can be typed.
    Both,
}

impl OptionAsMeta {
    /// Whether a press with the given Option side(s) held should be Meta.
    /// When both sides are down the Meta side wins (the user is holding a
    /// deliberate chord; composing would need the *other* side alone).
    pub fn is_meta(self, left: bool, right: bool) -> bool {
        match self {
            OptionAsMeta::None => false,
            OptionAsMeta::OnlyLeft => left,
            OptionAsMeta::OnlyRight => right,
            OptionAsMeta::Both => left || right,
        }
    }
}

/// Per-host legacy keyboard modes + feature toggles. All fields default
/// to today's behaviour; `TerminalQuirks::default()` is the identity
/// (`DEFAULT_QUIRKS`), so an untouched host encodes exactly as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TerminalQuirks {
    #[serde(default)]
    pub backspace: BackspaceMode,
    #[serde(default)]
    pub home_end: HomeEndMode,
    #[serde(default)]
    pub function_keys: FunctionKeyMode,
    /// Ignore remote mouse-tracking requests: clicks always select/paste
    /// locally even when the remote enabled mouse reporting.
    #[serde(default)]
    pub disable_mouse_reporting: bool,
    /// Ignore remote window-title changes (OSC 0/2): the tab keeps its
    /// connection label / manual rename, and the OSC-title cwd fallback is
    /// suppressed too.
    #[serde(default)]
    pub disable_title_change: bool,
    /// Per-host OSC 52 clipboard-WRITE override; `None` follows the global
    /// `clipboard_access` setting. Read is never per-host (see
    /// [`Osc52Override::allows_write`]).
    #[serde(default)]
    pub osc52: Option<Osc52Override>,
    /// macOS: which Option keys act as Meta instead of composing (see
    /// [`OptionAsMeta`]). Stored per-host like every other quirk so it
    /// rides sync / export; non-macOS platforms ignore it.
    #[serde(default)]
    pub option_as_meta: OptionAsMeta,
}

/// The identity quirks: today's xterm behaviour. Call sites without a
/// connection (local shells) use this so their encoding is unchanged.
pub const DEFAULT_QUIRKS: TerminalQuirks = TerminalQuirks {
    backspace: BackspaceMode::Del127,
    home_end: HomeEndMode::Standard,
    function_keys: FunctionKeyMode::Xterm,
    disable_mouse_reporting: false,
    disable_title_change: false,
    osc52: None,
    option_as_meta: OptionAsMeta::None,
};

// ── Display impls feed the host editor's pick_list mappers directly
//    (the fork's 4-step `pick_list` API renders via `|m| m.to_string()`).
//    English strings; the app layer maps these to i18n labels for the
//    actual picker, mirroring `auth_method_label`. ──

impl std::fmt::Display for BackspaceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BackspaceMode::Del127 => "Control-? (127)",
            BackspaceMode::CtrlH => "Control-H (8)",
        })
    }
}

impl std::fmt::Display for HomeEndMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HomeEndMode::Standard => "Standard",
            HomeEndMode::Rxvt => "rxvt",
        })
    }
}

impl std::fmt::Display for FunctionKeyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FunctionKeyMode::Xterm => "Xterm",
            FunctionKeyMode::LinuxConsole => "Linux",
            FunctionKeyMode::Vt400 => "VT400",
            FunctionKeyMode::Rxvt => "rxvt",
        })
    }
}

impl std::fmt::Display for OptionAsMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OptionAsMeta::None => "Off",
            OptionAsMeta::OnlyLeft => "Left Option",
            OptionAsMeta::OnlyRight => "Right Option",
            OptionAsMeta::Both => "Both",
        })
    }
}
