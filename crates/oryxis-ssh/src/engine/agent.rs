/// Extract the `IdentityAgent` value from an ssh_config-style fragment.
///
/// This is the *fallback* Pageant discovery path (`windows_agent_pipe`
/// enumerates the live pipe first). Pageant (PuTTY 0.81+) and KeePassXC
/// create a per-launch agent pipe whose name changes every run and can
/// publish it as an `IdentityAgent` line in a conf file (default
/// `%USERPROFILE%\.ssh\pageant.conf`, but `--openssh-config` may point
/// it elsewhere, which is why pipe enumeration is preferred). The value
/// may use forward slashes (`//./pipe/pageant.<user>.<guid>`); normalize
/// them to the backslash form the named-pipe client expects. Accepts
/// both the `Keyword Value` and `Keyword=Value` ssh_config spellings.
#[cfg(any(windows, test))]
pub(crate) fn parse_identity_agent(contents: &str) -> Option<String> {
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(split_at) = line.find(|c: char| c.is_whitespace() || c == '=') else {
            continue;
        };
        if !line[..split_at].eq_ignore_ascii_case("IdentityAgent") {
            continue;
        }
        let val = line[split_at..]
            .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
            .trim()
            .trim_matches('"');
        if val.is_empty() {
            // Empty IdentityAgent value: keep scanning for a later valid
            // line rather than giving up on the whole file.
            continue;
        }
        return Some(val.replace('/', "\\"));
    }
    None
}

/// Collect EVERY live Pageant agent pipe matching the current user from
/// a list of named-pipe names (as returned by enumerating `\\.\pipe\`),
/// in enumeration order.
///
/// Pageant (PuTTY 0.81+) / KeePassXC publish a per-launch pipe named
/// `pageant.<user>.<guid>` where `<guid>` is randomized every run. We
/// match the current user's entries and return the full `\\.\pipe\<name>`
/// paths the named-pipe client expects. Matching is case-insensitive
/// (Win32 pipe names are), but the original name's casing is preserved
/// in the returned paths.
///
/// ALL matches are returned, not just the first: KeePassXC (database
/// locked, serving zero keys) and PuTTY Pageant both publish
/// pageant-style pipes, and pipe enumeration order is arbitrary, so
/// stopping at the first match would nondeterministically shadow the
/// agent that actually holds the key (the issue-#98 class).
///
/// When the user is unknown we fall back to any `pageant.<x>.<guid>`
/// shaped name (single-user machines, the common case), accepting the
/// small risk of another user's pipe over missing the keys entirely.
#[cfg(any(windows, test))]
pub(crate) fn pick_pageant_pipes(names: &[String], user: Option<&str>) -> Vec<String> {
    let is_match = |name: &str| -> bool {
        let lower = name.to_ascii_lowercase();
        match user {
            Some(u) if !u.is_empty() => {
                // Trailing dot pins the user segment boundary so
                // `pageant.user.` never matches `pageant.user2.<guid>`.
                let prefix = format!("pageant.{}.", u.to_ascii_lowercase());
                lower.starts_with(&prefix) && lower.len() > prefix.len()
            }
            _ => {
                lower.starts_with("pageant.")
                    && lower.matches('.').count() >= 2
                    && !lower.ends_with('.')
            }
        }
    };
    names
        .iter()
        .filter(|n| is_match(n))
        .map(|n| format!(r"\\.\pipe\{n}"))
        .collect()
}

/// First user-matching Pageant pipe, for callers that must pick exactly
/// one endpoint (the agent-forwarding bridge). Authentication uses
/// `pick_pageant_pipes` and tries every match instead.
#[cfg(any(windows, test))]
pub(crate) fn pick_pageant_pipe(names: &[String], user: Option<&str>) -> Option<String> {
    pick_pageant_pipes(names, user).into_iter().next()
}

