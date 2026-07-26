//! Shell integration: the snippet that teaches the remote shell to report
//! its own prompt cycle (OSC 133) and, above all, the command line it just
//! parsed (`OSC 633 ; E`).
//!
//! Why this exists: the command-history capture used to read the command
//! back off the screen, which stops working the moment a multiplexer owns
//! it (issue #92). Inside tmux the grid the app sees is tmux's repaint of
//! every pane at once, so a vertical split puts two panes' text on one row.
//! The shell's own report is immune to all of that, and it is also the only
//! source that can never mistake a keystroke for a command.
//!
//! Two things have to be true for the report to arrive from inside tmux:
//! the snippet must wrap each sequence in tmux's passthrough envelope
//! (`ESC P tmux; <sequence with every ESC doubled> ESC \`), and the pane
//! must have `allow-passthrough` on (off by default since tmux 3.3). Every
//! terminal that supports OSC 133 documents the same recipe as a manual
//! chore; being an SSH client, Oryxis can do it for the user instead.
//!
//! The three levels are the user's call, not ours ([`ShellIntegrationMode`]):
//! nothing at all, a snippet that lives only as long as the session, or the
//! dotfiles on the host. The default is the one that touches nothing.

/// How far the app may go to get shell integration onto a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ShellIntegrationMode {
    /// Never inject, never install. Capture falls back to reading the
    /// screen, which means no commands from inside tmux.
    #[default]
    Off,
    /// Feed the snippet to the shell on every connect. Nothing is written
    /// on the host, so it leaves when the session does; a tmux session
    /// that was already running keeps the shells it already started.
    Session,
    /// Write the snippet into the login shell's rc file and
    /// `allow-passthrough` into `~/.tmux.conf`, so every shell on the host
    /// reports itself, including the ones tmux starts on its own.
    Persistent,
}

impl ShellIntegrationMode {
    pub const ALL: [ShellIntegrationMode; 3] =
        [Self::Off, Self::Session, Self::Persistent];

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Session => "session",
            Self::Persistent => "persistent",
        }
    }

    pub fn from_setting(value: &str) -> Self {
        match value {
            "session" => Self::Session,
            "persistent" => Self::Persistent,
            _ => Self::Off,
        }
    }

    /// i18n key of the picker label.
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Off => "shell_integration_off",
            Self::Session => "shell_integration_session",
            Self::Persistent => "shell_integration_persistent",
        }
    }
}

impl std::fmt::Display for ShellIntegrationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", crate::i18n::t(self.label_key()))
    }
}

/// Marker line opening the block the installer owns in the user's rc file.
/// Everything between it and [`RC_END`] is ours to rewrite, so a reinstall
/// replaces the old block instead of stacking another copy.
pub(crate) const RC_BEGIN: &str = "# >>> oryxis shell integration >>>";
pub(crate) const RC_END: &str = "# <<< oryxis shell integration <<<";

