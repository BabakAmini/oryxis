//! Import popular terminal color schemes into a `CustomTerminalTheme`.
//!
//! Three formats, auto-detected from the pasted content:
//! - **Windows Terminal** JSON scheme object (`background`, `foreground`,
//!   `black`..`brightWhite`).
//! - **base16** YAML (`base00`..`base0F`), mapped to the 16 ANSI slots by
//!   the standard base16 shell-template convention.
//! - **iTerm2** `.itermcolors` (XML plist with float 0..1 components).
//!
//! Each parser is pure (`&str -> Result<CustomTerminalTheme, String>`) so it
//! can be unit-tested without a vault or UI.

use oryxis_core::models::custom_terminal_theme::CustomTerminalTheme;
use oryxis_core::models::custom_ui_theme::CustomUiTheme;

/// Which of the two importers a pasted file belongs to. The app has one
/// panel per kind, in different settings sections, so this is what a
/// misplaced paste is routed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThemeKind {
    /// Oryxis chrome theme (Settings > Interface).
    Ui,
    /// Terminal color scheme (Settings > Terminal).
    Terminal,
}

/// Why an import failed. Typed rather than a ready-made sentence for two
/// reasons: the app ACTS on `WrongImporter` (it moves the paste to the
/// other panel instead of showing anything), and every message it does
/// show has to be translated, which `localized` is the single point of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportError {
    /// The content is fine, just pasted in the wrong panel; the payload
    /// is where it belongs.
    WrongImporter(ThemeKind),
    /// JSON without the `oryxis_ui_theme` marker that no terminal
    /// format recognizes either.
    NotUiTheme,
    /// The content is not JSON at all (payload = the parser's own
    /// message, which carries the offending line / column).
    InvalidJson(String),
    /// A required key is missing or is not a color (payload = the key).
    MissingField(String),
    /// None of the three terminal formats matched.
    UnrecognizedFormat,
}

impl ImportError {
    /// The sentence shown in an import panel's error slot. `WrongImporter`
    /// normally never reaches a user (the app redirects on it), but it
    /// renders the destination anyway so any other caller stays honest.
    pub(crate) fn localized(&self) -> String {
        use crate::i18n::t;
        match self {
            ImportError::WrongImporter(ThemeKind::Ui) => t("theme_import_err_is_ui").to_string(),
            ImportError::WrongImporter(ThemeKind::Terminal) => {
                t("theme_import_err_is_terminal").to_string()
            }
            ImportError::NotUiTheme => t("theme_import_err_not_ui").to_string(),
            ImportError::InvalidJson(e) => format!("{}: {e}", t("theme_import_err_json")),
            ImportError::MissingField(f) => format!("{}: {f}", t("theme_import_err_missing")),
            ImportError::UnrecognizedFormat => t("theme_import_err_format").to_string(),
        }
    }
}

/// Detect the format from the content and parse it. `name` is the
/// user-provided theme name.
pub(crate) fn parse_theme(content: &str, name: &str) -> Result<CustomTerminalTheme, ImportError> {
    let trimmed = content.trim_start();
    if trimmed.starts_with('{') {
        // The marker is checked FIRST here and in `parse_ui_theme`, which
        // is what makes a redirect loop structurally impossible: content
        // carrying it is a UI theme to both sides, never a scheme.
        if content.contains("\"oryxis_ui_theme\"") {
            return Err(ImportError::WrongImporter(ThemeKind::Ui));
        }
        // Any other JSON goes to the Windows Terminal parser, whose
        // per-key error ("background") is worth more to someone standing
        // in this panel than a blanket "unrecognized format".
        parse_windows_terminal(content, name)
    } else if trimmed.starts_with("<?xml") || trimmed.contains("<plist") {
        parse_iterm(content, name)
    } else if content.contains("base00") {
        parse_base16(content, name)
    } else {
        Err(ImportError::UnrecognizedFormat)
    }
}