/// Enumerate the Windows named-pipe namespace (`\\.\pipe\`), returning
/// the bare pipe names (without the `\\.\pipe\` prefix). Empty on any
/// failure, callers fall back to other discovery paths.
#[cfg(windows)]
pub(crate) fn list_named_pipes() -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindFirstFileW, FindNextFileW, WIN32_FIND_DATAW,
    };

    let pattern: Vec<u16> = std::ffi::OsStr::new(r"\\.\pipe\*")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let handle = unsafe { FindFirstFileW(pattern.as_ptr(), &mut data) };
    if handle == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut out = Vec::new();
    loop {
        let len = data
            .cFileName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(data.cFileName.len());
        let name = String::from_utf16_lossy(&data.cFileName[..len]);
        if !name.is_empty() && name != "." && name != ".." {
            out.push(name);
        }
        if unsafe { FindNextFileW(handle, &mut data) } == 0 {
            break;
        }
    }
    unsafe {
        FindClose(handle);
    }
    out
}

/// Resolve the Windows ssh-agent named pipe to dial.
///
/// Discovery order:
/// 1. The live Pageant/KeePassXC pipe, found by enumerating the
///    named-pipe namespace (`pick_pageant_pipe`). Authoritative: no
///    config file, no per-launch path to chase (`--openssh-config` can
///    point anywhere), and never stale (a `pageant.conf` can name a
///    dead guid; the live pipe is ground truth). Works even when
///    Pageant was launched without `--openssh-config`, when no conf is
///    written at all.
/// 2. A published `pageant.conf` `IdentityAgent` line at the default
///    `%USERPROFILE%\.ssh\pageant.conf` (see `parse_identity_agent`).
/// 3. The fixed Windows OpenSSH agent pipe.
#[cfg(windows)]
pub(crate) fn windows_agent_pipe() -> String {
    const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    let user = std::env::var("USERNAME").ok();
    if let Some(pipe) = pick_pageant_pipe(&list_named_pipes(), user.as_deref()) {
        tracing::info!("Using live Pageant agent pipe {pipe}");
        return pipe;
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let conf = std::path::Path::new(&profile)
            .join(".ssh")
            .join("pageant.conf");
        if let Ok(contents) = std::fs::read_to_string(&conf)
            && let Some(pipe) = parse_identity_agent(&contents)
        {
            tracing::info!("Using IdentityAgent pipe from {}", conf.display());
            return pipe;
        }
    }
    OPENSSH_PIPE.to_string()
}

/// Ordered agent endpoints for CLIENT auth, most specific first. Unlike
/// `windows_agent_pipe` (which must pick exactly one pipe, for the
/// forwarding bridge), authentication can and should FALL BACK: a live
/// Pageant-style pipe can be serving zero keys (KeePassXC with its
/// database locked keeps the pipe open) while the OpenSSH pipe holds
/// the working key, and stopping at the first pipe turns that state
/// into a spurious "no keys matched" (issue #98). EVERY live
/// pageant-style pipe is a candidate for the same reason: two of them
/// can coexist (locked KeePassXC next to PuTTY Pageant) and the
/// enumeration order must not decide which one gets dialed. The Oryxis
/// agent's own pipe closes the chain so vault keys still answer when
/// the OpenSSH alias name was squatted by the Windows agent service.
/// Case-insensitive dedup (Win32 pipe names are case-insensitive).
#[cfg(any(windows, test))]
pub(crate) fn windows_agent_pipe_candidates(
    names: &[String],
    user: Option<&str>,
    pageant_conf: Option<&str>,
) -> Vec<String> {
    const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    let mut out: Vec<String> = Vec::new();
    let push = |out: &mut Vec<String>, p: String| {
        if !out.iter().any(|e| e.eq_ignore_ascii_case(&p)) {
            out.push(p);
        }
    };
    for p in pick_pageant_pipes(names, user) {
        push(&mut out, p);
    }
    if let Some(conf) = pageant_conf
        && let Some(p) = parse_identity_agent(conf)
    {
        push(&mut out, p);
    }
    push(&mut out, OPENSSH_PIPE.to_string());
    // Deliberately NOT the Oryxis agent's own pipe: dialing our own
    // in-process agent to auth our own connection is redundant (the
    // engine already offers the connection's key directly in the
    // publickey phase) and would trip the agent-server confirm prompt
    // on a connection the user just initiated in this same app. External
    // tools still reach the pipe directly; we simply don't ask ourselves.
    out
}