/// The snippet itself, with `@NONCE@` still to be substituted.
///
/// Shape, and why each piece is the way it is:
///
/// - `__oryxis_osc` decides the envelope at emission time, not at load
///   time, because the user may start tmux long after the rc file ran.
/// - `__oryxis_esc` implements the OSC 633 escaping rules (`\` doubled,
///   `;` and control characters as `\xAB`), quoting both the pattern and
///   the replacement so bash and zsh agree on what a literal backslash is.
/// - bash has no preexec hook, so the DEBUG trap stands in for one. Its
///   guard is the history number: a line the shell did not record (the
///   `HISTCONTROL=ignorespace` convention, a duplicate, or any command the
///   prompt itself runs) leaves the number untouched and is skipped. That
///   makes the trap fire once per real command line without a
///   `bash-preexec`-style interactive-mode flag, and it means a
///   deliberately unrecorded command never reaches the app either.
/// - `D` carries the exit status, which the smart-tabs timing consumes.
/// - `A`/`B` ride the prompt string so their positions are true; a shell
///   whose prompt is rebuilt by a framework may lose them, which costs
///   nothing here because the capture reads `E`, not the screen.
const SNIPPET: &str = r##"__oryxis_osc() {
  if [ -n "$TMUX" ]; then
    printf '\033Ptmux;\033\033]%s\007\033\\' "$1"
  else
    printf '\033]%s\007' "$1"
  fi
}
__oryxis_esc() {
  local s=$1
  s=${s//'\'/'\\'}
  s=${s//';'/'\x3b'}
  s=${s//$'\n'/'\x0a'}
  s=${s//$'\r'/'\x0d'}
  s=${s//$'\t'/'\x09'}
  printf '%s' "$s"
}
__oryxis_hline() {
  local h
  h=$(HISTTIMEFORMAT= builtin history 1)
  h=${h#"${h%%[![:space:]]*}"}
  __oryxis_n=${h%%[![:digit:]]*}
  h=${h#"$__oryxis_n"}
  __oryxis_l=${h:2}
}
__oryxis_pre() {
  __oryxis_ran=1
  __oryxis_osc "633;E;$(__oryxis_esc "$1");@NONCE@"
  __oryxis_osc "133;C"
}
__oryxis_post() {
  local __oryxis_st=$?
  if [ -n "$BASH_VERSION" ] && [ -z "$__oryxis_init" ]; then
    __oryxis_init=1
    __oryxis_hline
    __oryxis_hn=$__oryxis_n
  fi
  if [ -n "$__oryxis_ran" ]; then
    __oryxis_osc "133;D;$__oryxis_st"
    __oryxis_ran=
  fi
}
if [ -n "$ZSH_VERSION" ]; then
  setopt prompt_subst 2>/dev/null
  __oryxis_zpre() { __oryxis_pre "$1"; }
  autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook preexec __oryxis_zpre \
    && add-zsh-hook precmd __oryxis_post
  PS1='%{$(__oryxis_osc "133;A")%}'$PS1'%{$(__oryxis_osc "133;B")%}'
elif [ -n "$BASH_VERSION" ]; then
  __oryxis_bpre() {
    [ -n "$COMP_LINE" ] && return
    [ -n "$__oryxis_ran" ] && return
    __oryxis_hline
    [ "$__oryxis_n" = "$__oryxis_hn" ] && return
    __oryxis_hn=$__oryxis_n
    __oryxis_pre "$__oryxis_l"
  }
  trap '__oryxis_bpre' DEBUG
  PROMPT_COMMAND="__oryxis_post${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
  PS1='\[$(__oryxis_osc "133;A")\]'"$PS1"'\[$(__oryxis_osc "133;B")\]'
fi
"##;

/// The snippet with `nonce` baked in. Every `OSC 633 ; E` it emits carries
/// the value, and the pane's sniffer refuses any that doesn't: output alone
/// can then never plant a command in the user's history.
pub(crate) fn snippet(nonce: &str) -> String {
    SNIPPET.replace("@NONCE@", nonce)
}

/// The rc-file block, marker lines included.
pub(crate) fn rc_block(nonce: &str) -> String {
    format!("{RC_BEGIN}\n{}{RC_END}\n", snippet(nonce))
}

/// One line to paste into a live interactive shell (the session-scoped
/// level). The snippet travels base64-encoded so the remote line editor
/// only ever sees plain text: sending the raw source through readline
/// would have every quote, newline and control byte interpreted as a key.
/// The line brackets itself with DECSC / DECRC + erase-to-end so it wipes
/// its own echo, the same trick the OSC 7 injection uses.
pub(crate) fn session_inject(nonce: &str) -> String {
    // DECSC, run, then DECRC + step over the one echoed line + erase to the
    // end of the screen, which wipes the echo however many rows it wrapped
    // to without touching the MOTD above it.
    format!("printf '\\x1b7'\n{}; printf '\\x1b8\\x1b[1A\\x1b[J'\n", one_liner(&snippet(nonce)))
}

/// One line that runs [`install_script`] on the live shell. Deliberately
/// NOT self-clearing: writing to the user's dotfiles is exactly the kind of
/// thing they should see happen, so the script's one-line report stays on
/// screen.
pub(crate) fn install_line(nonce: &str) -> String {
    format!("{}\n", one_liner(&install_script(nonce)))
}

/// One line that runs [`uninstall_script`] on the live shell, for leaving
/// the persistent level. Visible like the install, for the same reason.
pub(crate) fn uninstall_line() -> String {
    format!("{}\n", one_liner(&uninstall_script()))
}

/// Wrap a script so it can be typed into a live interactive shell. The
/// source travels base64-encoded because the remote line editor is in raw
/// mode: a quote, a newline or a control byte in the source would be read
/// as a keypress, not as text. `base64 -d` is GNU / busybox and
/// `--decode` is the BSD (macOS) spelling, so both are tried.
fn one_liner(script: &str) -> String {
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::STANDARD.encode(script);
    format!(
        "eval \"$(printf '%s' '{payload}' | base64 -d 2>/dev/null \
         || printf '%s' '{payload}' | base64 --decode)\""
    )
}

/// Script for the persistent level, run on an exec channel. It rewrites
/// its own block in the login shell's rc file (never appends a second
/// copy), turns on `allow-passthrough` in `~/.tmux.conf`, and applies it
/// to a tmux server that is already running so the setting takes effect
/// without a restart. Every step is idempotent, and the rc file is
/// rewritten in place (`cat >`), so its permissions and inode survive.
pub(crate) fn install_script(nonce: &str) -> String {
    let block = rc_block(nonce);
    format!(
        r##"set -e
sh_path="${{SHELL:-}}"
[ -n "$sh_path" ] || sh_path=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)
case "$sh_path" in
  *zsh) rc="${{ZDOTDIR:-$HOME}}/.zshrc" ;;
  *bash) rc="$HOME/.bashrc" ;;
  *) echo "oryxis: unsupported login shell: ${{sh_path:-unknown}}" >&2; exit 2 ;;
esac
touch "$rc"
if grep -qs '{nonce}' "$rc"; then
  echo "oryxis: shell integration already current in $rc"
  exit 0
fi
tmp="$rc.oryxis.$$"
awk '/^# >>> oryxis shell integration >>>$/{{skip=1}} !skip{{print}} /^# <<< oryxis shell integration <<<$/{{skip=0}}' "$rc" > "$tmp"
cat "$tmp" > "$rc"
rm -f "$tmp"
cat >> "$rc" <<'ORYXIS_SI_EOF'
{block}ORYXIS_SI_EOF
tc="$HOME/.tmux.conf"
if ! grep -qs 'allow-passthrough' "$tc"; then
  printf '%s\n' 'set -g allow-passthrough on' >> "$tc"
fi
if command -v tmux >/dev/null 2>&1 && tmux list-sessions >/dev/null 2>&1; then
  tmux set -g allow-passthrough on >/dev/null 2>&1 || true
fi
echo "oryxis: shell integration installed in $rc"
"##
    )
}

/// Script that removes everything [`install_script`] wrote, for turning the
/// persistent level back off. `allow-passthrough` is deliberately left
/// alone: other tools (image protocols, other terminals' integrations)
/// rely on it, and it is not ours to revoke.
pub(crate) fn uninstall_script() -> String {
    r##"set -e
sh_path="${SHELL:-}"
[ -n "$sh_path" ] || sh_path=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7)
case "$sh_path" in
  *zsh) rc="${ZDOTDIR:-$HOME}/.zshrc" ;;
  *bash) rc="$HOME/.bashrc" ;;
  *) exit 0 ;;