/// POSITIVE evidence that the content is a terminal scheme, which is a
/// different question from "which parser handles it" (`parse_theme`,
/// permissive by design). Absence of the UI marker is deliberately NOT
/// evidence: guessing from absence would send a typo pasted in the
/// interface panel over to Settings > Terminal, to face an error it
/// cannot act on there either.
fn looks_like_terminal_scheme(content: &str) -> bool {
    let trimmed = content.trim_start();
    if trimmed.starts_with("<?xml") || trimmed.contains("<plist") || content.contains("base00") {
        return true;
    }
    // Windows Terminal JSON carries no marker of its own, so its shape
    // is the evidence: the background/foreground pair, or an ANSI slot.
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(content)
    else {
        return false;
    };
    let has = |k: &str| obj.get(k).is_some_and(|v| v.is_string());
    (has("background") && has("foreground"))
        || ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"]
            .iter()
            .any(|k| has(k))
}

/// Pull a display name out of a pasted / loaded scheme so the import modal
/// can pre-fill its name field: Windows Terminal JSON `"name"`, base16
/// `scheme:` line, or an Oryxis UI theme envelope's `"name"`. `None` when
/// the content carries no name (iTerm plists never do).
pub(crate) fn suggest_name(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if trimmed.starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(content).ok()?;
        return v
            .get("name")
            .and_then(|n| n.as_str())
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty());
    }
    for line in content.lines() {
        if let Some((k, val)) = line.split_once(':')
            && k.trim() == "scheme"
        {
            let name = val.trim().trim_matches('"').trim_matches('\'').to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Parse the Oryxis UI-theme JSON envelope written by
/// `theme_export::ui_theme_to_json`. Colors are matched by key
/// (`UI_COLOR_KEYS`); a missing or invalid entry falls back to the
/// Oryxis Dark value so an older file still imports after new fields are
/// added. The file's `"name"` wins over `fallback_name`.
pub(crate) fn parse_ui_theme(
    content: &str,
    fallback_name: &str,
) -> Result<CustomUiTheme, ImportError> {
    // Cheap marker probe before anything else: a terminal scheme routed
    // out of here must not be rejected as "invalid JSON" first (an iTerm
    // plist is not JSON at all), and the marker still wins over the
    // scheme shape.
    if !content.contains("\"oryxis_ui_theme\"") && looks_like_terminal_scheme(content) {
        return Err(ImportError::WrongImporter(ThemeKind::Terminal));
    }
    let v: serde_json::Value =
        serde_json::from_str(content).map_err(|e| ImportError::InvalidJson(e.to_string()))?;
    // The parsed value is the authority: the probe above only sees text,
    // so a file merely mentioning the marker inside a string lands here.
    if v.get("oryxis_ui_theme").is_none() {
        return Err(ImportError::NotUiTheme);
    }
    let colors_obj = v
        .get("colors")
        .and_then(|c| c.as_object())
        .ok_or_else(|| ImportError::MissingField("colors".to_string()))?;
    let defaults = crate::theme::theme_colors_to_hex(&crate::theme::ORYXIS_DARK);
    let colors: [String; 21] = std::array::from_fn(|i| {
        colors_obj
            .get(crate::theme_export::UI_COLOR_KEYS[i])
            .and_then(|x| x.as_str())
            .and_then(norm_hex)
            .unwrap_or_else(|| defaults[i].clone())
    });
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(fallback_name);
    Ok(CustomUiTheme::new(name.to_string(), colors))
}

fn build(
    name: &str,
    fg: String,
    bg: String,
    cursor: String,
    ansi: [String; 16],
) -> CustomTerminalTheme {
    let mut t = CustomTerminalTheme::new_default(name.to_string());
    t.foreground = fg;
    t.background = bg;
    t.cursor = cursor;
    t.ansi = ansi;
    t
}

/// Normalize a hex string to `#rrggbb` (accepts a leading `#` or not).
fn norm_hex(s: &str) -> Option<String> {
    let h = s.trim().trim_matches('"').trim_matches('\'').trim_start_matches('#');
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", h.to_lowercase()))
    } else {
        None
    }
}

fn float_to_hex(r: f32, g: f32, b: f32) -> String {
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", q(r), q(g), q(b))
}

// ---- Windows Terminal ------------------------------------------------------

fn parse_windows_terminal(s: &str, name: &str) -> Result<CustomTerminalTheme, ImportError> {
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| ImportError::InvalidJson(e.to_string()))?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).and_then(norm_hex);
    let req = |k: &str| get(k).ok_or_else(|| ImportError::MissingField(k.to_string()));

    let bg = req("background")?;
    let fg = req("foreground")?;
    let cursor = get("cursorColor").unwrap_or_else(|| fg.clone());
    // Windows Terminal names magenta "purple".
    let keys = [
        "black", "red", "green", "yellow", "blue", "purple", "cyan", "white",
        "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue",
        "brightPurple", "brightCyan", "brightWhite",
    ];
    let mut ansi: [String; 16] = std::array::from_fn(|_| String::new());
    for (i, k) in keys.iter().enumerate() {
        ansi[i] = req(k)?;
    }
    Ok(build(name, fg, bg, cursor, ansi))
}

