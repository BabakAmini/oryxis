//! On-demand CJK font download + runtime load.
//!
//! Noto Sans (Latin / Cyrillic / Greek / Vietnamese) plus the Noto
//! Sans Arabic, Hebrew, Thai and Devanagari script fonts ship inside
//! the binary (see `main.rs`). The CJK scripts (Simplified and
//! Traditional Chinese, Japanese, Korean) are large (9-18 MB each) and
//! most users never need them, so they are fetched the first time the
//! user selects one of those languages, cached under
//! `~/.oryxis/fonts/`, integrity checked against a baked-in SHA-256,
//! then handed to the iced font system with `iced::font::load` so
//! cosmic-text falls back to them per codepoint.
//!
//! A failed download degrades to the system CJK font (the behaviour
//! that existed before this module) and never surfaces as a hard error.

use std::path::PathBuf;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::i18n::Language;
use crate::messages::Message;

/// Every font baked into the binary, in load order. `main.rs` feeds
/// these to the iced application builder one `.font()` call at a
/// time; the headless harness (`harness.rs`, feature `harness`) loads
/// the same list straight into the global font system, since the
/// emulator path never runs the shell's boot-time font loading.
///
/// The set, and why each entry is bundled:
/// - Lucide: the app's icon glyphs. Codicon: window chrome glyphs
///   (chrome-minimize/maximize/restore/close) matching the native
///   Windows title bar look that VS Code uses. Brand glyphs are
///   per-brand SVGs (`os_icon::BRAND_ICONS`), no font needed.
/// - Noto Sans (Regular / SemiBold / Bold): the single bundled UI
///   font across every platform, one standard look instead of per-OS
///   system fonts. Covers Latin, Latin Extended, Cyrillic, Greek and
///   Vietnamese, so most shipped languages render from the bundle
///   with no system font dependency. The three weights share the
///   "Noto Sans" typographic family (name ID 16), so weight selection
///   resolves to the right file. SIL OFL 1.1 (resources/fonts/OFL.txt).
/// - Noto Sans Arabic / Hebrew / Thai / Devanagari: small script
///   fonts (17-185 KB per weight) bundled so Arabic, Persian, Hebrew,
///   Thai and Hindi render offline; cosmic-text falls back to them
///   per-codepoint. CJK is the genuinely large script set and is
///   downloaded on demand instead (see this module).
/// - MenuCJK: tiny (~4 KB) subset holding only the glyphs of the
///   language-picker names (한국어 / 简体中文 / 繁體中文 / 日本語) so those
///   entries always render before the full CJK font is downloaded.
///   Distinct family ("Oryxis Menu CJK"), pure per-codepoint fallback.
/// - SauceCodePro Nerd Font (Regular / Medium): default terminal
///   font, Source Code Pro patched with the full Nerd Font glyph set
///   so Starship / Powerline prompts render out of the box. System
///   mono fonts lacking the PUA glyphs fall back to it per-codepoint
///   via the terminal widget's symbol_map.
/// - Symbols Nerd Font: same PUA set with no Latin coverage,
///   fallback-only so proportional text (chat, host labels, snippets)
///   can show Powerline/Devicon glyphs without going monospace.
pub static BUNDLED_FONTS: &[&[u8]] = &[
    iced_fonts::LUCIDE_FONT_BYTES,
    iced_fonts::CODICON_FONT_BYTES,
    include_bytes!("../../../resources/fonts/NotoSans-Regular.ttf"),
    include_bytes!("../../../resources/fonts/NotoSans-SemiBold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSans-Bold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansArabic-Regular.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansArabic-SemiBold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansArabic-Bold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansHebrew-Regular.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansHebrew-SemiBold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansHebrew-Bold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansThai-Regular.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansThai-SemiBold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansThai-Bold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansDevanagari-Regular.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansDevanagari-SemiBold.ttf"),
    include_bytes!("../../../resources/fonts/NotoSansDevanagari-Bold.ttf"),
    include_bytes!("../../../resources/fonts/MenuCJK.ttf"),
    include_bytes!("../../../resources/fonts/SauceCodeProNerdFont-Regular.ttf"),
    include_bytes!("../../../resources/fonts/SauceCodeProNerdFont-Medium.ttf"),
    include_bytes!("../../../resources/fonts/SymbolsNerdFont-Regular.ttf"),
];

