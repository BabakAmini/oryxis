//! Download-mirror routing for GitHub-bound traffic (China mirror, J1).
//!
//! `raw.githubusercontent.com` is hard-blocked on mainland-China
//! networks and `api.github.com` / release assets are intermittently
//! unreachable, so the CJK font download, plugin installs and the
//! auto-updater all fail there. This module owns one setting
//! (`download_mirror`) and rewrites every GitHub-bound download URL
//! through a prefix proxy (the ghproxy convention:
//! `<base>/<full-original-url>`) when one is configured.
//!
//! Only the four GitHub download hosts are ever rewritten
//! (`api.github.com`, `github.com`, `raw.githubusercontent.com`,
//! `objects.githubusercontent.com`); any other URL is returned
//! untouched, so AI-provider, cloud or user-relay traffic can never
//! leak through a configured proxy (unit-tested guarantee).
//!
//! Trust model: content integrity never depends on the mirror. Fonts
//! are SHA-256-pinned; plugin binaries and app updates are SHA-256 +
//! Ed25519 gated against baked-in anchors. A hostile mirror can
//! withhold updates or replay an older release listing (stale-version
//! pin), but cannot cause execution of unsigned code.
//!
//! Choices: `Auto` follows the bundled default mirror when one ships
//! (none does today, so Auto is GitHub-direct; wire it up via
//! [`AUTO_MIRROR`] once an official mirror exists); `GitHubDirect`
//! never rewrites; `Custom(base)` tries the mirror first and falls
//! back to the direct URL per request (public prefix proxies often
//! proxy only the download hosts and not `api.github.com`, so the
//! fallback keeps metadata working through whichever leg answers).

use std::sync::RwLock;

/// The mirror `Auto` uses when the project ships an official one
/// (owner decision pending: self-hosted worker vs GitCode assets).
/// `None` keeps Auto identical to GitHub-direct.
const AUTO_MIRROR: Option<&str> = None;

/// The GitHub hosts a mirror may rewrite. `objects.githubusercontent`
/// is where `releases/download` URLs 302-redirect to, so it is part
/// of the blocked surface even though the app never dials it
/// directly.
const GITHUB_HOSTS: &[&str] = &[
    "api.github.com",
    "github.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
];

/// The user's mirror preference, one settings key (`download_mirror`
/// = `"auto"` / `"github"` / an https base URL, the token-or-value
/// precedent of the `language` setting).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum MirrorChoice {
    /// Follow the bundled default mirror when one exists.
    #[default]
    Auto,
    /// Never rewrite, even when a bundled default exists.
    GitHubDirect,
    /// Always try the given prefix-proxy base first.
    Custom(String),
}

impl MirrorChoice {
    pub(crate) fn from_setting(raw: &str) -> Self {
        match raw.trim() {
            "" | "auto" => Self::Auto,
            "github" => Self::GitHubDirect,
            url => Self::Custom(url.to_string()),
        }
    }

    pub(crate) fn as_setting(&self) -> String {
        match self {
            Self::Auto => "auto".into(),
            Self::GitHubDirect => "github".into(),
            Self::Custom(url) => url.clone(),
        }
    }
}

/// Process-wide choice, seeded from the vault settings at boot and
/// updated by the Settings dispatcher. A plain std RwLock: readers
/// are download tasks that touch it once per request.
static CHOICE: RwLock<MirrorChoice> = RwLock::new(MirrorChoice::Auto);

pub(crate) fn set_choice(choice: MirrorChoice) {
    if let Ok(mut guard) = CHOICE.write() {
        *guard = choice;
    }
}

pub(crate) fn choice() -> MirrorChoice {
    CHOICE.read().map(|g| g.clone()).unwrap_or_default()
}

/// Whether `url` points at one of the GitHub download hosts (exact
/// host match on a parsed URL, not a substring test, so
/// `github.com.evil.tld` never qualifies).
fn is_github_bound(url: &str) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|host| GITHUB_HOSTS.contains(&host.as_str()))
}

/// Prefix-proxy rewrite: `<base>/<full-original-url>`.
fn rewrite(url: &str, base: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), url)
}

/// The URLs to try, in order, for a GitHub-bound download. Non-GitHub
/// URLs always come back as a single identity entry. With a mirror
/// configured the mirrored URL goes first and the direct URL stays as
/// the per-request fallback, so a proxy that only covers the download
/// hosts still leaves `api.github.com` reachable on the second leg
/// (and a dead proxy degrades to today's behavior instead of a hard
/// failure).
pub(crate) fn candidates(url: &str) -> Vec<String> {
    if !is_github_bound(url) {
        return vec![url.to_string()];
    }
    let base = match choice() {
        MirrorChoice::GitHubDirect => None,
        MirrorChoice::Custom(base) => Some(base),
        MirrorChoice::Auto => AUTO_MIRROR.map(str::to_string),
    };
    match base {
        Some(base) => vec![rewrite(url, &base), url.to_string()],
        None => vec![url.to_string()],
    }
}

