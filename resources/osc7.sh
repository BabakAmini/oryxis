# Report the shell's working directory (OSC 7) so the terminal's file
# browser follows it exactly. bash and zsh. Sourcing it twice is a
# no-op.
if [ -z "${__oryxis_osc7:-}" ]; then
  __oryxis_osc7=1
  # Percent-encode the path BYTE by byte, under LC_ALL=C. This is the
  # part that is easy to get wrong: a raw space would end the URL
  # early, and encoding per CHARACTER instead of per byte turns every
  # accented directory name into mojibake on the other side.
  __oryxis_urlencode() (
    LC_ALL=C
    str=$1
    while [ -n "$str" ]; do
      safe=${str%%[!a-zA-Z0-9/:_\.\-\!\'\(\)~]*}
      printf '%s' "$safe"
      str=${str#"$safe"}
      if [ -n "$str" ]; then
        printf '%%%02X' "'$str"
        str=${str#?}
      fi
    done
  )
  __oryxis_cwd() {
    printf '\033]7;file://%s%s\007' \
      "${HOSTNAME:-$(uname -n)}" "$(__oryxis_urlencode "$PWD")"
  }
  # Registered through each shell's own pre-prompt hook. bash has no
  # `precmd`, zsh has no `PROMPT_COMMAND`, so each branch uses the one
  # its shell actually runs.
  if [ -n "${ZSH_VERSION:-}" ]; then
    autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook precmd __oryxis_cwd
  elif [ -n "${BASH_VERSION:-}" ]; then
    PROMPT_COMMAND="__oryxis_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
  fi
  # Report once at load, so the first prompt is already correct.
  __oryxis_cwd
fi
