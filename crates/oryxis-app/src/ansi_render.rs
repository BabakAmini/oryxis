//! Render recorded terminal output (raw bytes with ANSI escapes) into
//! colored text spans for the session-log viewer.
//!
//! A tiny line-oriented emulator rather than a strip pass: carriage
//! returns overwrite the line (so progress bars and redrawn prompts
//! don't smear into "oot@host~root@host" artifacts), erase-line is
//! honored, OSC/CSI/charset sequences are consumed instead of leaking
//! replacement glyphs, and SGR color codes map onto the active
//! terminal palette so the dump reads like the terminal did.
//!
//! The cursor can also move UP across committed lines (DECSC/DECRC
//! save/restore, CUU/CUD, erase-down): output that wipes its own
//! lines from the live screen, like the OSC 7 cwd-tracking bootstrap
//! the app injects at connect (`ESC 7` ... `ESC 8 ESC[1A ESC[J`),
//! must vanish from the dump the same way it vanishes live (field
//! report 2026-07-17). Absolute cursor addressing (`ESC[H`, full-
//! screen TUIs) stays out: a linear dump has no viewport, so those
//! degrade to appended lines, the best a transcript can do.

use iced::Color;
use oryxis_terminal::TerminalPalette;

/// One run of same-colored text. `color: None` means the palette's
/// default foreground (resolved at view time so theme switches while
/// the viewer is open don't strand a stale color).
#[derive(Debug, Clone)]
pub(crate) struct AnsiSpan {
    pub text: String,
    pub color: Option<Color>,
}

type Cell = (char, Option<Color>);

/// Map an SGR-selected color index (0-255) onto a concrete color:
/// 0-15 from the theme palette, 16-231 from the 6x6x6 cube, 232-255
/// from the grayscale ramp.
fn indexed_color(idx: u8, palette: &TerminalPalette) -> Color {
    match idx {
        0..=15 => palette.ansi[idx as usize],
        16..=231 => {
            let i = idx - 16;
            let comp = |v: u8| -> f32 {
                if v == 0 { 0.0 } else { (55 + 40 * v as u16) as f32 / 255.0 }
            };
            Color::from_rgb(comp(i / 36), comp((i / 6) % 6), comp(i % 6))
        }
        _ => {
            let v = (8 + 10 * (idx - 232) as u16) as f32 / 255.0;
            Color::from_rgb(v, v, v)
        }
    }
}

/// Pen state driven by SGR sequences. Bold promotes the 8 base colors
/// to their bright variants, matching most terminal themes.
#[derive(Default, Clone, Copy)]
struct Pen {
    /// Base ANSI index (0-7) when set via 30-37, kept so a later bold
    /// can re-promote it.
    base_idx: Option<u8>,
    color: Option<Color>,
    bold: bool,
}