/// Live wrapper over `windows_agent_pipe_candidates`: enumerates the
/// pipe namespace and reads the default `pageant.conf`, mirroring the
/// discovery inputs of `windows_agent_pipe`.
#[cfg(windows)]
pub(crate) fn agent_pipe_candidates() -> Vec<String> {
    let user = std::env::var("USERNAME").ok();
    let conf = std::env::var_os("USERPROFILE").and_then(|profile| {
        std::fs::read_to_string(
            std::path::Path::new(&profile).join(".ssh").join("pageant.conf"),
        )
        .ok()
    });
    windows_agent_pipe_candidates(&list_named_pipes(), user.as_deref(), conf.as_deref())
}

/// The agent endpoints that EXIST right now, as an environment
/// fingerprint rather than a dial list: every live pageant-style pipe
/// plus the fixed OpenSSH name when something is actually serving it.
/// Sorted and case-insensitively deduped so the value changes only when
/// an agent really came or went, never because the pipe namespace
/// enumerated in a different order (`FindFirstFile` order is arbitrary).
///
/// Unlike `windows_agent_pipe_candidates`, the OpenSSH name is included
/// ONLY when present: a candidate list may end on a name nobody serves
/// (dialing it just fails), but a fingerprint must not claim an agent
/// that isn't there, or the "an agent appeared" edge would never fire.
#[cfg(any(windows, test))]
pub(crate) fn agent_endpoint_names(names: &[String], user: Option<&str>) -> Vec<String> {
    const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    let mut out = pick_pageant_pipes(names, user);
    if names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("openssh-ssh-agent"))
    {
        out.push(OPENSSH_PIPE.to_string());
    }
    out.sort_by_key(|p| p.to_ascii_lowercase());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

/// Live wrapper over `agent_endpoint_names`, mirroring the discovery
/// inputs of `agent_pipe_candidates` minus the config file (a
/// `pageant.conf` line can name a dead guid, and a fingerprint must
/// report what is actually up).
#[cfg(windows)]
pub(crate) fn agent_endpoints_present() -> Vec<String> {
    let user = std::env::var("USERNAME").ok();
    agent_endpoint_names(&list_named_pipes(), user.as_deref())
}

/// Unix half of the same fingerprint: `SSH_AUTH_SOCK` when the socket
/// is actually on disk. An exported-but-dead `SSH_AUTH_SOCK` (the agent
/// died, the systemd socket unit hasn't started) reads as "no agent",
/// so the socket appearing later registers as a change.
#[cfg(any(unix, test))]
pub(crate) fn unix_agent_endpoints(env_sock: Option<String>) -> Vec<String> {
    unix_agent_sock_candidates(env_sock)
        .into_iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect()
}

/// How long one endpoint gets to answer the census. Shorter than the
/// auth sweep's `AGENT_DIAL_TIMEOUT`: the census is a background
/// heartbeat, so a wedged endpoint must not eat the interval that
/// drives it, and unlike auth there is nothing to salvage by waiting.
const CENSUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Fold one endpoint's LIST answer into census lines.
///
/// A key ARRIVING in an agent that was already running (KeePassXC
/// pushing into the always-on Windows OpenSSH service) moves nothing in
/// the endpoint set, which is why the census goes down to fingerprints.
/// The three states are deliberately distinct: holding keys, reachable
/// but empty (a locked KeePassXC keeps its pipe open), and unreachable.
pub(crate) fn census_lines(
    endpoint: &str,
    identities: Option<Vec<russh::keys::agent::AgentIdentity>>,
) -> Vec<String> {
    match identities {
        Some(ids) if !ids.is_empty() => ids
            .iter()
            .map(|id| {
                format!(
                    "{endpoint} {}",
                    id.public_key().fingerprint(russh::keys::HashAlg::Sha256)
                )
            })
            .collect(),
        Some(_) => vec![format!("{endpoint} <empty>")],
        // Unreachable or timed out contributes nothing, so the endpoint
        // answering later reads as a change.
        None => Vec::new(),
    }
}