esac
[ -f "$rc" ] || exit 0
tmp="$rc.oryxis.$$"
awk '/^# >>> oryxis shell integration >>>$/{skip=1} !skip{print} /^# <<< oryxis shell integration <<<$/{skip=0}' "$rc" > "$tmp"
cat "$tmp" > "$rc"
rm -f "$tmp"
echo "oryxis: shell integration removed from $rc"
"##
    .to_string()
}

/// A fresh nonce for a vault that has none yet.
pub(crate) fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

impl crate::app::Oryxis {
    /// This vault's nonce, minted and persisted the first time it is
    /// needed. It has to survive restarts because the persistent level
    /// leaves it sitting in the host's rc file.
    pub(crate) fn shell_integration_nonce(&mut self) -> String {
        if self.shell_integration_nonce.is_empty() {
            let nonce = new_nonce();
            self.shell_integration_nonce = nonce.clone();
            self.persist_setting("shell_integration_nonce", &nonce);
        }
        self.shell_integration_nonce.clone()
    }

    /// Put shell integration on a freshly connected pane, as the current
    /// level allows. SSH only: the snippet is fed through the pane's own
    /// shell, and a serial line or a Telnet host has no shell contract to
    /// rely on.
    pub(crate) fn apply_shell_integration(&mut self, tab_idx: usize, pane_id: uuid::Uuid) {
        let mode = self.setting_shell_integration;
        if mode == ShellIntegrationMode::Off {
            return;
        }
        let nonce = self.shell_integration_nonce();
        let Some(tab) = self.tabs.get_mut(tab_idx) else {
            return;
        };
        let Some(pane) = tab.pane_by_id_mut(pane_id) else {
            return;
        };
        inject_into_pane(pane, mode, &nonce);
    }

    /// Take the persistent install back off every live host. Called when
    /// the user leaves that level: what the app wrote to their dotfiles is
    /// the app's to clean up, and leaving it behind would keep reporting
    /// commands from a setting they just turned off.
    pub(crate) fn remove_shell_integration_all(&mut self) {
        let line = uninstall_line();
        for tab in &mut self.tabs {
            for pane in tab.pane_grid.panes.values_mut() {
                if let Some(ssh) = pane.session.as_ref().and_then(|s| s.ssh())
                    && let Err(e) = ssh.write(line.as_bytes())
                {
                    tracing::warn!(
                        target = "oryxis::shell_integration",
                        error = %e,
                        "failed to remove the shell-integration snippet"
                    );
                }
            }
        }
    }

