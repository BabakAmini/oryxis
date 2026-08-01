viewport: 1200x750
mode: Zen
-----
# Issue #112: dragging a tab onto the content area merges it into the
# tab showing there, and WHERE you release picks the split. This is the
# reporter's first case: one pane on screen, release on its right, the
# arriving session lands beside it.
#
# The gesture only works because iced's `button` publishes `on_press` on
# the RELEASE, so grabbing a chip does not select it and the content area
# keeps showing the destination. If that ever changes, this test fails at
# the "Close this group?" assertion (the drag would be merging a tab into
# itself, which is refused), which is exactly the alarm we want.
#
# Local shells, so the flow needs no network and no host. A live PTY
# never lets the emulator quiesce, hence the raised timeout.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
timeout 800
type ctrl+shift+l
settle 300
type ctrl+shift+l
settle 500

# Grab the FIRST chip. The second one is active, so the content area
# below is showing it, and it is the destination.
press (140, 20)
move (160, 26)
move (600, 300)
# Right third of the pane, clear of the grid's own outer band: this is a
# per-pane anchor, so the preview is the right HALF of that pane.
move (1000, 400)
settle 300
# LOAD-BEARING, not decoration: the drop hit-tests the panes' last-drawn
# rects (`Pane.bounds`), and the headless emulator only draws when a
# screenshot is taken. Without a shot somewhere in the gesture every rect
# is still the zeroed one its cell was born with, nothing is proposed,
# and the release falls through to a plain reorder. The windowed app
# draws every frame, so this is a harness artifact only.
screenshot tab-drag-split-preview
release (1000, 400)
settle 600

# One chip left, and it is a group: the close X now has to ask, which is
# only true when the tab really owns both sessions.
click (95, 20)
settle 300
expect "Close this group?"
expect "Close group"
screenshot tab-drag-split-merged
click "Cancel"
settle 300
expect "bash (default)"