// ---- base16 ----------------------------------------------------------------

fn parse_base16(s: &str, name: &str) -> Result<CustomTerminalTheme, ImportError> {
    let mut bases: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in s.lines() {
        if let Some((k, val)) = line.split_once(':') {
            let k = k.trim();
            if k.len() == 6
                && k.starts_with("base")
                && let Some(hex) = norm_hex(val)
            {
                bases.insert(k.to_string(), hex);
            }
        }
    }
    let b = |k: &str| {
        bases
            .get(k)
            .cloned()
            .ok_or_else(|| ImportError::MissingField(k.to_string()))
    };
    let bg = b("base00")?;
    let fg = b("base05")?;
    // Standard base16 -> ANSI mapping (shell template).
    let ansi = [
        b("base00")?, b("base08")?, b("base0B")?, b("base0A")?,
        b("base0D")?, b("base0E")?, b("base0C")?, b("base05")?,
        b("base03")?, b("base08")?, b("base0B")?, b("base0A")?,
        b("base0D")?, b("base0E")?, b("base0C")?, b("base07")?,
    ];
    Ok(build(name, fg.clone(), bg, fg, ansi))
}

// ---- iTerm2 .itermcolors ---------------------------------------------------

fn parse_iterm(s: &str, name: &str) -> Result<CustomTerminalTheme, ImportError> {
    // For a `<key>NAME</key>` color entry, read the Red/Green/Blue float
    // components from the dict that follows it.
    let color_for = |key: &str| -> Option<String> {
        let start = s.find(&format!("<key>{key}</key>"))?;
        let rest = &s[start..];
        let comp = |name: &str| -> Option<f32> {
            let ci = rest.find(&format!("<key>{name} Component</key>"))?;
            let after = &rest[ci..];
            let ri = after.find("<real>")? + "<real>".len();
            let end = after[ri..].find("</real>")?;
            after[ri..ri + end].trim().parse::<f32>().ok()
        };
        Some(float_to_hex(comp("Red")?, comp("Green")?, comp("Blue")?))
    };

    let missing = |k: &str| ImportError::MissingField(k.to_string());
    let bg = color_for("Background Color").ok_or_else(|| missing("Background Color"))?;
    let fg = color_for("Foreground Color").ok_or_else(|| missing("Foreground Color"))?;
    let cursor = color_for("Cursor Color").unwrap_or_else(|| fg.clone());
    let mut ansi: [String; 16] = std::array::from_fn(|_| String::new());
    for (i, slot) in ansi.iter_mut().enumerate() {
        *slot = color_for(&format!("Ansi {i} Color"))
            .ok_or_else(|| missing(&format!("Ansi {i} Color")))?;
    }
    Ok(build(name, fg, bg, cursor, ansi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_terminal_round_trips() {
        let json = r##"{
            "name": "Sample",
            "background": "#1e1e1e", "foreground": "#d4d4d4",
            "cursorColor": "#ffffff",
            "black": "#000000", "red": "#cd3131", "green": "#0dbc79",
            "yellow": "#e5e510", "blue": "#2472c8", "purple": "#bc3fbc",
            "cyan": "#11a8cd", "white": "#e5e5e5",
            "brightBlack": "#666666", "brightRed": "#f14c4c",
            "brightGreen": "#23d18b", "brightYellow": "#f5f543",
            "brightBlue": "#3b8eea", "brightPurple": "#d670d6",
            "brightCyan": "#29b8db", "brightWhite": "#ffffff"
        }"##;
        let t = parse_theme(json, "My WT").unwrap();
        assert_eq!(t.name, "My WT");
        assert_eq!(t.background, "#1e1e1e");
        assert_eq!(t.ansi[1], "#cd3131"); // red
        assert_eq!(t.ansi[5], "#bc3fbc"); // purple -> magenta slot
        assert_eq!(t.ansi[15], "#ffffff");
    }

    #[test]
    fn base16_maps_to_ansi() {
        let yaml = "scheme: \"Test\"\nbase00: \"1d1f21\"\nbase01: \"282a2e\"\n\
            base02: \"373b41\"\nbase03: \"969896\"\nbase04: \"b4b7b4\"\n\
            base05: \"c5c8c6\"\nbase06: \"e0e0e0\"\nbase07: \"ffffff\"\n\
            base08: \"cc6666\"\nbase09: \"de935f\"\nbase0A: \"f0c674\"\n\
            base0B: \"b5bd68\"\nbase0C: \"8abeb7\"\nbase0D: \"81a2be\"\n\
            base0E: \"b294bb\"\nbase0F: \"a3685a\"\n";
        let t = parse_theme(yaml, "B16").unwrap();
        assert_eq!(t.background, "#1d1f21"); // base00
        assert_eq!(t.foreground, "#c5c8c6"); // base05
        assert_eq!(t.ansi[1], "#cc6666"); // red = base08
        assert_eq!(t.ansi[2], "#b5bd68"); // green = base0B
        assert_eq!(t.ansi[15], "#ffffff"); // bright white = base07
    }

    #[test]
    fn iterm_floats_to_hex() {
        let xml = r#"<?xml version="1.0"?>
        <plist version="1.0"><dict>
        <key>Background Color</key>
        <dict><key>Red Component</key><real>0.0</real>
        <key>Green Component</key><real>0.0</real>
        <key>Blue Component</key><real>0.0</real></dict>
        <key>Foreground Color</key>
        <dict><key>Red Component</key><real>1.0</real>
        <key>Green Component</key><real>1.0</real>
        <key>Blue Component</key><real>1.0</real></dict>
        <key>Ansi 1 Color</key>
        <dict><key>Red Component</key><real>1.0</real>
        <key>Green Component</key><real>0.0</real>
        <key>Blue Component</key><real>0.0</real></dict>
        </dict></plist>"#;
        // Needs all 16 ANSI keys; this minimal sample should fail cleanly.
        let err = parse_theme(xml, "iT").unwrap_err();
        assert_eq!(err, ImportError::MissingField("Ansi 0 Color".to_string()));
    }

    #[test]
    fn iterm_full_parses() {
        let mut xml = String::from("<?xml version=\"1.0\"?>\n<plist><dict>\n");
        let comp = |r: f32, g: f32, b: f32| {
            format!(
                "<dict><key>Red Component</key><real>{r}</real>\
                 <key>Green Component</key><real>{g}</real>\
                 <key>Blue Component</key><real>{b}</real></dict>"
            )
        };
        xml.push_str(&format!("<key>Background Color</key>{}\n", comp(0.1, 0.1, 0.1)));
        xml.push_str(&format!("<key>Foreground Color</key>{}\n", comp(1.0, 1.0, 1.0)));
        for i in 0..16 {
            xml.push_str(&format!("<key>Ansi {i} Color</key>{}\n", comp(1.0, 0.0, 0.0)));
        }
        xml.push_str("</dict></plist>");
        let t = parse_theme(&xml, "iT").unwrap();
        assert_eq!(t.foreground, "#ffffff");
        assert_eq!(t.ansi[0], "#ff0000");
    }

    #[test]
    fn unknown_format_errors() {
        assert_eq!(
            parse_theme("hello world", "x").unwrap_err(),
            ImportError::UnrecognizedFormat
        );
    }

    #[test]
    fn ui_theme_file_is_rejected_by_terminal_importer() {
        let json = r#"{ "oryxis_ui_theme": 1, "name": "X", "colors": {} }"#;
        assert_eq!(
            parse_theme(json, "x").unwrap_err(),
            ImportError::WrongImporter(ThemeKind::Ui)
        );
    }

    #[test]
    fn terminal_scheme_in_the_ui_importer_names_its_own_panel() {
        // Windows Terminal JSON (the discussion #68 case): shape alone,
        // no marker of its own.
        let wt = r##"{ "name": "Ubuntu", "background": "#300a24",
            "foreground": "#ffffff", "black": "#2e3436" }"##;
        assert_eq!(
            parse_ui_theme(wt, "x").unwrap_err(),
            ImportError::WrongImporter(ThemeKind::Terminal)
        );
        // An iTerm plist is not even JSON, so the probe has to run
        // before the parse or this reports "invalid JSON" instead.
        let plist = "<?xml version=\"1.0\"?><plist><dict></dict></plist>";
        assert_eq!(
            parse_ui_theme(plist, "x").unwrap_err(),
            ImportError::WrongImporter(ThemeKind::Terminal)
        );
        let base16 = "scheme: \"Test\"\nbase00: \"1d1f21\"\n";
        assert_eq!(
            parse_ui_theme(base16, "x").unwrap_err(),
            ImportError::WrongImporter(ThemeKind::Terminal)
        );
    }

    #[test]
    fn junk_in_the_ui_importer_stays_put() {
        // The redirect needs POSITIVE evidence: anything else must fail
        // here rather than move the user to a panel that would reject it
        // too. This is the guard on the whole redirect feature.
        for junk in ["hello world", "{}", "{ \"foreground\": \"#ffffff\" }", ""] {
            let err = parse_ui_theme(junk, "x").unwrap_err();
            assert!(
                !matches!(err, ImportError::WrongImporter(_)),
                "{junk:?} should not be routed to the terminal panel, got {err:?}"
            );
        }
    }

    #[test]
    fn the_marker_wins_over_the_scheme_shape() {
        // A UI theme that also carries scheme-shaped keys resolves to UI
        // on BOTH sides, which is what makes a redirect loop impossible.
        let json = r##"{ "oryxis_ui_theme": 1, "name": "X",
            "background": "#000000", "foreground": "#ffffff",
            "colors": { "bg_primary": "#101010" } }"##;
        assert_eq!(
            parse_theme(json, "x").unwrap_err(),
            ImportError::WrongImporter(ThemeKind::Ui)
        );
        assert!(parse_ui_theme(json, "x").is_ok());
    }

    #[test]
    fn suggest_name_from_wt_and_base16() {
        assert_eq!(
            suggest_name(r##"{ "name": "Dracula", "background": "#000000" }"##),
            Some("Dracula".to_string())
        );
        assert_eq!(
            suggest_name("scheme: \"Tomorrow Night\"\nbase00: \"1d1f21\"\n"),
            Some("Tomorrow Night".to_string())
        );
        assert_eq!(suggest_name("<?xml version=\"1.0\"?>"), None);
    }

    #[test]
    fn every_error_renders_a_real_string() {
        // `i18n::en` answers "???" for a key it does not know, so a typo
        // in `localized` reaches the user as punctuation instead of
        // failing to compile. This is what catches it.
        for err in [
            ImportError::WrongImporter(ThemeKind::Ui),
            ImportError::WrongImporter(ThemeKind::Terminal),
            ImportError::NotUiTheme,
            ImportError::InvalidJson("trailing comma".to_string()),
            ImportError::MissingField("background".to_string()),
            ImportError::UnrecognizedFormat,
        ] {
            let msg = err.localized();
            assert!(!msg.contains("???"), "unresolved i18n key for {err:?}");
            assert!(msg.len() > 5, "suspiciously short message for {err:?}");
        }
        // The two payload-carrying ones keep the detail the user needs
        // to fix the file.
        assert!(
            ImportError::MissingField("background".to_string())
                .localized()
                .contains("background")
        );
        assert!(
            ImportError::InvalidJson("trailing comma".to_string())
                .localized()
                .contains("trailing comma")
        );
    }

    #[test]
    fn ui_theme_missing_keys_fall_back_to_defaults() {
        let json = r##"{
            "oryxis_ui_theme": 1,
            "name": "Partial",
            "colors": { "bg_primary": "#101010", "accent": "#ff0000" }
        }"##;
        let t = parse_ui_theme(json, "fb").unwrap();
        assert_eq!(t.name, "Partial");
        assert_eq!(t.colors[0], "#101010");
        assert_eq!(t.colors[8], "#ff0000");
        // Unlisted key falls back to the Oryxis Dark value.
        let defaults =
            crate::theme::theme_colors_to_hex(&crate::theme::ORYXIS_DARK);
        assert_eq!(t.colors[20], defaults[20]);
        // No marker -> error.
        assert!(parse_ui_theme(r#"{ "colors": {} }"#, "fb").is_err());
    }
}