/// What every reachable agent is holding right now, as sorted
/// `<endpoint> <fingerprint>` lines. Meaningless in isolation: callers
/// compare two readings, and a difference means the key situation moved
/// (an agent appeared, went away, or its roster changed).
///
/// LIST is the only request made. It is unauthenticated in every agent
/// implementation and never triggers a confirm prompt (only signing
/// does), so polling it cannot nag the user.
#[cfg(unix)]
pub(crate) async fn agent_key_census() -> Vec<String> {
    let mut out = Vec::new();
    for path in unix_agent_endpoints(std::env::var("SSH_AUTH_SOCK").ok()) {
        let ids = tokio::time::timeout(CENSUS_TIMEOUT, async {
            let mut agent = russh::keys::agent::client::AgentClient::connect_uds(&path)
                .await
                .ok()?;
            agent.request_identities().await.ok()
        })
        .await
        .ok()
        .flatten();
        out.extend(census_lines(&path, ids));
    }
    out.sort();
    out
}

#[cfg(windows)]
pub(crate) async fn agent_key_census() -> Vec<String> {
    let mut out = Vec::new();
    for pipe in agent_endpoints_present() {
        let ids = tokio::time::timeout(CENSUS_TIMEOUT, async {
            let mut agent = russh::keys::agent::client::AgentClient::connect_named_pipe(&pipe)
                .await
                .ok()?;
            agent.request_identities().await.ok()
        })
        .await
        .ok()
        .flatten();
        out.extend(census_lines(&pipe, ids));
    }
    out.sort();
    out
}

#[cfg(not(any(unix, windows)))]
pub(crate) async fn agent_key_census() -> Vec<String> {
    Vec::new()
}

/// Agent sockets for CLIENT auth on Unix: just `SSH_AUTH_SOCK` (empty
/// or unset yields no candidate). Deliberately NOT the Oryxis agent's
/// own socket: dialing our own in-process agent to auth our own
/// connection is redundant (the engine already offers the connection's
/// key directly in the publickey phase) and would trip the agent-server
/// confirm prompt on a connection the user just initiated in this same
/// app. A user who explicitly points `SSH_AUTH_SOCK` at the Oryxis
/// socket still routes here, by their own choice.
#[cfg(any(unix, test))]
pub(crate) fn unix_agent_sock_candidates(
    env_sock: Option<String>,
) -> Vec<std::path::PathBuf> {
    env_sock
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .into_iter()
        .collect()
}

/// Bridge an inbound agent-forward channel to the local ssh-agent so
/// the remote side can use the keys held by our local agent. The remote
/// app speaks ssh-agent protocol over the channel; we just shovel raw
/// bytes between the channel and the local socket / pipe.
#[cfg(unix)]
pub(crate) async fn bridge_agent_channel(
    channel: russh::Channel<russh::client::Msg>,
) -> std::io::Result<()> {
    let path = std::env::var_os("SSH_AUTH_SOCK").ok_or_else(|| {
        std::io::Error::other("agent forwarding requested but SSH_AUTH_SOCK is not set")
    })?;
    let mut agent = tokio::net::UnixStream::connect(&path).await?;
    let mut stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut agent, &mut stream).await?;
    Ok(())
}

#[cfg(windows)]
pub(crate) async fn bridge_agent_channel(
    channel: russh::Channel<russh::client::Msg>,
) -> std::io::Result<()> {
    let pipe_path = windows_agent_pipe();
    let mut agent = tokio::net::windows::named_pipe::ClientOptions::new().open(&pipe_path)?;
    let mut stream = channel.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut agent, &mut stream).await?;
    Ok(())
}
