viewport: 1200x750
mode: Zen
-----
# C1: scrollback find-bar. Open a local shell, print two lines that
# share a needle, then Ctrl+F opens the find-bar over the terminal.
# Typing the needle finds every occurrence (2 echoed command lines +
# 2 output lines = 4) and the counter reads "1 / 4"; Enter steps to
# "2 / 4"; Esc closes the bar. The match highlights live in the
# terminal canvas (not text-selectable), so the assertions ride the
# find-bar's own text_input placeholder + counter, which are real
# widgets. `timeout 500` once the PTY is live: the zen emulator never
# quiesces with a running shell, so each instruction would otherwise
# burn the full timeout.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click (91, 20)
expect "Local Shell"
timeout 500
click "Local Shell"
settle 400
type "echo needle-alpha; echo needle-beta; echo other"
type enter
settle 500
type ctrl+f
settle 400
expect "Find in buffer"
type "needle"
settle 400
expect "1 / 4"
type enter
settle 300
expect "2 / 4"
type escape
settle 300
screenshot terminal-search-closed
