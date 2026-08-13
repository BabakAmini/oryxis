//! Pure keypress -> PTY byte encoding (no UI, no state).
//!
//! One entry point, [`pty_bytes`], turns an iced `KeyPressed` event into
//! the byte sequence a terminal writes, honoring the per-host C5 quirks
//! and the platform's modifier dialect. The platform is a parameter (not
//! `cfg!`) so the whole decision table is unit-testable from any host.
//!
//! The load-bearing rule (issue #80): a keypress that COMPOSED a
//! printable character (AltGr on Windows/Linux, Option on macOS) is text
//! input, not a chord. The composed character lives in the event's
//! `text` (winit `text_with_all_modifiers`) and `modified_key` (winit
//! `logical_key`); the bare `key` is `key_without_modifiers` and never
//! carries it. Concretely:
//!
//! - Windows reports AltGr as Ctrl+Alt. Ctrl+Alt with composed text is
//!   AltGr typing (bepo AltGr+Space = `_`, German AltGr+Q = `@`); the
//!   old encoder fed it to the Ctrl path and emitted control bytes.
//! - Linux reports AltGr as no modifier at all; composition already
//!   arrives in `text`, but `Named::Space` used to outrank it, eating
//!   bepo's `_`.
//! - macOS composes via Option. Whether a given Option side is Meta
//!   (`ESC <char>`) or composes is the per-host
//!   [`OptionAsMeta`](oryxis_core::models::terminal_quirks::OptionAsMeta)
//!   quirk; the default composes, matching every macOS terminal.
//!
//! Application-keypad mode (DECPAM, `ESC =`) is a deliberate non-goal:
//! the numpad always sends what it types (digits, operators, and CR for
//! its Enter). xterm's SS3 keypad forms (`ESC O q` for 1, `ESC O M` for
//! Enter) are what stops a numpad from typing digits inside anything
//! that enables `smkx`, and the emulator we embed (alacritty) doesn't
//! send them either.

use iced::keyboard;
use oryxis_core::models::terminal_quirks::{FunctionKeyMode, HomeEndMode, TerminalQuirks};

/// Which OS dialect of modifiers the event was produced under. Runtime
/// callers use [`Platform::current`]; tests exercise all three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    MacOs,
    Linux,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }
}

/// Which Option (Alt) keys are physically held, tracked by the app from
/// the Alt keypress/release events (iced's `Modifiers` can't tell the
/// sides apart). Only consulted on macOS for the `OptionAsMeta` quirk.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct OptionSides {
    pub left: bool,
    pub right: bool,
}

/// The slice of a `KeyPressed` event the encoder consumes.
pub(crate) struct KeyPress<'a> {
    /// winit `key_without_modifiers`: the base key, no AltGr/Option layer.
    pub key: &'a keyboard::Key,
    /// winit `logical_key`: the key with all modifiers except Ctrl
    /// applied, i.e. the composed character when there is one.
    pub modified_key: &'a keyboard::Key,
    pub modifiers: keyboard::Modifiers,
    /// winit `text_with_all_modifiers`: the OS-composed input text.
    pub text: Option<&'a str>,
    pub location: keyboard::Location,
}

/// The printable text this press composed, if any. `text` is authoritative
/// (it is what the OS says the press typed); `modified_key` is the
/// fallback for platforms that drop `text` under a held modifier. Control
/// characters are rejected: Ctrl+letter arrives as a C0 byte in `text`
/// and is a chord, not composition.
fn composed_text(press: &KeyPress<'_>) -> Option<String> {
    if let Some(t) = press.text
        && !t.is_empty()
        && !t.chars().any(char::is_control)
    {
        return Some(t.to_string());
    }
    if let keyboard::Key::Character(m) = press.modified_key
        && !m.chars().any(char::is_control)
        && press.modified_key != press.key
    {
        return Some(m.to_string());
    }
    None
}

