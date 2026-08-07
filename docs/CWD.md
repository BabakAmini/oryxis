# Making the Files sidebar follow your directory exactly

Every SSH tab has a Files sidebar that follows the directory your shell
is in, so `cd` in the terminal moves the file browser with you. It
follows one of two ways, and only one of them is exact. This page
explains the difference and how to get the exact one.

Everything here is optional and every step is yours to take: **Oryxis
never edits files on your servers.**

## TL;DR

| What you want | Works already | What to do |
|---|---|---|
| The sidebar roughly tracks your directory | Yes, if your prompt puts the path in the window title | Nothing |
| Exact tracking, any prompt, paths with spaces or accents | No | [Install the OSC 7 snippet](#install-the-snippet) |
| Same, inside tmux | No | The snippet, plus [passthrough](#4-inside-tmux-let-the-sequence-out) |

## Why the title is a guess

When the shell says nothing about where it is, Oryxis falls back to
reading the **window title**. That works because the default bash and
zsh prompts on most distributions set the title to `user@host: ~/path`,
so the path is sitting right there.

It is a heuristic, and it fails the moment your prompt stops matching
that shape:

- A prompt framework (starship, powerlevel10k, oh-my-zsh themes) that
  writes its own title, or none at all.
- A title that abbreviates: `~` instead of your real home path, or a
  truncated `.../deep/path`.
- A directory whose name contains `: ` or looks like a hostname.
- Anything that sets the title itself while running (`ssh` from inside
  the session, `vim`, a build tool).

None of that is fixable by parsing harder. The shell is the only thing
that actually knows its directory, so the answer is to have it say so.

## What OSC 7 is

`OSC 7` is the standard escape sequence a shell uses to report its
working directory:

```
ESC ] 7 ; file://<host>/<percent-encoded path> BEL
```

It originated in macOS Terminal and is understood today by iTerm2,
kitty, WezTerm, VS Code, GNOME Terminal and everything else built on
VTE. Oryxis accepts both terminators (`BEL` and `ESC \`), accepts a
missing host (`file:///path`), and percent-decodes the path.

Installing it is worth doing for reasons beyond Oryxis: the same
snippet makes every one of those terminals open new tabs in the right
directory.

## Install the snippet

### 1. Save it on the host

Save this as `~/.config/oryxis/osc7.sh` (any path works):

```sh
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
```

The `PROMPT_COMMAND` line **prepends**, keeping whatever your prompt
already set, so a framework that uses it keeps working.

### 2. Source it from your shell rc

Add this to the end of `~/.bashrc` or `~/.zshrc`:

```sh
[ -f ~/.config/oryxis/osc7.sh ] && . ~/.config/oryxis/osc7.sh
```

**bash users, read this one:** a *login* bash reads `~/.bash_profile`
(or `~/.bash_login`, or `~/.profile`) instead of `~/.bashrc`. Most
distributions ship a `~/.bash_profile` that sources `~/.bashrc` for
you; if yours does not, add the usual line to it:

```sh
[ -f ~/.bashrc ] && . ~/.bashrc
```

zsh has no such split: `~/.zshrc` is read by every interactive shell,
login or not.

### 3. Check it

Open a new shell (or source your rc), open the Files tab in the
terminal sidebar, and `cd` somewhere with a space in the name. The
sidebar should land exactly there. If it follows your `cd` but lands on
the wrong path for names with spaces or accents, the snippet is loaded
but the encoding half is not; re-copy the `__oryxis_urlencode` function.

### 4. Inside tmux, let the sequence out

tmux drops unknown escape sequences unless passthrough is on. On
**tmux 3.3 or newer**, add to `~/.tmux.conf`:

```tmux
set -g allow-passthrough on
```

then reload (`tmux source-file ~/.tmux.conf`) or apply it to the
running server with `tmux set -g allow-passthrough on`.

The option was added in **tmux 3.3**. On older versions the line is a
parse error at startup, and there is no way to get the sequence out of
tmux there. Check with `tmux -V` first: Ubuntu 22.04 and
AlmaLinux/RHEL 9 ship 3.2a, so this affects a lot of otherwise current
servers.

If you also want your typed commands to show up in the History tab
inside tmux, that needs its own snippet, and it is covered in
[the tmux guide](TMUX.md).

## What Oryxis will not do

Up to v0.12 there was a Settings toggle, "Force exact directory
following (OSC 7)", that typed a setup line into your shell on connect
and tried to erase its own echo afterwards. It is gone.

It was removed because the technique cannot be made safe. The app
writes those bytes the moment the SSH session is established, but it
has no way to know what the shell is doing at that instant: on a host
with a long MOTD or a slow `/etc/profile.d`, the bytes land before the
shell reaches its first prompt, the terminal echoes them raw, and the
self-erasing trailer wipes the wrong region because the cursor
position it was calibrated against never existed. On one field report
it left the setup block on screen and hung the session hard enough to
need a reconnect and a Ctrl+C.

No other terminal does this, which in hindsight was the tell. Every
client that installs shell integration on a remote host replaces the
**command the SSH channel runs** rather than typing into the running
shell: kitty's `ssh` kitten ships a bootstrap script and `exec`s your
login shell after it, VS Code injects through `--init-file` / `ZDOTDIR`
for the shells it launches itself and
[documents that this does not work over plain SSH](https://code.visualstudio.com/docs/terminal/shell-integration),
and WezTerm and iTerm2 hand you a snippet exactly like the one above.

That leaves your dotfiles as the right place for this, which is where
this page puts it.

Nothing to clean up on your servers: the old toggle never wrote to a
file, it only set `PROMPT_COMMAND` inside the running shell, so the
emitter died with that shell on disconnect. The one exception is a
shell that outlives the connection, inside tmux or screen: there it
keeps emitting until that shell exits, and `unset -f __oryxis_o7` plus
removing `__oryxis_o7;` from the front of `PROMPT_COMMAND` clears it
early if you want it gone before then.
