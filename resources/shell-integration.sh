# Oryxis shell integration: makes the shell report the command line it
# runs (OSC 633;E) so the command history keeps working inside tmux.
# bash and zsh. Loading it twice is a no-op.
#
# The trailing __ORYXIS_NONCE__ is YOUR key, copied from Settings >
# Terminal > Integration, the "Copy shell integration snippet" button.
# Oryxis ignores any reported command line that does not carry it, so a
# file or a log that prints this sequence cannot plant a command in your
# history.
if [ -z "${__oryxis_si:-}" ]; then
  __oryxis_si=1
  __oryxis_key=__ORYXIS_NONCE__
  # Wrap the sequence in tmux's passthrough envelope when inside tmux.
  # Decided at emission time, so starting tmux after login still works.
  __oryxis_osc() {
    if [ -n "$TMUX" ]; then
      printf '\033Ptmux;\033\033]%s\007\033\\' "$1"
    else
      printf '\033]%s\007' "$1"
    fi
  }
  # OSC 633 argument escaping: a raw ';' would end the argument early
  # (think `cd /tmp; ls`) and control characters would break the frame.
  __oryxis_esc() {
    local s=$1
    s=${s//'\'/'\\'}
    s=${s//';'/'\x3b'}
    s=${s//$'\n'/'\x0a'}
    s=${s//$'\r'/'\x0d'}
    s=${s//$'\t'/'\x09'}
    printf '%s' "$s"
  }
  # Report the command line, then "output starts here".
  __oryxis_pre() {
    case "$1" in ' '*) return ;; esac
    __oryxis_osc "633;E;$(__oryxis_esc "$1");$__oryxis_key"
    __oryxis_osc "133;C"
  }
  if [ -n "$ZSH_VERSION" ]; then
    autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook preexec __oryxis_pre
  elif [ -n "$BASH_VERSION" ]; then
    # bash has no preexec hook, so the DEBUG trap stands in for one. It
    # needs the two helpers below; see the notes under the block.
    __oryxis_hline() {
      local h
      h=$(HISTTIMEFORMAT= builtin history 1)
      h=${h#"${h%%[![:space:]]*}"}
      __oryxis_n=${h%%[![:digit:]]*}
      h=${h#"$__oryxis_n"}
      __oryxis_l=${h:2}
    }
    __oryxis_bpre() {
      [ -n "$COMP_LINE" ] && return
      __oryxis_hline
      [ "$__oryxis_n" = "$__oryxis_hn" ] && return
      __oryxis_hn=$__oryxis_n
      __oryxis_pre "$__oryxis_l"
    }
    __oryxis_hline
    __oryxis_hn=$__oryxis_n
    trap '__oryxis_bpre' DEBUG
  fi
fi
