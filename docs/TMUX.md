# Logs and command history when you use tmux

Oryxis records your sessions, and it captures the commands you type into
a per-host history. Inside tmux, one of those two keeps working and the
other needs your help. This page explains why, and what to do about it.

Everything here is optional and every step is yours to take: **Oryxis
never edits files on your servers.**

## TL;DR

| What you want | Inside tmux | What to do |
|---|---|---|
| A recording of everything on screen | Works already | Turn on Session logging (Settings > Security & Privacy > Logging & history) |
| A plain-text transcript or an asciicast replay | Works already | Export it from the History screen |
| The list of commands you typed (History sidebar) | Does not work | [Install the shell-integration snippet](#make-command-history-work-inside-tmux) yourself |
| A log file on the server itself | Not an Oryxis feature | [Use tmux's own `pipe-pane`](#let-tmux-write-its-own-log) |

## Why tmux hides your commands

Oryxis is the terminal at the end of the connection. When you run tmux,
tmux takes over the screen: it switches to the alternate buffer and
repaints every pane itself. From the outside, all Oryxis sees is that
repaint stream.

That is fine for a recording (the bytes are the bytes), but it breaks
command capture, which normally reads the command back off the grid at
the prompt position:

- On the alternate screen there is no reliable way to tell a command
  typed at a shell prompt from keystrokes going into vim, less or htop.
  Recording the latter would fill your history with junk, so capture
  from the screen is deliberately off there.
- With a vertical split, a single grid row holds two panes side by side.
  Reading a "line" would splice your neighbour pane's text into the
  command.

This is not specific to Oryxis: iTerm2 documents that its shell
integration does not work under plain tmux either, and WindTerm turns
the same features off on the alternate screen. The clients that do get
inner-tmux commands read them from the shell, never from the screen,
which is exactly what the next section sets up.

## What already works: session recording

Session recording captures the raw output stream, tmux included. Nothing
to configure beyond turning it on:

1. **Settings > Security & Privacy > Logging & history > Session logging** records SSH session output into the vault (encrypted). A per-host override exists on the host editor. If the row is hard to find, type "session logging" into the Settings search box and it takes you straight there.
2. **Detailed recording (replay)**, the next row down, additionally stores timing and resizes, which is what makes the asciicast export and the in-app player possible. It only appears once Session logging is on.
3. The **History** screen lists your recordings. Each one can be replayed in the app, exported as an asciicast `.cast` file, or exported as a plain-text transcript.

So if what you need is "a log of what happened in this session", you
already have it, tmux or not.

One thing to know about **reading** a tmux recording in the app: the
transcript viewer has two modes, and the header switches between them.
*Rendered screen* replays the recording faithfully, which is what you
want for an ordinary shell session. But tmux repaints a single screen
that has no scrollback, so a recording spent inside tmux replays into
one final frame with nothing to scroll: those open in *Linear dump*
instead, where every repaint is appended in order, the way the
plain-text transcript export reads. Recordings that spent more than
half their time on the alternate screen pick Linear by themselves
(recordings made before timing was stored fall back to "was it still on
the alternate screen at the end", so an old session that ended in a
pager opens Linear too).

## Make command history work inside tmux

The per-host command history (the History tab in the terminal sidebar,
and the optional plain-text command log) is built from what the shell
reports about itself, when the shell reports anything. Oryxis
understands the two standard sequences:

- **OSC 133** prompt marks (`A`/`B`/`C`/`D`): where the prompt starts
  and ends, when a command starts running, and its exit status.
- **OSC 633 ; E** (VS Code's superset): the command line *as the shell
  parsed it*. This is the one that survives tmux, because the text never
  has to be read back off a screen tmux owns.

Prompt marks are read from any shell that emits them, with no setup.
The reported command line is not: it has to carry **your shell
integration key**, and Oryxis ignores every `E` that does not.

That gate exists because a captured command is one click from running
again in the History tab, and nothing in a byte stream says who wrote
it. Without the key, any file you `cat`, any log line, any host you
connect to could put a command in your history that you never typed and
might later click. So the key is a shared secret between the app and
your own dotfile, and a stock VS Code integration (which knows nothing
about it) reports commands Oryxis will not record.

Your key lives in **Settings > Terminal > Integration**, under the "Capture command history" toggle: the **Copy shell integration snippet** button there copies the snippet below with the key already in it, which is the path that cannot go wrong. The block is reproduced here so you can read what you are about to paste into your shell.

### 1. Save the snippet on the host

Save this as `~/.config/oryxis/shell-integration.sh` (any path works),
replacing `__ORYXIS_NONCE__` with your key if you copied it from here
rather than from the app:

```sh
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
```

Yes, the bash half looks like a lot for "print the command". It is not
padding, it is bash: there is no `preexec` hook, and the obvious
one-liner is wrong in three ways you would only notice later.
`trap 'report "$BASH_COMMAND"' DEBUG` fires once per COMMAND, so
`ls /tmp | head -3` reports two commands and `cd /; pwd` reports two
more; and `$BASH_COMMAND` has already lost the leading space, so
` secret-command` (the `HISTCONTROL=ignorespace` convention) lands in
your history anyway. Reading the line back from `history 1`, gated on
the history NUMBER changing, gives one record per line you typed and
keeps the leading space, which is what those twelve lines buy. zsh has
`preexec` and needs one line.

Two things to know about the bash side:

- A command repeated back to back is reported once if your
  `HISTCONTROL` includes `ignoredups` (bash never assigns it a new
  history number, so the snippet cannot see it).
- Commands your prompt itself runs are never reported, for the same
  reason: they are not in the history.

### Optional: prompt marks for durations and exit codes

The block above is the minimum for command history. Oryxis also
understands the OSC 133 prompt cycle, which is what gives smart tabs
the real command duration and its exit status instead of a quiet-period
guess. If you want that too, add this inside the `if` block, right
before its closing `fi`:

```sh
  __oryxis_post() {
    local st=$?
    [ -n "$__oryxis_ran" ] && __oryxis_osc "133;D;$st" && __oryxis_ran=
  }
  if [ -n "$ZSH_VERSION" ]; then
    setopt prompt_subst 2>/dev/null
    autoload -Uz add-zsh-hook 2>/dev/null
    add-zsh-hook precmd __oryxis_post
    PS1='%{$(__oryxis_osc "133;A")%}'$PS1'%{$(__oryxis_osc "133;B")%}'
  elif [ -n "$BASH_VERSION" ]; then
    PROMPT_COMMAND="__oryxis_post${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
    PS1='\[$(__oryxis_osc "133;A")\]'"$PS1"'\[$(__oryxis_osc "133;B")\]'
  fi
```

and add `__oryxis_ran=1` as the first line of `__oryxis_pre`. A prompt
rebuilt by a framework (starship, powerlevel10k) may drop the `PS1`
wrappers; the command history does not care, since it reads the
reported line, not the screen.

### 2. Source it from your shell rc

Add this to the end of `~/.bashrc` or `~/.zshrc`:

```sh
[ -f ~/.config/oryxis/shell-integration.sh ] && . ~/.config/oryxis/shell-integration.sh
```

The shells tmux starts read your rc file, which is why this step is what
makes the difference inside tmux.

**bash users, read this one:** tmux starts your shell as a *login*
shell, and a login bash reads `~/.bash_profile` (or `~/.bash_login`, or
`~/.profile`) instead of `~/.bashrc`. Most distributions ship a
`~/.bash_profile` that sources `~/.bashrc` for you, but if yours does
not, the snippet will work over plain SSH and do nothing inside tmux.
Either add the usual line to `~/.bash_profile`:

```sh
[ -f ~/.bashrc ] && . ~/.bashrc
```

or tell tmux to start non-login shells in `~/.tmux.conf`:

```tmux
set -g default-command "${SHELL}"
```

zsh has no such split: `~/.zshrc` is read by every interactive shell,
login or not.

### 3. Let tmux pass the sequences through

tmux drops unknown escape sequences unless passthrough is on. On
**tmux 3.3 or newer**, add to `~/.tmux.conf`:

```tmux
set -g allow-passthrough on
```

then reload (`tmux source-file ~/.tmux.conf`) or apply it to the running
server with `tmux set -g allow-passthrough on`.

The option was added in **tmux 3.3**. On older versions the line is a
parse error (`invalid option: allow-passthrough` at startup), and there
is no way to get the sequences out of tmux there. Check with `tmux -V`
first: Ubuntu 22.04 and AlmaLinux/RHEL 9 ship 3.2a, so this affects a
lot of otherwise current servers.

Passthrough really is the switch: with it off, tmux swallows the
sequences and the History tab stays empty; with it on, the same shell in
the same pane reports every command.

### 4. Check it

Open a new shell (or `source` your rc), run a couple of commands inside
tmux, and look at the History tab in the terminal sidebar. Commands
typed in any tmux pane should now be listed.

## Let tmux write its own log

If what you want is a log file **on the server**, tmux does that by
itself, no Oryxis involved:

```tmux
# ~/.tmux.conf: toggle logging of the current pane with prefix + H
bind H pipe-pane -o 'cat >> ~/tmux-#S-#W-#P.log' \; display 'Logging to ~/tmux-#S-#W-#P.log'
```

`pipe-pane -o` toggles: the same key stops it. The file holds the raw
pane output, escape sequences included; pipe it through `sed` or
`ansi2txt` if you want plain text. For a fuller version (per-session
files, automatic capture of scrollback, log rotation) the
[tmux-logging](https://github.com/tmux-plugins/tmux-logging) plugin is
the usual answer.

## What Oryxis will not do

Earlier nightlies had an "Install on the host" option that wrote the
snippet into your `~/.bashrc` and appended `allow-passthrough` to your
`~/.tmux.conf`. It is gone, and it is not coming back: an SSH client has
no business editing the files on your servers. Your dotfiles are yours,
which is why this page tells you what to write instead of writing it for
you.

If a nightly build did install that block for you, remove it by deleting
the lines between `# >>> oryxis shell integration >>>` and
`# <<< oryxis shell integration <<<` in your rc file, and the
`set -g allow-passthrough on` line from `~/.tmux.conf` if you do not
want it (it is harmless and useful on tmux 3.3+, but on older tmux it is
the line that prints the startup error).
