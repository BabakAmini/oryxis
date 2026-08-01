viewport: 1200x750
mode: Zen
-----
# Issue #112: closing a GROUPED tab asks first, closing a single-pane
# tab does not. Regression guard for the gate in
# `handle_close_tab`, which sits there (rather than at each call site)
# precisely so the strip X, the context menu and Ctrl+W are all
# covered.
#
# Local shells, so the flow needs no network and no host. A live PTY
# never lets the emulator quiesce, hence the raised timeout: without
# it every instruction after the first shell burns the full default.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
timeout 800
type ctrl+shift+l
settle 300

# Split the tab: one chip, two live sessions.
click right (150, 20)
expect "Split side by side"
click "Split side by side"
settle 300
click "Local Shell"
settle 500

# The X on the grouped chip must NOT close it outright.
click (95, 20)
settle 300
expect "Close this group?"
# Negative button reads Cancel, not Close: beside "Close group" the
# word Close would look like a second way to do the thing.
expect "Cancel"
expect "Close group"
screenshot tab-group-close-confirm

# Cancel keeps every session.
click "Cancel"
settle 300
expect "bash (default)"

# A single-pane tab still closes with no prompt at all.
type ctrl+shift+l
settle 500
click (248, 20)
settle 300
expect "bash (default)"

# Confirming really does close the group.
click (95, 20)
settle 300
expect "Close group"
click "Close group"
settle 500
expect "Create host"