/// One downloadable CJK font, keyed by language. Each is a Noto Sans
/// regional variable TTF (all weights in one file) pinned to an
/// immutable `google/fonts` commit *and* to its SHA-256. To re-pin or
/// move to a self-hosted mirror, change `url` and `sha256` together.
struct CjkAsset {
    /// Short language code used as the in-memory "already loaded" guard
    /// key and the cache file stem.
    code: &'static str,
    /// Cache file name under `~/.oryxis/fonts/`.
    file: &'static str,
    /// Immutable (commit-pinned) download URL.
    url: &'static str,
    /// Expected SHA-256 of the bytes, lowercase hex.
    sha256: &'static str,
    /// Expected byte length. A cheap pre-check before hashing and the
    /// cache-hit validity test (guards against a truncated file).
    len: u64,
}

// The pinned URLs below resolve against `google/fonts` commit
// `c89741abbf4eeabce432c3ed2fd7dc28b022701e`. A raw `githubusercontent`
// URL at a fixed commit is content-addressed, so the bytes can never
// change under the SHA-256 pin.

/// The four regional CJK fonts. Han unification means each regional
/// font only covers its own language's full alphabet (KR has Hangul,
/// JP has kana, SC has the simplified Han set, TC the traditional
/// set), so they are downloaded per language rather than as one
/// shared file.
static ASSETS: &[CjkAsset] = &[
    CjkAsset {
        code: "ko",
        file: "NotoSansKR.ttf",
        url: "https://raw.githubusercontent.com/google/fonts/c89741abbf4eeabce432c3ed2fd7dc28b022701e/ofl/notosanskr/NotoSansKR%5Bwght%5D.ttf",
        sha256: "194018e6b2b293a7964f037b25c0249ce1418bc9ab3c971060a03aa57861e252",
        len: 10_414_588,
    },
    CjkAsset {
        code: "zh",
        file: "NotoSansSC.ttf",
        url: "https://raw.githubusercontent.com/google/fonts/c89741abbf4eeabce432c3ed2fd7dc28b022701e/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf",
        sha256: "a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da",
        len: 17_772_300,
    },
    CjkAsset {
        code: "ja",
        file: "NotoSansJP.ttf",
        url: "https://raw.githubusercontent.com/google/fonts/c89741abbf4eeabce432c3ed2fd7dc28b022701e/ofl/notosansjp/NotoSansJP%5Bwght%5D.ttf",
        sha256: "c2f3b4d463500a2ddcd3849cded1fceeb9fd6d1c32e6cbecd568453ba50fc68f",
        len: 9_589_900,
    },
    CjkAsset {
        code: "zh-TW",
        file: "NotoSansTC.ttf",
        url: "https://raw.githubusercontent.com/google/fonts/c89741abbf4eeabce432c3ed2fd7dc28b022701e/ofl/notosanstc/NotoSansTC%5Bwght%5D.ttf",
        sha256: "864727d210d54f2537bbe23b3a839436c3992af72de9322af5270897246bd44f",
        len: 11_941_968,
    },
];

/// The CJK asset a language needs, if any.
fn asset_for(lang: Language) -> Option<&'static CjkAsset> {
    let code = match lang {
        Language::Korean => "ko",
        Language::Chinese => "zh",
        Language::Japanese => "ja",
        Language::ChineseTraditional => "zh-TW",
        _ => return None,
    };
    ASSETS.iter().find(|a| a.code == code)
}

