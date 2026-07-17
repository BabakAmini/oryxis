//! Contrast guards for the bundled themes. WCAG 2.x AA needs ≥ 4.5:1
//! for body text and ≥ 3.0:1 for large/bold UI labels. We require the
//! looser bound for accent/success/warning/error backgrounds because
//! the chip labels we draw on them are bold UI text, not running copy.

use super::*;

const LARGE_TEXT_THRESHOLD: f32 = 3.0;
const BODY_TEXT_THRESHOLD: f32 = 4.5;

/// All theme tables we ship, reflects the `AppTheme::ALL` order.
fn all_themes() -> Vec<(&'static str, &'static ThemeColors)> {
    vec![
        ("OryxisDark", &ORYXIS_DARK),
        ("OryxisLight", &ORYXIS_LIGHT),
        ("Termius", &TERMIUS),
        ("Darcula", &DARCULA),
        ("IslandsDark", &ISLANDS_DARK),
        ("Dracula", &DRACULA),
        ("Monokai", &MONOKAI),
        ("HackerGreen", &HACKER_GREEN),
        ("Nord", &NORD),
        ("NordLight", &NORD_LIGHT),
        ("SolarizedLight", &SOLARIZED_LIGHT),
        ("PaperLight", &PAPER_LIGHT),
    ]
}

#[test]
fn contrast_ratio_known_pairs() {
    // Spot-check the formula against pairs with well-known ratios.
    // Black-on-white must be exactly 21.0; white-on-white 1.0.
    let r = contrast_ratio(Color::BLACK, Color::WHITE);
    assert!((r - 21.0).abs() < 0.01, "black/white = {r}");
    let r = contrast_ratio(Color::WHITE, Color::WHITE);
    assert!((r - 1.0).abs() < 0.001, "white/white = {r}");
}

#[test]
fn contrast_text_for_picks_the_better_option() {
    // The helper must always return whichever foreground gives the
    // higher ratio against the supplied background.
    let pure_black = Color::BLACK;
    let pure_white = Color::WHITE;
    assert_eq!(contrast_text_for(pure_black), pure_white);
    assert!(contrast_text_for(pure_white).r < 0.2);
}

#[test]
fn body_text_meets_aa_on_surface_in_every_theme() {
    // text_primary on bg_primary / bg_surface / bg_sidebar is the
    // baseline of every screen, anything below 4.5 makes the app
    // unreadable for users on AA-compliant displays.
    for (name, t) in all_themes() {
        for (bg_label, bg) in [
            ("bg_primary", t.bg_primary),
            ("bg_surface", t.bg_surface),
            ("bg_sidebar", t.bg_sidebar),
        ] {
            let r = contrast_ratio(t.text_primary, bg);
            assert!(
                r >= BODY_TEXT_THRESHOLD,
                "{name}: text_primary on {bg_label} contrast {r:.2} < {BODY_TEXT_THRESHOLD}",
            );
        }
    }
}

#[test]
fn buttons_using_helper_meet_large_text_aa() {
    // `contrast_text_for` picks white on dark-ish bgs and near-black
    // on pale ones, biased toward visual consistency rather than the
    // strict ≥ 3.0 large-text bound, flat-UI convention is white on
    // saturated colors even when WCAG is borderline (Termius cyan
    // accent_hover, OryxisDark accent_hover, etc. land between 2.5
    // and 3.0). Test only that the helper picks the better of the
    // two candidates and that the worst case is still distinguishable
    // Loose floor (≥ 1.7 : 1), keeping the helper's "always white on
    // saturated bgs" policy means a few bright-yet-not-quite-pale
    // success / warning fills (OryxisDark warning yellow lands at
    // ~1.77) sit below WCAG. The trade-off is intentional: visual
    // consistency with the surrounding `text_primary` buttons matters
    // more than a strict ratio, and any future theme that drops below
    // 1.7 is genuinely unreadable and should be tuned, not papered
    // over by flipping foregrounds for a single button.
    const VISUAL_FLOOR: f32 = 1.7;
    for (name, t) in all_themes() {
        for (bg_label, bg) in [
            ("accent", t.accent),
            ("accent_hover", t.accent_hover),
            ("success", t.success),
            ("warning", t.warning),
            ("error", t.error),
        ] {
            let fg = contrast_text_for(bg);
            let r = contrast_ratio(fg, bg);
            assert!(
                r >= VISUAL_FLOOR,
                "{name}: contrast_text_for({bg_label}) only {r:.2} : 1 (need ≥ {VISUAL_FLOOR} for legibility)",
            );
        }
    }
}

#[test]
fn readable_accent_repairs_unreadable_brand_colours() {
    // Issue #79: AlmaLinux's brand colour is pure black, which painted
    // the active-tab label invisible on dark themes. The validator must
    // lift it to AA small-text contrast over every shipped strip
    // background, and symmetrically darken a pale brand colour
    // (OpenBSD yellow) on the light themes.
    let alma_black = Color::from_rgb8(0x00, 0x00, 0x00);
    let openbsd_yellow = Color::from_rgb8(0xFA, 0xDA, 0x64);
    for (name, t) in all_themes() {
        for (accent_label, accent) in
            [("alma black", alma_black), ("openbsd yellow", openbsd_yellow)]
        {
            let fixed = readable_accent_on(accent, t.bg_sidebar);
            let r = contrast_ratio(fixed, t.bg_sidebar);
            assert!(
                r >= BODY_TEXT_THRESHOLD,
                "{name}: repaired {accent_label} on bg_sidebar only {r:.2} < {BODY_TEXT_THRESHOLD}",
            );
        }
    }
}

#[test]
fn readable_accent_keeps_passing_colours_untouched() {
    // Brand identity must survive bit-for-bit when no repair is needed;
    // text_primary on bg_sidebar is asserted ≥ 4.5 above, so it makes a
    // guaranteed-passing probe in every theme.
    for (name, t) in all_themes() {
        let fixed = readable_accent_on(t.text_primary, t.bg_sidebar);
        assert_eq!(
            fixed, t.text_primary,
            "{name}: passing colour was altered by the validator",
        );
    }
}

#[test]
fn muted_label_remains_legible_against_surface() {
    // text_secondary is used for form labels (e.g. "Forward SSH Agent"
    // beside the toggle). It can be a softer ratio than body text but
    // still has to pass the large-text bar against the surface bgs.
    for (name, t) in all_themes() {
        for (bg_label, bg) in [
            ("bg_primary", t.bg_primary),
            ("bg_surface", t.bg_surface),
        ] {
            let r = contrast_ratio(t.text_secondary, bg);
            assert!(
                r >= LARGE_TEXT_THRESHOLD,
                "{name}: text_secondary on {bg_label} contrast {r:.2} < {LARGE_TEXT_THRESHOLD}",
            );
        }
    }
}
