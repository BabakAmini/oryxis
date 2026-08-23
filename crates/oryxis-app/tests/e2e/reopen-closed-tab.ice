viewport: 1200x750
mode: Zen
-----
# Issue #186: closing a tab is one click on a small target, and the
# session behind it is not recoverable any other way, so the last closed
# tab can be brought back with Ctrl+Shift+Y (also on the tab context
# menu and in the command palette).
#
# A local shell is the whole flow's subject on purpose: it needs no
# network, and it exercises the same machinery a saved host does, since
# the reopen resolves a `PinnedTabSpec` through `spec_open_message`, the
# one authority a dormant pin also reopens through.
#
# The assertions are the tab label leaving and coming back. Closing the
# only tab lands on Hosts, so "bash (default)" is genuinely gone from the
# frame (the status bar's own segment renders with a leading dot, a
# different string, and disappears with the tab anyway).
settle 250
click "Skip"
settle 250
click "Continue without password"
settle 250
# A live PTY never lets the emulator quiesce, so cap the per-instruction
# wait instead of burning the full timeout on every line.
timeout 500
type ctrl+shift+l
settle 250
expect "bash (default)"
# Through the tab context menu, one of the three close paths that reach
# `close_tab_now`. Coordinates rather than a text selector: the chip's
# own label is the string being asserted on.
click right (150.00, 20.00)
settle 250
click "Close Tab"
settle 250
absent "bash (default)"
type ctrl+shift+y
settle 250
expect "bash (default)"