/// The CJK language code this language needs (`"ko"`/`"zh"`/`"ja"`), or
/// `None` for languages whose scripts are already bundled.
pub fn asset_code(lang: Language) -> Option<&'static str> {
    asset_for(lang).map(|a| a.code)
}

/// `~/.oryxis/fonts/`, the same `~/.oryxis` root the vault and plugin
/// cache use. Not created here; `download` creates it on demand.
fn cache_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".oryxis").join("fonts"))
}

fn cached_path(asset: &CjkAsset) -> Option<PathBuf> {
    Some(cache_dir()?.join(asset.file))
}

/// True when the language's font is already on disk at the expected
/// size. Used to decide whether to show a "downloading" hint; the byte
/// length is a cheap validity check (a half-written file fails it) so a
/// boot existence test can't load a truncated download.
pub fn is_language_cached(lang: Language) -> bool {
    let Some(asset) = asset_for(lang) else {
        return false;
    };
    cached_path(asset)
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len() == asset.len)
        .unwrap_or(false)
}

/// Read the cached font if present and the right size, otherwise
/// download it (size-capped + SHA-256 verified, written atomically),
/// and return the bytes ready for `iced::font::load`.
async fn ensure_and_read(asset: &'static CjkAsset) -> Result<Vec<u8>, String> {
    let path = cached_path(asset).ok_or_else(|| "no home directory".to_string())?;

    if let Ok(meta) = tokio::fs::metadata(&path).await
        && meta.len() == asset.len
        && let Ok(bytes) = tokio::fs::read(&path).await
    {
        return Ok(bytes);
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("Oryxis/", env!("CARGO_PKG_VERSION")))
        // Bound the request so a stalled connection becomes the Err
        // path (system-font fallback + retry) instead of leaving the
        // "downloading" toast and the in-memory guard stuck forever.
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(90))
        // Never let a mirror redirect the font fetch to plaintext http.
        // The sha256 pin already guards integrity; this keeps the fetch
        // consistent with the update / plugin surfaces.
        .https_only(true)
        .build()
        .map_err(|e| e.to_string())?;
    // Mirror-aware: try each candidate URL (mirror first when one is
    // configured, then direct) until one answers. The SHA-256 gate
    // below keeps any mirror untrusted.
    let mut resp = None;
    let mut last_err = String::new();
    for url in crate::net_mirror::candidates(asset.url) {
        match client.get(&url).send().await.and_then(|r| r.error_for_status()) {
            Ok(r) => {
                resp = Some(r);
                break;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    let resp = resp.ok_or(last_err)?;

    // Cap a little above the pinned length so a wrong/redirected body
    // can't exhaust memory; the SHA-256 below is the real gate.
    let max = asset.len + 64 * 1024;
    let mut buf = Vec::with_capacity(asset.len as usize);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if (buf.len() as u64).saturating_add(chunk.len() as u64) > max {
            return Err(format!("font body exceeds {max} byte ceiling"));
        }
        buf.extend_from_slice(&chunk);
    }

    let digest = format!("{:x}", Sha256::digest(&buf));
    if !digest.eq_ignore_ascii_case(asset.sha256) {
        return Err(format!(
            "sha256 mismatch for {}: expected {}, got {digest}",
            asset.code, asset.sha256
        ));
    }

    // Atomic install: write a sibling .tmp, fsync it, then rename into
    // place so an interrupted download or a power loss never leaves a
    // partial file the cache-hit path would trust.
    if let Some(dir) = cache_dir() {
        let _ = tokio::fs::create_dir_all(&dir).await;
        let tmp = dir.join(format!("{}.tmp", asset.file));
        let wrote = async {
            use tokio::io::AsyncWriteExt as _;
            let mut f = tokio::fs::File::create(&tmp).await.ok()?;
            f.write_all(&buf).await.ok()?;
            f.sync_all().await.ok()?;
            Some(())
        }
        .await;
        if wrote.is_some() {
            let _ = tokio::fs::rename(&tmp, &path).await;
            // fsync the directory so the rename itself survives a power
            // loss (the durability step download.rs documents as required).
            if let Ok(d) = tokio::fs::File::open(&dir).await {
                let _ = d.sync_all().await;
            }
        } else {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
    }

    Ok(buf)
}

/// A task that ensures the CJK font for `lang` is available and loads
/// it into the iced font system. `Task::none()` for languages that
/// don't need a downloaded font. The resulting `CjkFontReady` message
/// carries the bytes (or an error) back to the update loop, which calls
/// `iced::font::load` on the main side.
pub fn ensure_task(lang: Language) -> iced::Task<Message> {
    let Some(asset) = asset_for(lang) else {
        return iced::Task::none();
    };
    let code = asset.code.to_string();
    iced::Task::perform(ensure_and_read(asset), move |res| {
        Message::CjkFontReady(code.clone(), res)
    })
}

#[cfg(test)]
mod tests {
    use crate::i18n::Language;
    use fontdb::{Database, Family, Query, Stretch, Style, Weight};

    fn db_with_noto() -> Database {
        let mut db = Database::new();
        db.load_font_data(
            include_bytes!("../../../resources/fonts/NotoSans-Regular.ttf").to_vec(),
        );
        db.load_font_data(
            include_bytes!("../../../resources/fonts/NotoSans-SemiBold.ttf").to_vec(),
        );
        db.load_font_data(
            include_bytes!("../../../resources/fonts/NotoSans-Bold.ttf").to_vec(),
        );
        db
    }

    fn resolve_weight(db: &Database, weight: Weight) -> Weight {
        let id = db
            .query(&Query {
                families: &[Family::Name("Noto Sans")],
                weight,
                stretch: Stretch::Normal,
                style: Style::Normal,
            })
            .expect("the \"Noto Sans\" family must resolve");
        db.face(id).expect("face for id").weight
    }

    /// The three bundled Noto Sans files must group under one family
    /// ("Noto Sans") so the UI's Regular (400), SemiBold (600) and Bold
    /// (700) each resolve to the right file. SemiBold's legacy family-1
    /// name is "Noto Sans SemiBold"; the grouping relies on fontdb
    /// reading the typographic family (name ID 16 = "Noto Sans"). If
    /// that breaks, weight 600 would fall back to 400/700 and every
    /// heading / tab / active chip would render at the wrong weight on
    /// every platform, the exact regression this guards headless.
    #[test]
    fn noto_sans_weights_resolve_distinctly() {
        let db = db_with_noto();
        assert_eq!(resolve_weight(&db, Weight::NORMAL), Weight::NORMAL);
        assert_eq!(resolve_weight(&db, Weight::SEMIBOLD), Weight::SEMIBOLD);
        assert_eq!(resolve_weight(&db, Weight::BOLD), Weight::BOLD);
    }

    /// The bundled MenuCJK subset must cover every glyph of the CJK
    /// language-picker names, so those entries always render from the
    /// binary even before the full on-demand CJK font is downloaded. If
    /// a `Language::name()` for a CJK language gains a character the
    /// subset doesn't carry, this fails (re-generate the subset, see the
    /// fonts memory note).
    #[test]
    fn menu_cjk_covers_picker_names() {
        let data =
            include_bytes!("../../../resources/fonts/MenuCJK.ttf").as_slice();
        let face = ttf_parser::Face::parse(data, 0).expect("MenuCJK parses");
        for lang in [
            Language::Chinese,
            Language::ChineseTraditional,
            Language::Japanese,
            Language::Korean,
        ] {
            for ch in lang.name().chars() {
                assert!(
                    face.glyph_index(ch).is_some(),
                    "MenuCJK is missing glyph {ch:?} from {} name {:?}",
                    lang.code(),
                    lang.name(),
                );
            }
        }
    }
}