/// Translate one keypress into PTY bytes. `None` means "write nothing"
/// (a swallowed chord, a bare modifier, a key with no encoding).
///
/// The caller keeps the two decisions that need app state: Ctrl+C's
/// interrupt short-circuit and Ctrl+D closing a dead tab; both are
/// Ctrl-without-Alt and must run before this.
pub(crate) fn pty_bytes(
    press: &KeyPress<'_>,
    platform: Platform,
    app_cursor: bool,
    quirks: &TerminalQuirks,
    option_sides: OptionSides,
) -> Option<Vec<u8>> {
    let mods = press.modifiers;

    // macOS: Cmd is the platform's shortcut modifier; every unregistered
    // Cmd combo is swallowed so it never leaks into the PTY as text
    // (registered ones were consumed upstream by the hotkey table).
    if platform == Platform::MacOs && mods.logo() && !mods.control() && !mods.alt() {
        return None;
    }

    // Ctrl+Alt BEFORE the Ctrl paths: on Windows this is how AltGr
    // arrives, and AltGr+Shift lands here too (level-4 characters), so
    // this must also outrank the Ctrl+Shift swallow below. With composed
    // text that differs from the base character it is typing; without,
    // it is a deliberate Ctrl+Alt chord and gets xterm's ESC-prefixed
    // control byte.
    if mods.control() && mods.alt() {
        if let Some(t) = composed_text(press) {
            let base = match press.key {
                keyboard::Key::Character(c) => Some(c.as_str()),
                _ => None,
            };
            if base != Some(t.as_str()) {
                return Some(t.into_bytes());
            }
        }
        if let Some(ctrl) = ctrl_key_bytes(press.key) {
            let mut esc = Vec::with_capacity(ctrl.len() + 1);
            esc.push(0x1b);
            esc.extend_from_slice(&ctrl);
            return Some(esc);
        }
        return key_to_named_bytes(press.key, &mods, app_cursor, quirks);
    }

    // Ctrl+Shift + a cursor / editing / function key gets the xterm
    // modified-key sequence every real terminal sends (Ctrl+Shift+Left =
    // ESC[1;6D; param 6 = base 1 + shift 1 + ctrl 4), so a TUI that reads
    // modifiers sees the combo instead of nothing. Any Ctrl+Shift chord
    // bound to an app action (the clipboard trio, palette, files, ...)
    // was already consumed upstream by the hotkey table / widget, so a
    // Ctrl+Shift press that reaches here is unbound. Everything else under
    // Ctrl+Shift (letters, Enter, Tab, Space, Backspace, Escape) is not a
    // terminal sequence and stays swallowed, as before.
    if mods.control() && mods.shift() {
        if matches!(
            press.key,
            keyboard::Key::Named(
                keyboard::key::Named::ArrowUp
                    | keyboard::key::Named::ArrowDown
                    | keyboard::key::Named::ArrowLeft
                    | keyboard::key::Named::ArrowRight
                    | keyboard::key::Named::Home
                    | keyboard::key::Named::End
                    | keyboard::key::Named::PageUp
                    | keyboard::key::Named::PageDown
                    | keyboard::key::Named::Insert
                    | keyboard::key::Named::Delete
                    | keyboard::key::Named::F1
                    | keyboard::key::Named::F2
                    | keyboard::key::Named::F3
                    | keyboard::key::Named::F4
                    | keyboard::key::Named::F5
                    | keyboard::key::Named::F6
                    | keyboard::key::Named::F7
                    | keyboard::key::Named::F8
                    | keyboard::key::Named::F9
                    | keyboard::key::Named::F10
                    | keyboard::key::Named::F11
                    | keyboard::key::Named::F12
            )
        ) {
            return key_to_named_bytes(press.key, &mods, app_cursor, quirks);
        }
        return None;
    }

    // Ctrl+letter -> the shell's control byte; Ctrl + named key (e.g.
    // Ctrl+Home, Ctrl+Space = NUL) folds the modifier in the named
    // encoding.
    if mods.control() {
        if let Some(ctrl) = ctrl_key_bytes(press.key) {
            return Some(ctrl);
        }
        return key_to_named_bytes(press.key, &mods, app_cursor, quirks);
    }

    // A Space that composed something else is typing, not Space: on
    // Linux, bepo's AltGr+Space (`_`) arrives with NO modifiers at all
    // (level 3 is not a reported modifier), so only the composed text
    // betrays it. Alt/Ctrl presses were handled above or belong to the
    // Meta path below.
    if !mods.alt()
        && matches!(press.key, keyboard::Key::Named(keyboard::key::Named::Space))
        && let Some(t) = composed_text(press).filter(|t| t != " ")
    {
        return Some(t.into_bytes());
    }

    // Iced's `key` is the key WITHOUT modifiers, so a numpad keypress
    // with NumLock on still shows up as Named::Home / ArrowUp / etc.
    // while the OS-produced `text` is "7" / "8". Prefer the text on
    // numpad so NumLock-on sends digits.
    //
    // Control characters are NOT typing and never win over the named
    // encoding (issue #162): macOS reports the keypad Enter's text as
    // the Cocoa `NSEnterCharacter` (U+0003), so the preference handed
    // the PTY an ETX (^C, "interrupt") instead of the CR the same key
    // sends on every other platform, discarding the line the user had
    // just typed. Rejecting control text makes numpad Enter resolve
    // through `key_to_named_bytes` like the main Enter, on every
    // platform, whatever the OS put in `text`.
    let numpad_text = if press.location == keyboard::Location::Numpad {
        press
            .text
            .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
            .map(|t| t.as_bytes().to_vec())
    } else {
        None
    };
    let mut bytes = numpad_text
        .or_else(|| key_to_named_bytes(press.key, &mods, app_cursor, quirks))
        .or_else(|| press.text.map(|t| t.as_bytes().to_vec()));

    // Meta-sends-escape: Alt+<char> is the ESC-prefixed character, the
    // form readline / bash / zsh / vim / emacs / tmux bind their Meta
    // keymaps to. On Linux/Windows a bare Alt is always Meta (AltGr
    // never reaches here: it is Ctrl+Alt on Windows and no modifier on
    // Linux). On macOS the Option key COMPOSES by default and Meta is
    // the per-host `OptionAsMeta` quirk, per side. Named keys already
    // fold their modifier in via key_to_named_bytes, so this only
    // touches literal characters.
    if mods.alt() {
        let meta = match platform {
            Platform::MacOs => quirks
                .option_as_meta
                .is_meta(option_sides.left, option_sides.right),
            _ => true,
        };
        if meta {
            if let keyboard::Key::Character(c) = press.key {
                let ch: Vec<u8> = if platform == Platform::MacOs {
                    // The Option layer must not leak into the Meta
                    // character (ESC + the composed char is useless to
                    // readline): use the base key, uppercased under
                    // Shift so M-B still works.
                    if mods.shift() {
                        c.to_uppercase().into_bytes()
                    } else {
                        c.as_str().as_bytes().to_vec()
                    }
                } else {
                    // Some platforms drop `text` while Alt is held; fall
                    // back to the key's base character so Alt+b still
                    // emits `ESC b`.
                    bytes
                        .take()
                        .filter(|b| !b.is_empty())
                        .unwrap_or_else(|| c.as_str().as_bytes().to_vec())
                };
                let mut esc = Vec::with_capacity(ch.len() + 1);
                esc.push(0x1b);
                esc.extend_from_slice(&ch);
                bytes = Some(esc);
            }
        } else if let Some(t) = composed_text(press) {
            // Composing Option: the OS-composed character IS the input
            // (German Mac Option+7 = `|`, Option+Space = NBSP, ...).
            return Some(t.into_bytes());
        }
    }

    bytes.filter(|b| !b.is_empty())
}

