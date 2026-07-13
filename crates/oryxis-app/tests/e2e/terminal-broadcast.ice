viewport: 1200x750
mode: Zen
-----
# C2: broadcast input across split panes. Open a Local Shell, split it
# side-by-side (the split prompts the picker for the new pane's
# content), arm broadcast from the status-bar segment, then type one
# command and confirm BOTH panes ran it. The pane output lives in the
# terminal canvas (invisible to `expect`), so the screenshots are the
# assertion; the status "Broadcast" segment and the picker rows are
# real widgets and ARE asserted. `timeout 500` once a PTY is live: the
# zen emulator never quiesces with a running shell.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click (91, 20)
expect "Local Shell"
timeout 500
click "Local Shell"
settle 400
# Split via the tab context menu (robust vs the split hotkey under the
# emulator). The split opens the picker to choose the new pane.
click right (148, 20)
settle 300
expect "Split side by side"
click "Split side by side"
settle 300
expect "Local Shell"
click "Local Shell"
settle 500
# Arm broadcast from the status bar and type one command into both.
expect "Broadcast"
click "Broadcast"
settle 400
type "echo BCAST-HELLO"
type enter
settle 600
screenshot terminal-broadcast-both
