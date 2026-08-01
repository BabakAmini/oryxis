# Feature tour

The complete inventory of what Oryxis does today. For the short version,
see the [Highlights](../README.md#highlights) in the README; for what's
coming next, see the [Roadmap](../README.md#roadmap).

## SSH & connectivity

- **Auto-authentication.** Tries key, agent, password, and
  keyboard-interactive in order. A "Password prompt" method asks at every
  connection and never writes anything to the vault.
- **Full SSH pipeline.** Direct, SOCKS4/5, HTTP CONNECT, ProxyCommand,
  multi-hop jump host chaining, and port forwarding via
  [russh](https://github.com/warp-tech/russh).
- **Standalone port forwarding.** Local (`-L`), Remote (`-R`) and Dynamic
  SOCKS5 (`-D`) forwards live as their own entities with per-row on/off
  toggles, auto-start at boot, and no terminal required.
- **Authenticated proxies.** SOCKS5 and HTTP CONNECT Basic auth, with proxy
  passwords in their own encrypted column.
- **Proxy + jump host stacking.** A jump host behind a proxy dials through
  it on the first hop.
- **Reusable Proxy Identities.** Save SOCKS5 / HTTP / SOCKS4 configs once
  and link them from any host.
- **SSH agent forwarding.** Per-host opt-in; bridges the local ssh-agent
  socket through the channel.
- **X11 forwarding.** Per-host opt-in (and imported from `ForwardX11` in
  `~/.ssh/config`) so remote GUI apps draw on your local display. The
  remote never learns your real cookie: a fake `MIT-MAGIC-COOKIE-1` is
  minted per session, verified in constant time on every X11 channel and
  swapped for the real one before any byte reaches the X server.
  Resolves `$DISPLAY` across Linux/BSD unix sockets, XQuartz launchd
  paths on macOS, and the TCP endpoint VcXsrv / Xming serve on Windows,
  including displays that run with access control off.
- **Rich `~/.ssh/config` import.** `ProxyCommand` and `ProxyJump` resolved
  automatically.
- **PuTTY-grade details.** TCP_NODELAY on every socket, per-host IPv4/IPv6
  preference honored on direct dials, proxy dials and jump chains, and the
  SSH pre-authentication banner shown instead of silently dropped.
- **RSA SHA-2 support** (`rsa-sha2-512/256` with SHA-1 fallback),
  step-by-step connection progress, TOFU host key verification, and
  integration tests against real OpenSSH containers.

## Telnet, serial & ZMODEM

- **Per-host protocol selector.** A host can be SSH, Telnet or serial; the
  editor swaps to a reduced form per protocol.
- **Native Telnet engine.** RFC 854/855 option negotiation with the
  loop-proof RFC 1143 state machine, NAWS window sizing, terminal-type,
  charset transcoding, and prompt-driven credential autofill. The editor
  carries an honest note that the protocol is cleartext by design.
- **Serial consoles.** Local COM / `/dev/tty*` lines with configurable
  baud, framing, flow control, line endings and local echo.
- **ZMODEM transfers.** Run `sz` / `rz` on the remote and Oryxis
  auto-detects the transfer, takes over the byte stream and moves the file
  with a progress overlay. Works over SSH, Telnet and serial alike; a
  disconnect mid-transfer resumes the terminal cleanly.

## Quick connect

- **`user@host`, no ceremony.** Type it into the new-tab picker (Ctrl+K),
  the toolbar search or the tab jump and connect without saving a host.
- **Auth switch on failure.** If the first attempt fails, the prompt offers
  every saved identity and key and reconnects in place.
- **Edit mid-connect.** "Edit host" is available in every connection state,
  editing the temporary host without writing to the vault.
- Quick connections behave like real tabs (splits, SFTP, port forwards);
  typed credentials are swept on vault lock.

## Remote desktop (RDP / VNC)

- **One click to a desktop.** An RDP or VNC host is a first-class card that
  launches the OS-native client (`mstsc`, Microsoft Remote Desktop,
  FreeRDP, Remmina, or your VNC viewer).
- **Through an SSH tunnel.** The card can reach the machine directly or
  tunnel through an SSH host you pick as a gateway, an ephemeral `-L`
  forward provisioned on demand, managed, and closed once the client
  disconnects.
- Opt-in and hidden until enabled in Settings.

## Terminal

- **Embedded emulator.**
  [alacritty_terminal](https://github.com/alacritty/alacritty) with
  256-color, truecolor, mouse selection, scrollback.
- **Split panes.** Split a tab into a tmux/iTerm-style grid; each pane is
  its own session (saved host or local shell), with keyboard / paste /
  snippets / AI targeting the focused pane.
- **Session groups.** Save a split arrangement (panes + split tree +
  per-pane startup scripts) as a reusable, credential-free entity.
- **Smart tabs.** A command that ran past a threshold and finished while
  you were looking elsewhere earns an attention dot (green success, red
  nonzero exit) plus a notification with the command and duration. Hosts
  without shell integration get a quiet-period heuristic.
- **Per-host command history.** Captured via shell-integration marks
  (OSC 133) with a raw-input fallback, encrypted at rest, redacted before
  storage, surfaced in a sidebar History tab: most-frequent shortlist,
  recent list, search, re-run, paste-without-execute, delete. Local-only
  by design (never synced, never exported); optional plain-text export and
  per-host live-append log. Inside tmux the screen belongs to tmux, so
  capture needs the shell's own report: see the
  [tmux guide](TMUX.md).
- **Session recording.** The encrypted session logs capture timing and
  terminal resizes; any session exports as an asciicast v3 `.cast` file
  with your terminal theme embedded, or as a plain-text transcript.
  Output-only by design, so keystrokes never leak into a recording.
- **Search inside recordings.** The History screen searches the session
  content itself, not just titles, decrypting on demand with a bounded
  scan, and can filter to the hosts a given command ever ran on.
- **Pinned & reorderable tabs.** Pin tabs (restored on next launch, lazy
  reconnect), drag to reorder, rename, MRU switching with Ctrl+Tab, and an
  optional bottom tab bar.
- **Syntax highlighting.** IPs, URLs, and file paths auto-detected and
  colored.
- **17 terminal palettes plus custom schemes.** Picker with inline swatch
  previews, global or per-host; build your own, clone a built-in as a
  starting point, or import iTerm / Windows Terminal / base16 from a
  pasted blob or a file. Terminal and UI themes both export back out, so
  a scheme moves between machines without the vault.
- **Bundled Nerd Fonts.** SauceCodePro plus a Symbols Nerd Font fallback so
  Powerline and icon glyphs always render.
- **Paste done right.** X11-style middle-click paste, configurable
  right-click (paste / context menu / xterm-style extend selection), CRLF
  normalization, and a paste guard (see Security below).
- **International keyboards done right.** AltGr-composed characters (the
  bepo `_`, the German `@`, `{`, `[`) reach the shell as text on every
  platform instead of being eaten as control chords, and Ctrl+Space sends
  NUL. Ctrl+Shift with a cursor, editing or function key sends the xterm
  modified sequence (`ESC[1;6D` for Ctrl+Shift+Left) like a real terminal,
  so modifier-aware TUIs see the combo. On macOS the Option key composes
  characters like every native terminal, with a per-host "Option as Meta"
  override (off / left / right / both) for readline and emacs users.
- **System mono font enumeration**, configurable font size (10-24px,
  `Ctrl + = / - / 0`), bold-to-bright colors, scrollback reset on keypress
  (on by default: typing jumps back to the live edge, so a scrolled-up
  viewport never hides what you type) and/or on output, and an opt-in
  performance HUD (frame time vs budget, RTT and jitter on the SSH
  connection).

## Host monitoring

- **Agentless vitals.** A sidebar Monitor tab reads CPU, memory, swap,
  disk and network off the SSH connection you already have, on an exec
  channel multiplexed onto the live session. Nothing is installed on the
  server; Linux `/proc` is the primary source with BSD and macOS probe
  fallbacks.
- **Opt-in, twice over.** The whole feature hides behind a Features &
  Plugins toggle, and then per host, so no connection starts probing
  behind your back. "Enable for all hosts" flips it wholesale, and the
  probe interval is configurable.
- **Listening ports with click-to-forward.** The panel lists what the
  host is listening on and turns any row into a local port forward in
  one click, honoring the listener's own bind address.
- **Kill the process on a port.** Right-click a port row for a graceful
  stop or a forced kill, behind a confirmation naming the port, process,
  PID and signal. The signal goes out on an exec channel, never into
  your shell, and Oryxis re-asks the host who owns the port first, so a
  service that restarted while the dialog was open is reported instead
  of signalled by mistake. Sockets whose PID the login user cannot see
  (the usual case for root-owned services) escalate through sudo, using
  the host's stored password only if sudo actually asks for one.
- **Threshold alerts** when a gauge crosses a line you set, a collapsible
  disk list for hosts with many mounts, and an optional status-bar
  segment that keeps the headline numbers visible with the panel closed.
- **Untrusted by construction.** Every number crossing the wire comes
  from a machine you may not control, so all probe arithmetic saturates
  rather than wrapping, and truncated or forged payloads degrade to
  "unknown" instead of nonsense.

## SFTP & files

- **Dual-pane layout.** Local and remote side by side, with sortable
  columns.
- **Open / Edit, in the background.** Hands a remote file to your OS
  default application, the editor you configured, or the OS "open with"
  picker, then watches the local copy while you keep browsing: nothing
  blocks, and each save you make asks whether to send the file back (yes,
  yes to all, autosave, skip, or stop editing). Reopening a file that is
  still being edited offers the local copy instead of silently
  re-downloading over it. A path-history dropdown jumps back to
  directories you have already visited.
- **Per-host start folder.** A host can remember where its SFTP mounts
  open, set from the host editor or from any remote folder's context
  menu. A path that stops resolving falls back to the login directory
  instead of failing the mount.
- **Files in every SSH tab.** A sidebar Files tab browses over the tab's
  existing connection and follows your shell's working directory as you
  `cd` (shell-integration cwd reporting with a window-title fallback;
  manual navigation unpins, one click follows again).
- **Hybrid Files mode.** "Open SFTP session" flips the whole tab into the
  dual-pane manager at the directory you were in, while the PTY keeps
  running underneath; a chip on the tab (or Ctrl+Shift+F) flips back, and
  the surface can detach into its own tab.
- **Drag-and-drop uploads.** Drop files from any OS file manager onto the
  remote pane; drag rows between panes to upload or download.
- **Multi-select.** Ctrl/Cmd-click and Shift-range; batch Delete /
  Download / Duplicate / Upload.
- **Properties dialog.** Per-row chmod grid, size, mtime, owner.
- **Server-to-server copy.** Transfer files directly between two remote
  hosts, streamed host-to-host with no local round-trip and a live
  byte-level progress bar.
- **Overwrite handling**, configurable parallelism (1-8 channels),
  recursive delete over exec, and tunable timeouts.

## AI chat assistant

- **Integrated AI sidebar.** Collapsible chat panel per terminal session;
  replies route to the tab that asked.
- **Streaming responses.** Tokens land as the model emits them; markdown
  re-renders progressively.
- **Runs commands for you.** The assistant drives the focused pane through
  an `execute_command` tool and reads the output back, instead of printing
  commands to copy.
- **Plan / Ask / Auto modes** with a floating Stop control.
- **Three-layer auto-exec safety.** A deterministic floor force-prompts
  catastrophic commands, an independent fail-safe LLM judge vets the rest,
  and the "always run" allow-list refuses chained / piped / substituted
  commands so a trusted name can't smuggle a destructive payload.
- **Multiple providers.** Anthropic, OpenAI, Google Gemini, or any
  OpenAI-compatible endpoint; bring your own key, stored encrypted.
- **Privacy-aware.** Privacy Mode redaction is applied before terminal
  context reaches the model. Terminal context, smart output capture, and a
  custom system prompt option.

## MCP server

- **AI integration.** Expose your SSH hosts to AI assistants via the
  [Model Context Protocol](https://modelcontextprotocol.io/).
- **5 tools.** `list_hosts`, `get_host`, `ssh_execute`, `list_groups`,
  `list_keys`.
- **Per-host control.** Toggle MCP exposure per connection.
- **Disabled by default.** Enable in Settings > Security.
- **Distributed as a plugin.** Downloaded on demand, with a stable launcher
  path for external clients.

Setup for Claude Code (`~/.claude.json`):

```json
{
  "mcpServers": {
    "oryxis": {
      "command": "oryxis-mcp",
      "env": {
        "ORYXIS_VAULT_PASSWORD": "your-vault-password"
      }
    }
  }
}
```

If your vault has no password, omit the `env` field.

## Cloud accounts (AWS, GCP, Azure, Kubernetes)

- **AWS.** Encrypted profiles (named profile, static keys, or IAM Identity
  Center / SSO) with a "Test credentials" button; EC2 and ECS discovery
  grouped by region and cluster; EC2 Instance Connect one-shot key push
  with AMI-aware OS user inference; SSM Session for private instances with
  no open ports; ECS Exec into live containers.
- **Google Cloud.** Compute Engine discovery and GKE clusters via the
  `gcloud` CLI you already authenticate with; adding a GKE cluster wires it
  up as a Kubernetes account (runs `get-credentials` for you).
- **Azure.** VM discovery and AKS clusters via the `az` CLI; AKS clusters
  become Kubernetes accounts the same way.
- **Kubernetes.** Kubeconfig auth (path + context), discovers Deployments /
  StatefulSets / DaemonSets across namespaces, imports them as dynamic
  groups that resolve to live pods, and opens an interactive shell via
  `kubectl exec -it`. A thin CLI wrapper, no heavy SDK.
- **Dynamic groups.** Imports nest under a folder named after the profile;
  services / workloads become groups that resolve live on expand, with
  multi-container Lens and Copy CLI per row.
- **On-demand plugins.** Every provider ships as an Ed25519-signed
  subprocess plugin downloaded on first use, so the core binary stays
  small. Discovery is best-effort per service: an API you never enabled
  doesn't sink the rest.

## Identity system

- **Reusable credentials.** Identities (username + password + key) linked
  to many hosts.
- **Autocomplete.** Type a username to find and link matching identities.
- **Keychain view.** Keys and Identities side by side with search and
  context menus.
- **Proxy Identities.** Same shape for proxy configs, password stored
  encrypted.
- **Encrypted SSH key import.** Passphrase-protected keys are decrypted on
  import; the vault master password protects them at rest.

## Vault & security

- **No password by default.** Opens instantly; enable a master password in
  Settings.
- **Argon2id + ChaCha20-Poly1305** with a per-field salt and nonce;
  re-encryption of all secrets when the password changes; vault reset from
  the lock screen.
- **Biometric unlock.** Windows Hello, Touch ID, or the Linux system
  keyring; opt-in, with the password always one click away.
- **Idle auto-lock.** A timer zeroizes the master key and shows the lock
  screen while live SSH sessions and tabs survive and greet you after
  unlock; secret-bearing UI is swept on lock.
- **TOTP 2FA autofill.** Store a per-host TOTP secret (bare base32 or an
  `otpauth://` URI), encrypted like every credential;
  keyboard-interactive verification-code prompts are answered
  automatically, once per auth attempt, with a manual fallback.
- **Privacy Mode.** Masks IPv4/IPv6 addresses, `user@host` pairs, home
  directories and vault hostnames on screen, in notifications, and before
  AI context leaves the app; click a mask to pin-reveal it.
- **Paste guard.** Multi-line pastes and single-line pastes with invisible
  or bidirectional characters, raw control sequences, `curl | sh`
  fetch-and-execute patterns, or mixed-alphabet look-alikes hit a
  confirmation with one plain warning line per finding.
- **No telemetry.** No data leaves your machine.

See [SECURITY.md](../SECURITY.md) for the full security model and the
vulnerability disclosure policy.

## Snippets

- **Snippets with variables.** `{name}` and `{name:default}` placeholders
  prompt in a small dialog before the send; shell text like `${VAR}` and
  `{print $1}` is deliberately left alone.
- **Groups, tags and shortcuts.** Grouped folder cards in the vault
  (nestable to any depth, see below), a tag filter on hosts and snippets,
  a sidebar toggle that surfaces only snippets tagged like the focused
  host, and a recordable key combo that runs a snippet straight into the
  focused terminal.
- **Run or paste.** Every snippet can run (with Enter) or paste (without)
  into the focused pane, from the vault or the terminal sidebar.

## Themes & internationalization

- **13 global themes plus custom UI schemes.** Switch the entire UI
  instantly, or build your own (21 colors) with a built-in graphical color
  picker and live preview.
- **Per-theme button colors** with WCAG contrast guards enforced in CI.
- **23 languages.** English, Português, Español, Français, Deutsch,
  Italiano, 简体中文, 繁體中文, 日本語, Русский, فارسی, العربية, עברית,
  한국어, Polski, Türkçe, Bahasa Indonesia, Tiếng Việt, Українська, ไทย,
  हिन्दी, Čeština, Ελληνικά.
- **RTL layout support.** Persian, Arabic and Hebrew flip the chrome;
  `Settings → Theme → Layout direction` overrides with Auto / LTR / RTL.
- **Theme + language on the lock screen**, plus floating overlay context
  menus.

## Export / import

- **Single encrypted file.** Export your whole vault as a
  password-protected `.oryxis` file.
- **Selective export.** Include SSH private keys or only host configs.
- **Smart merge.** Import merges by UUID, keeping the newer record (LWW).
- **Round-trips proxy data** so a fresh device gets working proxy auth.

## P2P sync

- **Decentralized.** Sync vault data between devices over QUIC, no cloud
  dependency and no account.
- **LAN discovery.** Automatic peer discovery via mDNS with one-click pair.
- **Cross-network discovery.** Self-hostable signaling (Cloudflare Worker
  or `oryxis-relay`) plus STUN for NAT traversal. See
  [SELF_HOSTING.md](../SELF_HOSTING.md).
- **Pairing.** 6-digit code then Ed25519 challenge/response;
  `oryxis://pair/...` link and QR code.
- **E2E encrypted.** Payloads sealed with X25519 + XChaCha20-Poly1305
  (192-bit nonces).
- **Tombstone-driven deletes** with a 30-day TTL gated by active-peer
  catch-up.
- **Audit hardening.** Signed register/unregister with TOFU pinning, replay
  rejection, bounded session maps, and `verify_strict` across client and
  server.
- **Optional relay** (ciphertext-only) and **opt-in password sync**, off by
  default.

## Plugin subsystem

- **Out-of-process plugins.** Cloud providers and the MCP server run as
  subprocess binaries over JSON-RPC stdio.
- **Signed binaries.** Every plugin is Ed25519-signed and verified against
  a baked-in key before execution.
- **Manifest + cache.** The right asset for the host arch is downloaded on
  demand and verified (signature + sha256).
- **Install errors translated** across all 23 languages.

## OS integration

- **Windows system tray.** Show / Hide to tray / Quit with dynamic
  submenus, an active-sessions submenu, a recent-hosts submenu, opt-in
  close-to-tray and minimize-to-tray, and a single-instance guard with
  primary/child IPC.
- **Windows JumpList.** Recent hosts on the taskbar icon; right-click,
  pick, connected.
- **Window memory.** Size, position, monitor, maximized and fullscreen
  state restored across launches.
- **Truthful rendering.** A boot probe detects software rasterizers
  masquerading as GPU backends (the WSL / llvmpipe class) and drops to the
  built-in software renderer, with honest backend labels and an in-app
  restart.

## UI / UX

- **Native GPU-accelerated UI.** [Iced](https://iced.rs) on the wgpu
  backend.
- **Keyboard everything.** Focus zones across the vault, keyboard walks
  through modals, menus, Settings and the terminal sidebar, Ctrl+Shift+1..8
  section jumps, MRU tab switching, and creation chords. Every binding is
  rebindable with a live capture mode.
- **Workspace layout mode.** Sidebar hides when a tab is open so the
  terminal fills the canvas; Classic mode stays a one-click opt-out.
- **Nested host folders.** Groups hold groups, to any depth. Pickers
  show the full breadcrumb path so two folders named `prod` under
  different parents stay distinguishable, typing a path creates the whole
  chain at once, and deleting a folder promotes its children to the
  grandparent instead of orphaning them.
- **Customizable host icons.** Circular / Square / Outline / Initials,
  global or per-host, with dynamic accent on the chrome from the host's
  color.
- **Responsive card grid.** Column count reflows to the available width;
  long labels truncate cleanly.
- **Multi-tab sessions** with tab overflow, a scrollable strip, and a
  jump-to modal.
- **Tab strip on any edge.** Dock the tabs top, bottom, left or right;
  the side docks turn the strip vertical, can absorb the window chrome
  (burger, Home, window buttons) to reclaim the top bar entirely, and run
  full height. Inactive tabs take a separation style of your choice
  (none, border or underline).
- **Terminal sidebar, where you want it.** Dock it left or right, pick
  which tab it opens on, and have it open itself on connect, globally or
  per host.
- **Configurable status bar.** Every segment is individually
  toggleable, including latency, transfer size, the working directory
  and the host vitals.
- **Settings search.** Type in the Settings sidebar and matching rows
  highlight in place with a hit-count badge per section; Enter and
  Shift+Enter step through matches. English terms work in any UI
  language.
- **Debug affordances.** Settings > Advanced has an opt-in debug log and a
  "Copy environment info" button for bug reports.

## Default keyboard shortcuts

Every binding is rebindable in Settings; this is the out-of-the-box set
for the most common actions. The in-app burger menu shows the full,
current list.

Chords that a terminal application could legitimately want (a bare
`Ctrl+K`, `Ctrl+L` or `Ctrl+J`) are deliberately left to the shell, so
the app's own actions sit on `Ctrl+Shift`.

| Shortcut | Action |
|----------|--------|
| `Ctrl+Shift+T` | New-tab picker |
| `Ctrl+Shift+G` | Quick connect |
| `Ctrl+Shift+J` | Jump to tab |
| `Alt+Left` / `Alt+Right` | Cycle tabs |
| `Ctrl+1...9` | Switch to tab 1-9 |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste in the terminal |
| `Ctrl+Shift+W` | Close tab |
| `Ctrl+Shift+R` | Reconnect the active tab |
| `Ctrl+Shift+F` | Toggle Files mode on an SSH tab |
| `Ctrl+Shift+H` | Focus the terminal sidebar |
| `Ctrl+Shift+P` | Command palette |
| `Ctrl+Shift+L` | Open local terminal |
| `Ctrl+N` | New host |
| `Ctrl+F` | Search the current view / terminal scrollback |
| `Ctrl+,` | Settings |
| `Ctrl+= / Ctrl+- / Ctrl+0` | Terminal font size (also `Ctrl+Wheel`) |