/// xterm modifier parameter: `1 + Shift(1) + Alt(2) + Ctrl(4)`. Returns 1 when
/// no modifier is held (the "unmodified" sentinel xterm uses in `CSI 1 ; N …`).
fn xterm_modifier_param(m: &keyboard::Modifiers) -> u8 {
    1 + (m.shift() as u8) + 2 * (m.alt() as u8) + 4 * (m.control() as u8)
}

/// Translate a named iced key (Enter, Tab, ArrowUp, …) into the PTY byte
/// sequence, honoring modifiers in the xterm scheme.
///
/// Cursor / Home / End keys take the SS3 form (`ESC O A`) under application-
/// cursor-keys mode (DECCKM) and the CSI form (`ESC [ A`) otherwise, but a
/// *modified* press is always CSI with a parameter (`ESC [ 1 ; 5 A` for
/// Ctrl+Up), the form vim / readline / editors bind word-jump and selection
/// to. The `~`-terminated keys (PageUp/Down, Insert, Delete, F5-F12) carry the
/// modifier as `ESC [ N ; M ~`. F1-F4 go from `ESC O P` to `ESC [ 1 ; M P`
/// when modified. Shift+Tab is the back-tab `ESC [ Z`.
pub(crate) fn key_to_named_bytes(
    key: &keyboard::Key,
    modifiers: &keyboard::Modifiers,
    app_cursor: bool,
    quirks: &TerminalQuirks,
) -> Option<Vec<u8>> {
    let keyboard::Key::Named(named) = key else {
        return None;
    };
    let param = xterm_modifier_param(modifiers);
    let modified = param > 1;

    // Arrows + Home/End: `letter` is the CSI/SS3 final byte.
    let csi_letter = |letter: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{}{}", param, letter as char).into_bytes()
        } else if app_cursor {
            vec![0x1b, b'O', letter]
        } else {
            vec![0x1b, b'[', letter]
        }
    };
    // `~`-terminated keys (PageUp/Down, Insert, Delete, F5-F12, and the
    // rxvt Home/End + F1-F4 numbers).
    let tilde = |num: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[{num};{param}~").into_bytes()
        } else {
            format!("\x1b[{num}~").into_bytes()
        }
    };
    // F1-F4 (xterm): SS3 final byte unmodified, CSI with parameter when
    // modified.
    let ss3_fn = |letter: u8| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{}{}", param, letter as char).into_bytes()
        } else {
            vec![0x1b, b'O', letter]
        }
    };
    // VT400 F1-F4: SS3 always, no CSI modified form even under a modifier.
    let ss3_always = |letter: u8| -> Vec<u8> { vec![0x1b, b'O', letter] };
    // Linux console F1-F5: `ESC [ [ A..E`, no modified form.
    let linux_fn = |letter: u8| -> Vec<u8> { vec![0x1b, b'[', b'[', letter] };

    use keyboard::key::Named;
    let bytes: Vec<u8> = match named {
        Named::Enter => b"\r".to_vec(),
        // Backspace encoding is per-host (PuTTY Control-? vs Control-H);
        // Ctrl+Backspace sends the flipped code. DEFAULT (`Del127`) keeps
        // plain Backspace = DEL (0x7f), today's behaviour.
        Named::Backspace => vec![quirks.backspace.byte(modifiers.control())],
        // Shift+Tab is back-tab (CBT); plain Tab stays HT.
        Named::Tab if modifiers.shift() => b"\x1b[Z".to_vec(),
        Named::Tab => b"\t".to_vec(),
        Named::Escape => b"\x1b".to_vec(),
        // Ctrl+Space is NUL (xterm), the emacs set-mark muscle memory.
        Named::Space if modifiers.control() => vec![0x00],
        Named::Space => b" ".to_vec(),
        Named::ArrowUp => csi_letter(b'A'),
        Named::ArrowDown => csi_letter(b'B'),
        Named::ArrowRight => csi_letter(b'C'),
        Named::ArrowLeft => csi_letter(b'D'),
        // Home/End: xterm cursor-style (default) or rxvt tilde form
        // (`ESC[7~` / `ESC[8~`, app-cursor-independent).
        Named::Home => match quirks.home_end {
            HomeEndMode::Standard => csi_letter(b'H'),
            HomeEndMode::Rxvt => tilde(7),
        },
        Named::End => match quirks.home_end {
            HomeEndMode::Standard => csi_letter(b'F'),
            HomeEndMode::Rxvt => tilde(8),
        },
        Named::PageUp => tilde(5),
        Named::PageDown => tilde(6),
        Named::Insert => tilde(2),
        Named::Delete => tilde(3),
        // F1-F4 vary by mode; F5 too (Linux console); F6-F12 are the
        // xterm tilde form in every mode.
        Named::F1 => match quirks.function_keys {
            FunctionKeyMode::Xterm => ss3_fn(b'P'),
            FunctionKeyMode::Vt400 => ss3_always(b'P'),
            FunctionKeyMode::LinuxConsole => linux_fn(b'A'),
            FunctionKeyMode::Rxvt => tilde(11),
        },
        Named::F2 => match quirks.function_keys {
            FunctionKeyMode::Xterm => ss3_fn(b'Q'),
            FunctionKeyMode::Vt400 => ss3_always(b'Q'),
            FunctionKeyMode::LinuxConsole => linux_fn(b'B'),
            FunctionKeyMode::Rxvt => tilde(12),
        },
        Named::F3 => match quirks.function_keys {
            FunctionKeyMode::Xterm => ss3_fn(b'R'),
            FunctionKeyMode::Vt400 => ss3_always(b'R'),
            FunctionKeyMode::LinuxConsole => linux_fn(b'C'),
            FunctionKeyMode::Rxvt => tilde(13),
        },
        Named::F4 => match quirks.function_keys {
            FunctionKeyMode::Xterm => ss3_fn(b'S'),
            FunctionKeyMode::Vt400 => ss3_always(b'S'),
            FunctionKeyMode::LinuxConsole => linux_fn(b'D'),
            FunctionKeyMode::Rxvt => tilde(14),
        },
        Named::F5 => match quirks.function_keys {
            FunctionKeyMode::LinuxConsole => linux_fn(b'E'),
            _ => tilde(15),
        },
        Named::F6 => tilde(17),
        Named::F7 => tilde(18),
        Named::F8 => tilde(19),
        Named::F9 => tilde(20),
        Named::F10 => tilde(21),
        Named::F11 => tilde(23),
        Named::F12 => tilde(24),
        _ => return None,
    };
    Some(bytes)
}