    /// Same for every live pane, so flipping the setting reaches the
    /// sessions the user already has open instead of only future ones.
    pub(crate) fn apply_shell_integration_all(&mut self) {
        let mode = self.setting_shell_integration;
        if mode == ShellIntegrationMode::Off {
            return;
        }
        let nonce = self.shell_integration_nonce();
        for tab in &mut self.tabs {
            for pane in tab.pane_grid.panes.values_mut() {
                inject_into_pane(pane, mode, &nonce);
            }
        }
    }
}

/// Feed the snippet to one pane's shell and arm its sniffer with the nonce.
/// The nonce is set FIRST: a sequence arriving before it would be accepted
/// unverified, and the arming has to hold even if the write fails.
fn inject_into_pane(pane: &mut crate::state::Pane, mode: ShellIntegrationMode, nonce: &str) {
    if pane.shell_integration_injected {
        return;
    }
    let Some(ssh) = pane.session.as_ref().and_then(|s| s.ssh()) else {
        return;
    };
    let mut payload = session_inject(nonce);
    if mode == ShellIntegrationMode::Persistent {
        payload.push_str(&install_line(nonce));
    }
    if let Err(e) = ssh.write(payload.as_bytes()) {
        tracing::warn!(
            target = "oryxis::shell_integration",
            error = %e,
            "failed to inject the shell-integration snippet"
        );
        return;
    }
    pane.shell_integration_injected = true;
    if let Ok(mut term) = pane.terminal.lock() {
        term.set_shell_command_nonce(Some(nonce.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_carries_the_nonce_and_the_tmux_envelope() {
        let s = snippet("abc123");
        assert!(s.contains("633;E;$(__oryxis_esc \"$1\");abc123"));
        // The passthrough envelope, with the inner ESC doubled, is what
        // makes the sequence survive tmux at all.
        assert!(s.contains(r"\033Ptmux;\033\033]%s\007\033\\"));
        assert!(!s.contains("@NONCE@"));
    }

    #[test]
    fn session_inject_is_plain_text_and_self_clearing() {
        let line = session_inject("n0nce");
        // The remote shell reads this through readline in raw mode: a real
        // control byte would be taken as a keypress, not as text. Only
        // printable bytes (plus the two command-terminating newlines) may
        // travel.
        for b in line.bytes() {
            assert!(
                b == b'\n' || (b' '..=b'~').contains(&b),
                "non-printable byte {b:#x} would be read as a keypress"
            );
        }
        assert!(line.starts_with("printf '\\x1b7'\n"), "saves the cursor first");
        assert!(line.ends_with("printf '\\x1b8\\x1b[1A\\x1b[J'\n"), "wipes its own echo");
        // The payload is opaque, so quotes and newlines in the snippet
        // cannot break the line apart.
        assert_eq!(line.matches('\n').count(), 2);
    }

    #[test]
    fn install_script_replaces_its_own_block_and_keeps_permissions() {
        let s = install_script("n0nce");
        // Rewrite in place: `cat >` keeps the rc file's mode and inode,
        // where `mv` would install the temp file's.
        assert!(s.contains(r#"cat "$tmp" > "$rc""#));
        assert!(!s.contains(r#"mv "$tmp""#));
        // The awk filter drops any previous block before the append, so
        // reinstalling can never stack two copies.
        assert!(s.contains(RC_BEGIN) && s.contains(RC_END));
        assert!(s.matches("ORYXIS_SI_EOF").count() == 2);
        // A quoted heredoc: the snippet must land verbatim, not expanded
        // by the shell that writes it.
        assert!(s.contains("<<'ORYXIS_SI_EOF'"));
    }

    #[test]
    fn uninstall_leaves_allow_passthrough_alone() {
        let s = uninstall_script();
        assert!(s.contains(RC_BEGIN));
        assert!(
            !s.contains("allow-passthrough"),
            "other tools depend on it; removing it is not ours to do"
        );
    }

    #[test]
    fn modes_round_trip_through_the_setting_value() {
        for m in ShellIntegrationMode::ALL {
            assert_eq!(ShellIntegrationMode::from_setting(m.as_setting()), m);
        }
        // Anything unknown (an older vault, a hand-edited row) is the
        // level that touches nothing.
        assert_eq!(
            ShellIntegrationMode::from_setting("bogus"),
            ShellIntegrationMode::Off
        );
    }
}