impl Pen {
    fn apply_sgr(&mut self, params: &[u16], palette: &TerminalPalette) {
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => *self = Pen::default(),
                1 => {
                    self.bold = true;
                    if let Some(b) = self.base_idx {
                        self.color = Some(palette.ansi[(b + 8) as usize]);
                    }
                }
                22 => {
                    self.bold = false;
                    if let Some(b) = self.base_idx {
                        self.color = Some(palette.ansi[b as usize]);
                    }
                }
                30..=37 => {
                    let b = (params[i] - 30) as u8;
                    self.base_idx = Some(b);
                    let idx = if self.bold { b + 8 } else { b };
                    self.color = Some(palette.ansi[idx as usize]);
                }
                90..=97 => {
                    let b = (params[i] - 90) as u8;
                    self.base_idx = Some(b);
                    self.color = Some(palette.ansi[(b + 8) as usize]);
                }
                39 => {
                    self.base_idx = None;
                    self.color = None;
                }
                38 => {
                    // Extended fg: 38;5;n or 38;2;r;g;b.
                    self.base_idx = None;
                    if params.get(i + 1) == Some(&5)
                        && let Some(&n) = params.get(i + 2)
                    {
                        self.color = Some(indexed_color(n as u8, palette));
                        i += 2;
                    } else if params.get(i + 1) == Some(&2)
                        && let (Some(&r), Some(&g), Some(&b)) =
                            (params.get(i + 2), params.get(i + 3), params.get(i + 4))
                    {
                        self.color = Some(Color::from_rgb8(r as u8, g as u8, b as u8));
                        i += 4;
                    }
                }
                48 => {
                    // Extended bg: consume its arguments, ignore the color
                    // (the viewer renders foreground only).
                    if params.get(i + 1) == Some(&5) {
                        i += 2;
                    } else if params.get(i + 1) == Some(&2) {
                        i += 4;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Parse recorded bytes into colored spans. The cursor model is a
/// linear grid: `\r` rewinds the column, printable chars overwrite,
/// `\n` moves to the next row, and relative vertical motion (CUU/CUD,
/// DECSC/DECRC restore) can revisit earlier rows so self-erasing
/// output resolves like it did on the live screen.
pub(crate) fn render(data: &[u8], palette: &TerminalPalette) -> Vec<AnsiSpan> {
    let text = String::from_utf8_lossy(data);
    let mut chars = text.chars().peekable();

    let mut lines: Vec<Vec<Cell>> = vec![Vec::new()];
    let mut row: usize = 0;
    let mut col: usize = 0;
    let mut saved_cursor: Option<(usize, usize)> = None;
    let mut pen = Pen::default();

    while let Some(ch) = chars.next() {
        match ch {
            '\x1b' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    // CSI: numeric/; params then a final byte 0x40-0x7e.
                    let mut params: Vec<u16> = Vec::new();
                    let mut cur: Option<u16> = None;
                    let mut fin = '\0';
                    for c in chars.by_ref() {
                        match c {
                            '0'..='9' => {
                                let d = c as u16 - '0' as u16;
                                cur = Some(cur.unwrap_or(0).saturating_mul(10).saturating_add(d));
                            }
                            ';' | ':' => {
                                params.push(cur.take().unwrap_or(0));
                            }
                            '?' | '>' | '<' | '=' | ' ' | '!' | '"' | '#' | '$' | '%' => {}
                            c if ('\u{40}'..='\u{7e}').contains(&c) => {
                                fin = c;
                                break;
                            }
                            _ => break,
                        }
                    }
                    if let Some(v) = cur.take() {
                        params.push(v);
                    }
                    let n = params.first().copied().unwrap_or(1).max(1) as usize;
                    match fin {
                        'm' => {
                            if params.is_empty() {
                                params.push(0);
                            }
                            pen.apply_sgr(&params, palette);
                        }
                        // Erase in line: 0/default = cursor to end.
                        'K' if params.first().copied().unwrap_or(0) == 0 => {
                            lines[row].truncate(col);
                        }
                        'K' if params.first() == Some(&2) => {
                            lines[row].clear();
                            col = 0;
                        }
                        // Cursor forward: pad with spaces when past the end.
                        'C' => col += n,
                        // Cursor back / column absolute.
                        'D' => col = col.saturating_sub(n),
                        'G' => {
                            col = params.first().copied().unwrap_or(1).max(1) as usize - 1;
                        }
                        // Cursor up/down. Down clamps to written rows: a
                        // linear dump has no blank viewport to move into.
                        'A' => row = row.saturating_sub(n),
                        'B' => row = (row + n).min(lines.len() - 1),
                        // Erase in display, 0/default = cursor to end of
                        // screen: truncate this row at the cursor and drop
                        // every row below, the erase half of the
                        // self-wiping pattern. `2J` (clear all) stays
                        // ignored: a live terminal keeps its scrollback
                        // through it, and so should the dump.
                        'J' if params.first().copied().unwrap_or(0) == 0 => {
                            lines[row].truncate(col);
                            lines.truncate(row + 1);
                        }
                        _ => {}
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: terminated by BEL or ST (ESC \).
                    let mut prev_esc = false;
                    for c in chars.by_ref() {
                        if c == '\x07' || (prev_esc && c == '\\') {
                            break;
                        }
                        prev_esc = c == '\x1b';
                    }
                }
                Some('(') | Some(')') => {
                    // Charset designation: ESC ( B etc., two chars total.
                    chars.next();
                    chars.next();
                }
                // DECSC/DECRC cursor save + restore, the save half of
                // the self-wiping pattern (see the module docs).
                Some('7') => {
                    chars.next();
                    saved_cursor = Some((row, col));
                }
                Some('8') => {
                    chars.next();
                    if let Some((r, c)) = saved_cursor {
                        row = r.min(lines.len() - 1);
                        col = c;
                    }
                }
                _ => {
                    // Other ESC x: consume the single following char.
                    chars.next();
                }
            },
            '\n' => {
                row += 1;
                if row == lines.len() {
                    lines.push(Vec::new());
                }
                col = 0;
            }
            '\r' => col = 0,
            '\x08' => col = col.saturating_sub(1),
            '\t' => col = (col / 8 + 1) * 8,
            c if c.is_control() => {}
            c => {
                let line = &mut lines[row];
                while line.len() < col {
                    line.push((' ', None));
                }
                if col < line.len() {
                    line[col] = (c, pen.color);
                } else {
                    line.push((c, pen.color));
                }
                col += 1;
            }
        }
    }
    // The last row is the uncommitted partial; an empty one renders
    // nothing (same shape the pre-grid renderer produced).
    if lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }

    // Compress cell rows into same-color spans (newlines included in
    // the span text so the viewer renders one Rich text).
    let mut spans: Vec<AnsiSpan> = Vec::new();
    let push = |text: String, color: Option<Color>, spans: &mut Vec<AnsiSpan>| {
        if text.is_empty() {
            return;
        }
        if let Some(last) = spans.last_mut()
            && last.color.map(color_key) == color.map(color_key)
        {
            last.text.push_str(&text);
            return;
        }
        spans.push(AnsiSpan { text, color });
    };
    for row in &lines {
        let mut run = String::new();
        let mut run_color: Option<Color> = None;
        for (c, color) in row {
            if color.map(color_key) != run_color.map(color_key) && !run.is_empty() {
                push(std::mem::take(&mut run), run_color, &mut spans);
            }
            run_color = *color;
            run.push(*c);
        }
        run.push('\n');
        push(run, run_color, &mut spans);
    }
    spans
}

/// Serialize rendered spans back into terminal bytes: truecolor SGR per
/// span, `\r\n` line ends, a reset at the end.
///
/// This is what lets the transcript viewer show a linear dump inside the
/// real terminal widget instead of a text widget. The emulator is what
/// gives the viewer its selection, copy-on-select, right-click schemes
/// and Ctrl+F (issues #90/#91); [`render`] is what gives it CONTENT for a
/// recording that lived on the alternate screen, where replaying the
/// stream faithfully leaves nothing to read (a whole tmux session repaints
/// one screen that has no scrollback). Only the foreground color survives,
/// because that is all `render` tracks, and all the old text viewer drew.
pub(crate) fn to_ansi_bytes(spans: &[AnsiSpan]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut current: Option<[u32; 4]> = None;
    for span in spans {
        let key = span.color.map(color_key);
        if key != current {
            match span.color {
                Some(c) => {
                    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    out.extend_from_slice(
                        format!("\x1b[38;2;{};{};{}m", q(c.r), q(c.g), q(c.b)).as_bytes(),
                    );
                }
                None => out.extend_from_slice(b"\x1b[39m"),
            }
            current = key;
        }
        // `render` puts the newlines inside the span text; the emulator
        // needs the carriage return too, or every line starts where the
        // previous one ended.
        for ch in span.text.chars() {
            if ch == '\n' {
                out.extend_from_slice(b"\r\n");
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.extend_from_slice(b"\x1b[0m");
    out
}

/// Comparable key for a Color (f32 fields aren't Eq).
fn color_key(c: Color) -> [u32; 4] {
    [c.r.to_bits(), c.g.to_bits(), c.b.to_bits(), c.a.to_bits()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(data: &[u8]) -> String {
        let palette = TerminalPalette::default();
        render(data, &palette).iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn carriage_return_overwrites_the_line() {
        // The shell redraws the prompt over itself; the dump must not
        // smear both renders together.
        assert_eq!(
            flat(b"root@host:~# \rroot@host:~# ls\n"),
            "root@host:~# ls\n"
        );
        // Progress-bar style updates keep only the final state.
        assert_eq!(flat(b"10%\r50%\r100%\n"), "100%\n");
    }

    #[test]
    fn erase_line_truncates() {
        assert_eq!(flat(b"hello world\r\x1b[Khi\n"), "hi\n");
    }

    #[test]
    fn osc_and_charset_sequences_vanish() {
        assert_eq!(flat(b"\x1b]0;window title\x07ok\n"), "ok\n");
        assert_eq!(flat(b"\x1b(Bok\x1b)0\n"), "ok\n");
    }

    /// The OSC 7 cwd-tracking bootstrap the app injects at connect
    /// erases its own echo from the live screen (DECSC before, then
    /// DECRC + CUU + erase-down + a prompt redraw). The dump must
    /// resolve that erasure instead of leaking the injected command
    /// into the viewer / transcript (field report 2026-07-17).
    #[test]
    fn self_erasing_injection_vanishes_like_live() {
        let data = b"banner\r\n\
            root@h:~# printf 'x'\r\n\
            \x1b7root@h:~# __oryxis_o7(){ ...; }\r\n\
            \x1b8\x1b[1A\x1b[Jroot@h:~# \n";
        assert_eq!(flat(data), "banner\nroot@h:~# \n");
    }

    #[test]
    fn cursor_up_revisits_rows_and_erase_down_drops_them() {
        // "three" ends at col 5; two rows up keeps the column, so the
        // erase truncates nothing on "one" (len 3) but drops the rows
        // below, and the next glyphs land padded at col 5.
        assert_eq!(
            flat(b"one\r\ntwo\r\nthree\x1b[2A\x1b[Jok\n"),
            "one  ok\n"
        );
        // Cursor down is clamped to written rows.
        assert_eq!(flat(b"a\x1b[5Bb\n"), "ab\n");
    }

    #[test]
    fn restore_without_save_is_a_noop() {
        assert_eq!(flat(b"keep\x1b8\x1b[Jok\n"), "keepok\n");
    }

    #[test]
    fn column_absolute_and_cursor_back_move_within_the_line() {
        // CHA to column 1 then overwrite; CUB backs over a glyph.
        assert_eq!(flat(b"abcdef\x1b[1GX\n"), "Xbcdef\n");
        assert_eq!(flat(b"abc\x1b[2Dz\n"), "azc\n");
    }

    #[test]
    fn sgr_colors_map_to_palette() {
        let palette = TerminalPalette::default();
        let spans = render(b"\x1b[31mred\x1b[0m plain\n", &palette);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].text, "red");
        assert_eq!(spans[0].color.map(color_key), Some(color_key(palette.ansi[1])));
        assert_eq!(spans[1].text, " plain\n");
        assert!(spans[1].color.is_none());
    }

    #[test]
    fn bold_promotes_to_bright() {
        let palette = TerminalPalette::default();
        let spans = render(b"\x1b[1;32mok\n", &palette);
        assert_eq!(spans[0].color.map(color_key), Some(color_key(palette.ansi[10])));
    }

    /// The linear transcript mode feeds these bytes into the real
    /// emulator, so they have to arrive as separate lines (CR included,
    /// not just LF) and keep their color.
    #[test]
    fn to_ansi_bytes_feeds_the_emulator_line_by_line() {
        let palette = TerminalPalette::default();
        let bytes = to_ansi_bytes(&render(b"\x1b[31mred\x1b[0m one\ntwo\n", &palette));
        assert!(
            bytes.windows(7).any(|w| w == b"\x1b[38;2;"),
            "colored runs carry a truecolor SGR"
        );
        assert!(!bytes.windows(2).any(|w| w == b"\r\r"));

        let mut term = oryxis_terminal::widget::TerminalState::new_no_pty(80, 24).unwrap();
        term.process(&bytes);
        let text = term.all_text();
        assert!(text.contains("red one"), "text survives: {text:?}");
        assert!(
            text.lines().any(|l| l.trim() == "two"),
            "the second line starts at column 0: {text:?}"
        );
    }
}