/// Translate a Ctrl+<char> combination into the control byte sequence.
pub(crate) fn ctrl_key_bytes(key: &keyboard::Key) -> Option<Vec<u8>> {
    if let keyboard::Key::Character(c) = key {
        let ch = c.as_str().bytes().next()?;
        let ctrl = match ch {
            b'a'..=b'z' => ch - b'a' + 1,
            b'A'..=b'Z' => ch - b'A' + 1,
            // The C0 block maps `@`..`_` (0x40..0x5f) to 0x00..0x1f, i.e.
            // `ch & 0x1f`; `@` (0x40) folds to NUL exactly like the letters.
            b'@' => 0,
            b'[' => 27,
            b'\\' => 28,
            b']' => 29,
            b'^' => 30,
            b'_' => 31,
            // The one exception to `ch & 0x1f`: Ctrl+? is DEL (127), not 31,
            // matching xterm / the historical teletype mapping.
            b'?' => 127,
            _ => return None,
        };
        Some(vec![ctrl])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::key::Named;
    use iced::keyboard::{Key, Location, Modifiers};
    use oryxis_core::models::terminal_quirks::{
        BackspaceMode, FunctionKeyMode, HomeEndMode, OptionAsMeta, TerminalQuirks, DEFAULT_QUIRKS,
    };

    fn nb(named: Named, mods: Modifiers, app_cursor: bool) -> Vec<u8> {
        // Existing vectors run against DEFAULT_QUIRKS (today's xterm
        // behaviour); a change here that broke them would mean the quirks
        // refactor altered the default encoding.
        key_to_named_bytes(&Key::Named(named), &mods, app_cursor, &DEFAULT_QUIRKS).unwrap()
    }

    // Vectors under a specific quirks profile.
    fn nq(named: Named, mods: Modifiers, app_cursor: bool, q: &TerminalQuirks) -> Vec<u8> {
        key_to_named_bytes(&Key::Named(named), &mods, app_cursor, q).unwrap()
    }

    // ── pty_bytes matrix helpers ───────────────────────────────────────

    struct Press {
        key: Key,
        modified: Key,
        mods: Modifiers,
        text: Option<&'static str>,
        location: Location,
    }

    impl Press {
        fn ch(base: &'static str) -> Self {
            Press {
                key: Key::Character(base.into()),
                modified: Key::Character(base.into()),
                mods: Modifiers::empty(),
                text: Some(base),
                location: Location::Standard,
            }
        }
        fn named(named: Named) -> Self {
            Press {
                key: Key::Named(named),
                modified: Key::Named(named),
                mods: Modifiers::empty(),
                text: None,
                location: Location::Standard,
            }
        }
        fn mods(mut self, mods: Modifiers) -> Self {
            self.mods = mods;
            self
        }
        fn composes(mut self, s: &'static str) -> Self {
            self.modified = Key::Character(s.into());
            self.text = Some(s);
            self
        }
        fn text(mut self, t: Option<&'static str>) -> Self {
            self.text = t;
            self
        }
        fn numpad(mut self) -> Self {
            self.location = Location::Numpad;
            self
        }
        fn encode(&self, platform: Platform, quirks: &TerminalQuirks, sides: OptionSides) -> Option<Vec<u8>> {
            pty_bytes(
                &KeyPress {
                    key: &self.key,
                    modified_key: &self.modified,
                    modifiers: self.mods,
                    text: self.text,
                    location: self.location,
                },
                platform,
                false,
                quirks,
                sides,
            )
        }
        fn on(&self, platform: Platform) -> Option<Vec<u8>> {
            self.encode(platform, &DEFAULT_QUIRKS, OptionSides::default())
        }
    }

    const CTRL_ALT: Modifiers = Modifiers::CTRL.union(Modifiers::ALT);

    // ── issue #80 matrix: AltGr / Option composition ───────────────────

    #[test]
    fn linux_bepo_altgr_space_types_underscore() {
        // Linux reports AltGr as no modifier; only text/modified_key
        // carry the composed `_`. Named::Space must not outrank it.
        let p = Press::named(Named::Space).composes("_");
        assert_eq!(p.on(Platform::Linux).unwrap(), b"_");
    }

    #[test]
    fn windows_bepo_altgr_space_types_underscore() {
        // Windows reports AltGr as Ctrl+Alt; the press must not fall
        // into the Ctrl path (which would emit a plain space).
        let p = Press::named(Named::Space).composes("_").mods(CTRL_ALT);
        assert_eq!(p.on(Platform::Windows).unwrap(), b"_");
    }

    #[test]
    fn windows_german_altgr_q_types_at() {
        // AltGr+Q on the German layout is `@`; the old encoder sent
        // Ctrl+Q (0x11 = XON), hijacking flow control.
        let p = Press::ch("q").composes("@").mods(CTRL_ALT);
        assert_eq!(p.on(Platform::Windows).unwrap(), b"@");
    }

    #[test]
    fn windows_german_altgr_8_types_bracket() {
        let p = Press::ch("8").composes("[").mods(CTRL_ALT);
        assert_eq!(p.on(Platform::Windows).unwrap(), b"[");
    }

    #[test]
    fn windows_altgr_shift_composition_is_not_swallowed() {
        // AltGr+Shift (level 4) arrives as Ctrl+Alt+Shift and must not
        // be eaten by the Ctrl+Shift chord swallow.
        let p = Press::ch("e")
            .composes("\u{20ac}")
            .mods(CTRL_ALT.union(Modifiers::SHIFT));
        assert_eq!(p.on(Platform::Windows).unwrap(), "\u{20ac}".as_bytes());
    }

    #[test]
    fn ctrl_alt_letter_without_composition_is_esc_ctrl_byte() {
        // A deliberate Ctrl+Alt chord (no composed character, e.g. the
        // US layout) is xterm's ESC-prefixed control byte.
        let p = Press::ch("b").text(None).mods(CTRL_ALT);
        assert_eq!(p.on(Platform::Windows).unwrap(), vec![0x1b, 0x02]);
        assert_eq!(p.on(Platform::Linux).unwrap(), vec![0x1b, 0x02]);
    }

    #[test]
    fn ctrl_alt_text_equal_to_base_is_still_a_chord() {
        // Some backends leave the bare character in `text` under
        // Ctrl+Alt (winit#3038); text identical to the base key is not
        // composition.
        let p = Press::ch("a").mods(CTRL_ALT);
        assert_eq!(p.on(Platform::Windows).unwrap(), vec![0x1b, 0x01]);
    }

    // ── macOS Option: compose by default, Meta per quirk/side ──────────

    #[test]
    fn macos_option_composes_by_default() {
        // German Mac layout: Option+7 is the ONLY way to type a pipe.
        let p = Press::ch("7").composes("|").mods(Modifiers::ALT);
        let sides = OptionSides { left: true, right: false };
        assert_eq!(p.encode(Platform::MacOs, &DEFAULT_QUIRKS, sides).unwrap(), b"|");
    }

    #[test]
    fn macos_option_as_meta_both_sends_esc_base_char() {
        let q = TerminalQuirks { option_as_meta: OptionAsMeta::Both, ..Default::default() };
        // The composed char must not leak into the Meta sequence:
        // Option+b is ESC b, never ESC + the integral sign.
        let p = Press::ch("b").composes("\u{222b}").mods(Modifiers::ALT);
        let sides = OptionSides { left: true, right: false };
        assert_eq!(p.encode(Platform::MacOs, &q, sides).unwrap(), b"\x1bb");
        // Shift folds into the Meta character (M-B).
        let p = Press::ch("b")
            .composes("\u{131}")
            .mods(Modifiers::ALT.union(Modifiers::SHIFT));
        assert_eq!(p.encode(Platform::MacOs, &q, sides).unwrap(), b"\x1bB");
    }

    #[test]
    fn macos_option_meta_sides_are_independent() {
        let q = TerminalQuirks { option_as_meta: OptionAsMeta::OnlyLeft, ..Default::default() };
        let p = Press::ch("b").composes("\u{222b}").mods(Modifiers::ALT);
        let left = OptionSides { left: true, right: false };
        let right = OptionSides { left: false, right: true };
        assert_eq!(p.encode(Platform::MacOs, &q, left).unwrap(), b"\x1bb");
        assert_eq!(
            p.encode(Platform::MacOs, &q, right).unwrap(),
            "\u{222b}".as_bytes()
        );
    }

    #[test]
    fn macos_cmd_combos_are_swallowed() {
        let p = Press::ch("c").mods(Modifiers::LOGO);
        assert_eq!(p.on(Platform::MacOs), None);
        // Super+key elsewhere keeps today's behaviour (falls through).
        assert!(p.on(Platform::Linux).is_some());
    }

    // ── goldens: the pre-#80 behaviour that must not move ──────────────

    #[test]
    fn plain_and_shifted_typing_use_the_event_text() {
        assert_eq!(Press::ch("a").on(Platform::Linux).unwrap(), b"a");
        let p = Press::ch("2").composes("@").mods(Modifiers::SHIFT);
        assert_eq!(p.on(Platform::Windows).unwrap(), b"@");
    }

    #[test]
    fn ctrl_letter_is_the_control_byte() {
        let p = Press::ch("u").text(Some("\u{15}")).mods(Modifiers::CTRL);
        assert_eq!(p.on(Platform::Linux).unwrap(), vec![0x15]);
    }

    #[test]
    fn ctrl_space_is_nul() {
        let p = Press::named(Named::Space).text(Some(" ")).mods(Modifiers::CTRL);
        assert_eq!(p.on(Platform::Linux).unwrap(), vec![0x00]);
        assert_eq!(nb(Named::Space, Modifiers::CTRL, false), vec![0x00]);
        // Plain Space stays a plain space.
        assert_eq!(nb(Named::Space, Modifiers::empty(), false), b" ");
    }

    #[test]
    fn ctrl_at_is_nul_and_ctrl_question_is_del() {
        // Completes the C0 table: `@` folds to NUL like a letter (0x40 &
        // 0x1f), `?` is the DEL exception (127, not 31). Reached through
        // the plain-Ctrl branch when the layout puts these on an unshifted
        // key (or via Ctrl+Alt); Ctrl+Shift on a US layout stays swallowed.
        let at = Press::ch("@").text(Some("\u{0}")).mods(Modifiers::CTRL);
        assert_eq!(at.on(Platform::Linux).unwrap(), vec![0x00]);
        let q = Press::ch("?").text(Some("\u{7f}")).mods(Modifiers::CTRL);
        assert_eq!(q.on(Platform::Linux).unwrap(), vec![0x7f]);
    }

    #[test]
    fn ctrl_shift_chords_are_swallowed() {
        let p = Press::ch("c").mods(Modifiers::CTRL.union(Modifiers::SHIFT));
        assert_eq!(p.on(Platform::Linux), None);
    }

    #[test]
    fn ctrl_shift_named_keys_emit_the_xterm_modified_sequence() {
        // Cursor / editing / function keys under Ctrl+Shift are now
        // normalized to the param-6 form real terminals send, instead of
        // being swallowed. (The encoder-level form is covered by
        // `modified_arrows_use_xterm_parameter_and_stay_csi`; this checks
        // the end-to-end pty_bytes path that used to drop them.)
        //
        // NOTE the arrows are DEFAULT-bound to FocusPane* (terminal_only),
        // so under stock bindings Ctrl+Shift+Left is consumed by the hotkey
        // router upstream and never reaches pty_bytes; this asserts the
        // unit behaviour that applies once those chords are rebound/freed.
        // Home / PageUp / etc. have no Ctrl+Shift default, so they reach
        // the PTY as-is.
        let cs = Modifiers::CTRL.union(Modifiers::SHIFT);
        assert_eq!(
            Press::named(Named::ArrowLeft).mods(cs).on(Platform::Linux).unwrap(),
            b"\x1b[1;6D"
        );
        assert_eq!(
            Press::named(Named::PageUp).mods(cs).on(Platform::Linux).unwrap(),
            b"\x1b[5;6~"
        );
        assert_eq!(
            Press::named(Named::Home).mods(cs).on(Platform::Linux).unwrap(),
            b"\x1b[1;6H"
        );
        // Non-sequence keys under Ctrl+Shift stay swallowed: characters
        // (the clipboard trio / hotkeys) and Enter / Tab / Space.
        assert_eq!(Press::ch("c").mods(cs).on(Platform::Linux), None);
        assert_eq!(Press::named(Named::Enter).mods(cs).on(Platform::Linux), None);
        assert_eq!(Press::named(Named::Space).mods(cs).on(Platform::Linux), None);
    }

    #[test]
    fn alt_char_is_meta_esc_on_linux_and_windows() {
        let p = Press::ch("b").mods(Modifiers::ALT);
        assert_eq!(p.on(Platform::Linux).unwrap(), b"\x1bb");
        assert_eq!(p.on(Platform::Windows).unwrap(), b"\x1bb");
        // Platforms that drop `text` under Alt fall back to the base char.
        let p = Press::ch("b").text(None).mods(Modifiers::ALT);
        assert_eq!(p.on(Platform::Linux).unwrap(), b"\x1bb");
    }

    #[test]
    fn numpad_numlock_prefers_the_digit_text() {
        // NumLock-on numpad 7 arrives as Named::Home with text "7".
        let p = Press::named(Named::Home).text(Some("7")).numpad();
        assert_eq!(p.on(Platform::Linux).unwrap(), b"7");
    }

    #[test]
    fn numpad_enter_matches_the_main_enter_on_every_platform() {
        // Issue #162: the numpad's Enter must send what the main Enter
        // sends, whatever the OS wrote into `text`. macOS reports it as
        // the Cocoa NSEnterCharacter (U+0003, ETX = ^C); Windows / Linux
        // report the CR the key means. Both resolve to the main Enter's
        // encoding, so the pair is asserted against each other rather
        // than against a literal.
        for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
            let main = Press::named(Named::Enter).on(platform);
            assert_eq!(main.as_deref(), Some(b"\r".as_slice()));
            for text in [Some("\u{3}"), Some("\r"), Some("\n"), None] {
                let numpad = Press::named(Named::Enter).text(text).numpad();
                assert_eq!(
                    numpad.on(platform),
                    main,
                    "numpad Enter with text {text:?} on {platform:?}"
                );
            }
        }
    }

    #[test]
    fn numpad_control_text_never_outranks_the_named_key() {
        // Same rule one step up: control text on the numpad is never
        // typing, so the named key keeps its own encoding (a numpad
        // Delete reporting DEL still sends the CSI form).
        let p = Press::named(Named::Delete).text(Some("\u{7f}")).numpad();
        assert_eq!(p.on(Platform::MacOs).unwrap(), b"\x1b[3~");
    }

    // ── named-key vectors (pre-C5 goldens + C5 quirk modes) ────────────

    #[test]
    fn arrows_plain_are_csi_and_app_cursor_is_ss3() {
        // Default (normal) cursor mode: CSI form.
        assert_eq!(nb(Named::ArrowUp, Modifiers::empty(), false), b"\x1b[A");
        assert_eq!(nb(Named::ArrowLeft, Modifiers::empty(), false), b"\x1b[D");
        // Application-cursor-keys mode (DECCKM): SS3 form, what mc/vim bind to.
        assert_eq!(nb(Named::ArrowUp, Modifiers::empty(), true), b"\x1bOA");
        assert_eq!(nb(Named::End, Modifiers::empty(), true), b"\x1bOF");
    }

    #[test]
    fn modified_arrows_use_xterm_parameter_and_stay_csi() {
        // Ctrl = param 5, Shift = 2, Alt = 3.
        assert_eq!(nb(Named::ArrowRight, Modifiers::CTRL, false), b"\x1b[1;5C");
        assert_eq!(nb(Named::ArrowUp, Modifiers::SHIFT, false), b"\x1b[1;2A");
        assert_eq!(nb(Named::ArrowDown, Modifiers::ALT, false), b"\x1b[1;3B");
        assert_eq!(
            nb(Named::ArrowLeft, Modifiers::CTRL | Modifiers::SHIFT, false),
            b"\x1b[1;6D"
        );
        // A modified press is CSI even under application-cursor-keys mode.
        assert_eq!(nb(Named::ArrowUp, Modifiers::CTRL, true), b"\x1b[1;5A");
    }

    #[test]
    fn tilde_keys_carry_modifier() {
        assert_eq!(nb(Named::PageUp, Modifiers::empty(), false), b"\x1b[5~");
        assert_eq!(nb(Named::PageUp, Modifiers::CTRL, false), b"\x1b[5;5~");
        assert_eq!(nb(Named::Delete, Modifiers::SHIFT, false), b"\x1b[3;2~");
    }

    #[test]
    fn function_keys_promote_to_csi_when_modified() {
        assert_eq!(nb(Named::F1, Modifiers::empty(), false), b"\x1bOP");
        assert_eq!(nb(Named::F1, Modifiers::CTRL, false), b"\x1b[1;5P");
        assert_eq!(nb(Named::F5, Modifiers::empty(), false), b"\x1b[15~");
        assert_eq!(nb(Named::F5, Modifiers::CTRL, false), b"\x1b[15;5~");
    }

    #[test]
    fn shift_tab_is_back_tab() {
        assert_eq!(nb(Named::Tab, Modifiers::empty(), false), b"\t");
        assert_eq!(nb(Named::Tab, Modifiers::SHIFT, false), b"\x1b[Z");
    }

    #[test]
    fn backspace_modes_and_ctrl_flip() {
        // Default (Del127): plain = DEL (0x7f) as today; Ctrl+Backspace
        // sends the flipped BS (0x08), the PuTTY Control-? convention.
        assert_eq!(nb(Named::Backspace, Modifiers::empty(), false), b"\x7f");
        assert_eq!(nb(Named::Backspace, Modifiers::CTRL, false), b"\x08");
        // CtrlH: plain = BS (0x08); Ctrl+Backspace flips to DEL (0x7f).
        let ctrl_h = TerminalQuirks { backspace: BackspaceMode::CtrlH, ..Default::default() };
        assert_eq!(nq(Named::Backspace, Modifiers::empty(), false, &ctrl_h), b"\x08");
        assert_eq!(nq(Named::Backspace, Modifiers::CTRL, false, &ctrl_h), b"\x7f");
    }

    #[test]
    fn rxvt_home_end_is_tilde_and_app_cursor_independent() {
        let rxvt = TerminalQuirks { home_end: HomeEndMode::Rxvt, ..Default::default() };
        // rxvt Home/End are the tilde form, unaffected by app-cursor mode.
        assert_eq!(nq(Named::Home, Modifiers::empty(), false, &rxvt), b"\x1b[7~");
        assert_eq!(nq(Named::End, Modifiers::empty(), false, &rxvt), b"\x1b[8~");
        assert_eq!(nq(Named::Home, Modifiers::empty(), true, &rxvt), b"\x1b[7~");
        // Modified form carries the xterm parameter.
        assert_eq!(nq(Named::End, Modifiers::CTRL, false, &rxvt), b"\x1b[8;5~");
        // Default (Standard) stays the cursor-style CSI/SS3 form.
        assert_eq!(nb(Named::Home, Modifiers::empty(), false), b"\x1b[H");
        assert_eq!(nb(Named::Home, Modifiers::empty(), true), b"\x1bOH");
    }

    #[test]
    fn vt400_function_keys_stay_ss3_even_when_modified() {
        let vt400 =
            TerminalQuirks { function_keys: FunctionKeyMode::Vt400, ..Default::default() };
        assert_eq!(nq(Named::F1, Modifiers::empty(), false, &vt400), b"\x1bOP");
        // No CSI modified form, unlike xterm: SS3 always.
        assert_eq!(nq(Named::F1, Modifiers::SHIFT, false, &vt400), b"\x1bOP");
        assert_eq!(nq(Named::F4, Modifiers::CTRL, false, &vt400), b"\x1bOS");
        // F5+ are the xterm tilde form in VT400 too.
        assert_eq!(nq(Named::F5, Modifiers::empty(), false, &vt400), b"\x1b[15~");
    }

    #[test]
    fn linux_console_function_keys_are_csi_bracket() {
        let linux =
            TerminalQuirks { function_keys: FunctionKeyMode::LinuxConsole, ..Default::default() };
        // F1-F5 = ESC [ [ A..E, no modified form.
        assert_eq!(nq(Named::F1, Modifiers::empty(), false, &linux), b"\x1b[[A");
        assert_eq!(nq(Named::F5, Modifiers::empty(), false, &linux), b"\x1b[[E");
        assert_eq!(nq(Named::F1, Modifiers::CTRL, false, &linux), b"\x1b[[A");
        // F6+ fall back to the xterm tilde numbers.
        assert_eq!(nq(Named::F6, Modifiers::empty(), false, &linux), b"\x1b[17~");
    }

    #[test]
    fn rxvt_function_keys_use_rxvt_tilde_numbers() {
        let rxvt =
            TerminalQuirks { function_keys: FunctionKeyMode::Rxvt, ..Default::default() };
        // F1-F4 = tilde 11..14; F5+ keep the xterm numbers.
        assert_eq!(nq(Named::F1, Modifiers::empty(), false, &rxvt), b"\x1b[11~");
        assert_eq!(nq(Named::F4, Modifiers::empty(), false, &rxvt), b"\x1b[14~");
        assert_eq!(nq(Named::F5, Modifiers::empty(), false, &rxvt), b"\x1b[15~");
        // Modified F1 carries the parameter (tilde form).
        assert_eq!(nq(Named::F1, Modifiers::CTRL, false, &rxvt), b"\x1b[11;5~");
    }
}