/// Validate a user-entered mirror base: https and a parseable URL
/// with a host. Returns the normalized base (trailing slash trimmed).
pub(crate) fn validate_base(raw: &str) -> Result<String, ()> {
    let raw = raw.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(raw).map_err(|_| ())?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(());
    }
    Ok(raw.to_string())
}

/// Probe a mirror base for the Settings "Test" button: fetch the
/// first bytes of the pinned KR font (a real, content-addressed
/// GitHub raw URL) through the mirror, requiring TLS + HTTP 2xx/206.
/// Returns the round-trip latency in milliseconds.
pub(crate) async fn probe(base: String) -> Result<u64, String> {
    const PROBE_URL: &str = "https://raw.githubusercontent.com/google/fonts/c89741abbf4eeabce432c3ed2fd7dc28b022701e/ofl/notosanskr/NotoSansKR%5Bwght%5D.ttf";
    let client = reqwest::Client::builder()
        .user_agent(concat!("Oryxis/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(4))
        .timeout(std::time::Duration::from_secs(8))
        .https_only(true)
        .build()
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    let resp = client
        .get(rewrite(PROBE_URL, &base))
        .header(reqwest::header::RANGE, "bytes=0-127")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !(resp.status().is_success() || resp.status() == reqwest::StatusCode::PARTIAL_CONTENT) {
        return Err(format!("HTTP {}", resp.status()));
    }
    // Pull the (range-bounded) body so the probe measures a full
    // round trip, not just response headers.
    let _ = resp.bytes().await.map_err(|e| e.to_string())?;
    Ok(started.elapsed().as_millis() as u64)
}

/// Settings > Advanced UI state for the mirror block, one field on
/// `Oryxis` (`download_mirror`).
#[derive(Default)]
pub(crate) struct MirrorUi {
    /// The persisted choice (mirrors the `download_mirror` setting).
    pub(crate) choice: MirrorChoice,
    /// Live contents of the custom-URL field.
    pub(crate) url_input: String,
    /// The picker sits on "Custom" while the URL is still being
    /// typed/invalid (choice keeps the last persisted value).
    pub(crate) custom_pending: bool,
    /// The last committed URL failed https/URL validation.
    pub(crate) url_error: bool,
    /// A probe is in flight (Test button disabled).
    pub(crate) testing: bool,
    /// Outcome of the last probe: latency in ms or the failure.
    pub(crate) test_result: Option<Result<u64, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test for everything that touches the process-wide CHOICE:
    /// cargo runs tests concurrently and two tests flipping the same
    /// global would race.
    #[test]
    fn mirror_routing() {
        let base = "https://mirror.example.com";
        let cases = [
            ("https://api.github.com/repos/x/releases", true),
            ("https://github.com/x/releases/download/v1/a.bin", true),
            ("https://raw.githubusercontent.com/google/fonts/c/f.ttf", true),
            ("https://objects.githubusercontent.com/blob/x", true),
            ("https://api.anthropic.com/v1/messages", false),
            ("https://github.com.evil.tld/payload", false),
            ("https://mygithub.com/x", false),
            ("https://relay.user.example/healthz", false),
        ];
        set_choice(MirrorChoice::Custom(base.into()));
        for (url, github) in cases {
            assert_eq!(is_github_bound(url), github, "{url}");
            if github {
                assert_eq!(
                    candidates(url),
                    vec![format!("{base}/{url}"), url.to_string()],
                    "{url}"
                );
            } else {
                assert_eq!(candidates(url), vec![url.to_string()], "{url}");
            }
        }

        let url = "https://api.github.com/repos/x/releases";
        set_choice(MirrorChoice::GitHubDirect);
        assert_eq!(candidates(url), vec![url.to_string()]);
        // Auto ships no bundled mirror today, so it is direct too.
        set_choice(MirrorChoice::Auto);
        assert_eq!(candidates(url), vec![url.to_string()]);
    }

    #[test]
    fn setting_round_trip() {
        for (raw, choice) in [
            ("auto", MirrorChoice::Auto),
            ("", MirrorChoice::Auto),
            ("github", MirrorChoice::GitHubDirect),
            (
                "https://mirror.example.com",
                MirrorChoice::Custom("https://mirror.example.com".into()),
            ),
        ] {
            assert_eq!(MirrorChoice::from_setting(raw), choice);
        }
        assert_eq!(MirrorChoice::Auto.as_setting(), "auto");
        assert_eq!(MirrorChoice::GitHubDirect.as_setting(), "github");
        assert_eq!(
            MirrorChoice::from_setting(&MirrorChoice::Custom("https://m.cn".into()).as_setting()),
            MirrorChoice::Custom("https://m.cn".into())
        );
    }

    #[test]
    fn base_validation_requires_https_url() {
        assert!(validate_base("https://mirror.example.com/").is_ok());
        assert_eq!(
            validate_base("https://mirror.example.com/").unwrap(),
            "https://mirror.example.com"
        );
        assert!(validate_base("http://mirror.example.com").is_err());
        assert!(validate_base("not a url").is_err());
        assert!(validate_base("").is_err());
        assert!(validate_base("ftp://x").is_err());
    }
}
