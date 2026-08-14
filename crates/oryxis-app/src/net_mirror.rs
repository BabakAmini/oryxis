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
//! Choices: `Auto` goes to GitHub first and falls back to the
//! project's asset host per request (owner decision 2026-07-11:
//! GitHub stays the default so the mirror only carries traffic that
//! actually needs it); `GitHubDirect` never falls back;
//! `Custom(base)` tries the user's prefix proxy first and falls back
//! to the direct URL (public proxies often cover only the download
//! hosts and not `api.github.com`, so the fallback keeps metadata
//! working through whichever leg answers).
//!
//! The asset host is NOT a prefix proxy: it is a static bucket
//! (Cloudflare R2 behind `dl.oryxis.app`, fronted for mainland China
//! by Tencent EdgeOne at `dl-cn.oryxis.app`) that the release
//! workflows populate with a fixed layout, plus API-response
//! snapshots so release METADATA works without `api.github.com`:
//!
//! - `fonts/<file>`: the pinned CJK font files
//! - `releases/<tag>/<asset>`: release assets (installers, plugin
//!   binaries, manifests, `.sig` sidecars)
//! - `releases/latest.json`: snapshot of `repos/.../releases/latest`
//! - `releases/nightly.json`: snapshot of `releases/tags/nightly-latest`
//!   (file name kept from the pre-rename `nightly` tag so binaries
//!   built before 2026-07-16 still resolve it through the fallback)
//! - `releases/index.json`: snapshot of `releases?per_page=30`

use std::sync::RwLock;

/// The asset host `Auto` falls back to when GitHub is unreachable.
const AUTO_ASSET_HOST: &str = "https://dl-cn.oryxis.app";

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

