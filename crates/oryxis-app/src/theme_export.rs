//! Export custom themes to shareable JSON files.
//!
//! Terminal themes serialize as a **Windows Terminal** scheme object, the
//! most widely understood interchange format for 16-color palettes; our own
//! importer (`theme_import::parse_theme`) reads it back, so an Oryxis export
//! round-trips and also drops straight into Windows Terminal or any tool
//! that accepts its schemes.
//!
//! UI (chrome) themes have no ecosystem format, so they use a small Oryxis
//! JSON envelope: a `oryxis_ui_theme` version marker, the name, and a
//! `colors` object keyed by the `ThemeColors` field names
//! (`UI_COLOR_KEYS`). `theme_import::parse_ui_theme` is the inverse.
//!
//! Serializers are pure (`&T -> String`) so they unit-test without a vault
//! or UI.

use oryxis_core::models::custom_terminal_theme::CustomTerminalTheme;
use oryxis_core::models::custom_ui_theme::CustomUiTheme;

/// Canonical JSON keys for the 21 UI colors, in `UI_COLOR_FIELDS` order.
/// These are the `ThemeColors` field names, so the file is self-describing
/// and order-independent on import.
pub(crate) const UI_COLOR_KEYS: [&str; 21] = [
    "bg_primary",
    "bg_sidebar",
    "bg_surface",
    "bg_hover",
    "bg_selected",
    "text_primary",
    "text_secondary",
    "text_muted",
    "accent",
    "accent_hover",
    "success",
    "warning",
    "error",
    "terminal_bg",
    "terminal_fg",
    "terminal_cursor",
    "border",
    "border_focus",
    "button_bg",
    "button_bg_hover",
    "button_text",
];

/// Windows Terminal ANSI slot names in ANSI 0-15 order (magenta is
/// "purple" there, matching `theme_import::parse_windows_terminal`).
const WT_ANSI_KEYS: [&str; 16] = [
    "black", "red", "green", "yellow", "blue", "purple", "cyan", "white",
    "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue",
    "brightPurple", "brightCyan", "brightWhite",
];

/// A terminal theme as a pretty-printed Windows Terminal scheme object.
pub(crate) fn terminal_theme_to_json(t: &CustomTerminalTheme) -> String {
    let mut obj = serde_json::Map::new();
    obj.insert("name".into(), t.name.clone().into());
    obj.insert("background".into(), t.background.clone().into());
    obj.insert("foreground".into(), t.foreground.clone().into());
    obj.insert("cursorColor".into(), t.cursor.clone().into());
    for (i, key) in WT_ANSI_KEYS.iter().enumerate() {
        obj.insert((*key).into(), t.ansi[i].clone().into());
    }
    // Map preserves insertion order only with the serde_json
    // `preserve_order` feature; either way the file stays valid.
    serde_json::to_string_pretty(&serde_json::Value::Object(obj))
        .unwrap_or_default()
}

/// A UI (chrome) theme as the Oryxis JSON envelope.
pub(crate) fn ui_theme_to_json(t: &CustomUiTheme) -> String {
    let mut colors = serde_json::Map::new();
    for (i, key) in UI_COLOR_KEYS.iter().enumerate() {
        colors.insert((*key).into(), t.colors[i].clone().into());
    }
    let mut obj = serde_json::Map::new();
    obj.insert("oryxis_ui_theme".into(), 1.into());
    obj.insert("name".into(), t.name.clone().into());
    obj.insert("colors".into(), serde_json::Value::Object(colors));
    serde_json::to_string_pretty(&serde_json::Value::Object(obj))
        .unwrap_or_default()
}

/// Seed name for a cloned theme: `"{base} ({suffix})"`, then
/// `"{base} ({suffix} 2)"` and so on until `taken` clears. `suffix` is the
/// localized "copy" word.
pub(crate) fn unique_copy_name(
    base: &str,
    suffix: &str,
    taken: impl Fn(&str) -> bool,
) -> String {
    let first = format!("{base} ({suffix})");
    if !taken(&first) {
        return first;
    }
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base} ({suffix} {n})");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Keep only filesystem-safe characters for a suggested export file name.
pub(crate) fn sanitize_theme_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "theme".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_terminal_theme() -> CustomTerminalTheme {
        let mut t = CustomTerminalTheme::new_default("Roundtrip".to_string());
        t.foreground = "#d4d4d4".into();
        t.background = "#1e1e1e".into();
        t.cursor = "#ffcc00".into();
        t.ansi[5] = "#bc3fbc".into(); // magenta -> "purple" key
        t
    }

    #[test]
    fn terminal_export_round_trips_through_importer() {
        let original = sample_terminal_theme();
        let json = terminal_theme_to_json(&original);
        let parsed =
            crate::theme_import::parse_theme(&json, &original.name).unwrap();
        assert_eq!(parsed.foreground, original.foreground);
        assert_eq!(parsed.background, original.background);
        assert_eq!(parsed.cursor, original.cursor);
        assert_eq!(parsed.ansi, original.ansi);
    }

    #[test]
    fn ui_export_round_trips_through_importer() {
        let colors: [String; 21] =
            std::array::from_fn(|i| format!("#0000{:02x}", i));
        let original = CustomUiTheme::new("Chrome RT".to_string(), colors);
        let json = ui_theme_to_json(&original);
        let parsed =
            crate::theme_import::parse_ui_theme(&json, "fallback").unwrap();
        assert_eq!(parsed.name, "Chrome RT");
        assert_eq!(parsed.colors, original.colors);
    }

    #[test]
    fn unique_copy_name_dedupes() {
        let taken = ["Nord (copy)", "Nord (copy 2)"];
        let is_taken = |n: &str| taken.contains(&n);
        assert_eq!(unique_copy_name("Nord", "copy", |_| false), "Nord (copy)");
        assert_eq!(unique_copy_name("Nord", "copy", is_taken), "Nord (copy 3)");
    }

    #[test]
    fn filename_sanitized() {
        assert_eq!(sanitize_theme_filename("My Theme #2!"), "My_Theme__2_");
        assert_eq!(sanitize_theme_filename("///"), "theme");
    }
}
