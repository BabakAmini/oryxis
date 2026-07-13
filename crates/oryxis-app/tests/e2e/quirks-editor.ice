viewport: 1000x2560
mode: Zen
-----
# C5: the host editor's "Advanced terminal" section (per-host legacy
# keyboard modes + feature toggles). A tall viewport so the whole form
# fits (the side-panel scrollable can't be wheel-scrolled headless).
# This smoke test proves the section is wired and its i18n resolves; the
# byte-level encoding + the form save/load round-trip are unit-tested
# (util.rs vectors, the vault + core round-trip tests). The pick-list
# option overlays aren't text-selectable, so flipping a value is left to
# the unit tests.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click "Create host"
settle 300
click "Type IP or Hostname"
type "10.0.0.5"
click "Continue"
settle 500
expect "Advanced terminal"
expect "Backspace key"
expect "Home/End keys"
expect "Function keys"
expect "Report mouse to remote"
expect "Clipboard (OSC 52)"
expect "Rekey limit (MB)"
screenshot quirks-editor