/// The hosts a GitHub-bound download may talk to under the CURRENT
/// mirror choice, for error surfaces that want to say exactly what a
/// blocked network needs to allow (discussion #163): the four GitHub
/// hosts, plus the fallback asset host under Auto, plus a custom
/// proxy's own host. Order matches dial order.
pub(crate) fn consulted_hosts() -> Vec<String> {
    let mut hosts: Vec<String> = GITHUB_HOSTS.iter().map(|h| (*h).to_string()).collect();
    match choice() {
        MirrorChoice::Auto => hosts.push(host_of(AUTO_ASSET_HOST)),
        MirrorChoice::GitHubDirect => {}
        MirrorChoice::Custom(base) => {
            let host = host_of(&base);
            if !host.is_empty() && !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

/// The bare host of an `https://host[/path]` base, for display.
fn host_of(base: &str) -> String {
    base.split("://")
        .nth(1)
        .unwrap_or(base)
        .split('/')
        .next()
        .unwrap_or("")
        .to_string()
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

/// The asset-host path for a GitHub URL the release workflows also
/// publish to the bucket, or `None` for URLs the host does not carry
/// (those stay direct-only under `Auto`). Kept in lockstep with the
/// `publish-mirror` layout in `.github/workflows/`.
fn asset_path(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let path = parsed.path();
    let repo = crate::plugins::RELEASE_REPO;
    match host {
        // The pinned CJK fonts. The client path keeps the percent-encoded
        // file name from the pinned raw URL; the mirror/bucket key is the
        // DECODED name (the server decodes request paths before matching),
        // so publish-mirror.yml stores decoded keys (see a038c67).
        "raw.githubusercontent.com"
            if path.starts_with("/google/fonts/")
                || path.starts_with("/ryanoasis/nerd-fonts/") =>
        {
            // Both pinned font sources (CJK from google/fonts, the
            // terminal font pack from ryanoasis/nerd-fonts) flatten
            // to `fonts/<file>` on the asset host.
            let file = url.rsplit('/').next()?;
            (!file.is_empty()).then(|| format!("fonts/{file}"))
        }
        // Release metadata: snapshots of the API responses.
        "api.github.com" => {
            let rest = path.strip_prefix(&format!("/repos/{repo}/"))?;
            match rest {
                "releases/latest" => Some("releases/latest.json".into()),
                // The rolling tag renamed from `nightly` on 2026-07-16
                // (immutable-release tag burn); the snapshot keeps the
                // old file name, see the module doc.
                "releases/tags/nightly-latest" => Some("releases/nightly.json".into()),
                "releases" => Some("releases/index.json".into()),
                _ => None,
            }
        }
        // Release assets (installers, plugin binaries, manifests and
        // their .sig sidecars): `releases/<tag>/<asset>`.
        "github.com" => {
            let rest = path.strip_prefix(&format!("/{repo}/releases/download/"))?;
            (rest.split('/').count() == 2).then(|| format!("releases/{rest}"))
        }
        _ => None,
    }
}

/// The URLs to try, in order, for a GitHub-bound download. Non-GitHub
/// URLs always come back as a single identity entry.
///
/// - `Auto`: GitHub first; when the asset host carries the file, its
///   URL is the per-request fallback (owner decision: the mirror only
///   serves traffic that actually needs it).
/// - `Custom(base)`: the user's prefix proxy first, direct as the
///   fallback, so a proxy that only covers the download hosts still
///   leaves `api.github.com` reachable on the second leg (and a dead
///   proxy degrades to today's behavior instead of a hard failure).
pub(crate) fn candidates(url: &str) -> Vec<String> {
    if !is_github_bound(url) {
        return vec![url.to_string()];
    }
    match choice() {
        MirrorChoice::GitHubDirect => vec![url.to_string()],
        MirrorChoice::Custom(base) => vec![rewrite(url, &base), url.to_string()],
        MirrorChoice::Auto => match asset_path(url) {
            Some(path) => vec![url.to_string(), format!("{AUTO_ASSET_HOST}/{path}")],
            None => vec![url.to_string()],
        },
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

        // Auto: GitHub first, asset host as the fallback, only for
        // URLs the bucket layout carries.
        set_choice(MirrorChoice::Auto);
        let repo = crate::plugins::RELEASE_REPO;
        let latest = format!("https://api.github.com/repos/{repo}/releases/latest");
        assert_eq!(
            candidates(&latest),
            vec![
                latest.clone(),
                format!("{AUTO_ASSET_HOST}/releases/latest.json"),
            ]
        );
        let asset = format!(
            "https://github.com/{repo}/releases/download/v1.0.0/oryxis-setup-x86_64.exe.sig"
        );
        assert_eq!(
            candidates(&asset),
            vec![
                asset.clone(),
                format!("{AUTO_ASSET_HOST}/releases/v1.0.0/oryxis-setup-x86_64.exe.sig"),
            ]
        );
        // A GitHub URL outside the published layout stays direct-only
        // (a foreign repo's releases, a gist, ...).
        assert_eq!(candidates(url), vec![url.to_string()]);

        // The allowlist an error surface shows (discussion #163)
        // follows the same choice. Asserted inside this test because
        // it shares the process-wide CHOICE with the cases above.
        assert!(consulted_hosts().contains(&"dl-cn.oryxis.app".to_string()));
        set_choice(MirrorChoice::GitHubDirect);
        assert_eq!(consulted_hosts().len(), 4);
        set_choice(MirrorChoice::Custom("https://proxy.corp.example/p".into()));
        assert!(consulted_hosts().contains(&"proxy.corp.example".to_string()));
        set_choice(MirrorChoice::Auto);
    }

    #[test]
    fn asset_paths_follow_the_bucket_layout() {
        let repo = crate::plugins::RELEASE_REPO;
        let cases = [
            (
                "https://raw.githubusercontent.com/google/fonts/c89741abbf4eeabce432c3ed2fd7dc28b022701e/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf".to_string(),
                Some("fonts/NotoSansSC%5Bwght%5D.ttf"),
            ),
            // Terminal font pack pins (issue #109) flatten to the same
            // `fonts/` prefix as the CJK pins.
            (
                "https://raw.githubusercontent.com/ryanoasis/nerd-fonts/fa7b859994228a9c8759f99c55a8d31ee92a1b5e/patched-fonts/JetBrainsMono/Ligatures/Regular/JetBrainsMonoNerdFont-Regular.ttf".to_string(),
                Some("fonts/JetBrainsMonoNerdFont-Regular.ttf"),
            ),
            (
                format!("https://api.github.com/repos/{repo}/releases/latest"),
                Some("releases/latest.json"),
            ),
            (
                format!("https://api.github.com/repos/{repo}/releases/tags/nightly-latest"),
                Some("releases/nightly.json"),
            ),
            // The burned pre-rename tag is no longer requested by this
            // binary and is not mapped.
            (
                format!("https://api.github.com/repos/{repo}/releases/tags/nightly"),
                None,
            ),
            (
                format!("https://api.github.com/repos/{repo}/releases?per_page=30"),
                Some("releases/index.json"),
            ),
            (
                format!("https://github.com/{repo}/releases/download/gcp-v0.1.0/gcp.json"),
                Some("releases/gcp-v0.1.0/gcp.json"),
            ),
            // Other repos / API paths are not published to the host.
            (
                "https://api.github.com/repos/other/repo/releases/latest".to_string(),
                None,
            ),
            (
                "https://github.com/other/repo/releases/download/v1/x.bin".to_string(),
                None,
            ),
            (
                "https://raw.githubusercontent.com/other/repo/c/file.ttf".to_string(),
                None,
            ),
        ];
        for (url, expected) in cases {
            assert_eq!(asset_path(&url).as_deref(), expected, "{url}");
        }
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
