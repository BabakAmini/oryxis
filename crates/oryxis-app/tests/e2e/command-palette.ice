viewport: 1200x750
mode: Zen
-----
# C4: command palette. Ctrl+Shift+P opens a fuzzy action search over
# every hotkey (plus hosts / Settings sections / a few extras). The
# rows, the title pill and the empty state are real text widgets, so
# the assertions ride those (the activated terminal canvas is not
# text-selectable, hence the final step is a screenshot). `timeout
# 500` only after a live shell opens: the zen emulator never quiesces
# with a running PTY.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
# Open the palette and confirm its identity pill.
type ctrl+shift+p
settle 400
expect "Command palette"
# A query that matches nothing shows the empty state.
type "zzzz"
settle 400
expect "No matching actions"
# Esc closes the palette (it rides the modal ESC layer); the dashboard
# is visible again underneath.
type escape
settle 300
expect "Create host"
# Reopen and fuzzy-match a real action row by label.
type ctrl+shift+p
settle 400
type "local shell"
settle 400
expect "Open local shell"
# Enter activates the top match via the two-step PaletteActivate
# dispatch: the palette closes and a local shell tab opens.
type enter
settle 500
timeout 500
screenshot command-palette-activated
