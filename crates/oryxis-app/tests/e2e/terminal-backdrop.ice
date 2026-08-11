viewport: 1400x2200
mode: Zen
-----
# Translucent terminal background: the Settings row, its index entry and
# the reveal that carries the search hit onto the row.
#
# What the row is worth pinning for: the effect itself is two gates
# (theme::terminal_bg_alpha), and the second one is the window's own
# surface, decided before this process ever drew a frame. Headless there
# is no window at all, so the batch can only assert the control and its
# copy; the gate logic is unit-tested
# (terminal_alpha_needs_both_a_transparent_window_and_a_reduced_setting)
# and the composited result needs a real desktop to look at.
#
# The search leg doubles as the SETTINGS_INDEX assertion: typing an
# English word that appears in no visible label (the row reads
# "Background opacity") can only match through the index keywords, so a
# missing entry fails here rather than silently degrading to "the
# setting exists but nobody can find it".
click "Skip"
click "Continue without password"
settle
click (19, 20)
settle
click "Settings"
settle
click "Terminal Settings"
settle
expect "Background opacity"
expect "Lets the desktop show through the terminal background. Panels, tabs and the status bar stay opaque."
click "Search settings"
type "transparency"
settle
expect "Background opacity"
# Background picture. The fit and fade rows deliberately do NOT exist
# until a picture is set (a control that governs nothing should not be
# on screen), so their absence here is the assertion, not an oversight.
# Picking a file needs the OS dialog, which the harness cannot drive:
# the geometry that would follow is unit-tested instead
# (widget::background::tests).
type ctrl+a
type "wallpaper"
settle
expect "Background image"
absent "Image fit"
absent "Fade image"
# Per-host overrides live in the host editor's Terminal card, in the
# same build order the keyboard walk records. The pickers' VALUES are
# invisible to text selectors, so this pins the rows themselves; which
# value each one resolves to is unit-tested (terminal_appearance::tests).
# Back to the vault by hotkey, not by the hamburger's "Hosts" row: the
# Terminal Settings page grew a "Sidebar tab locations" block (issue
# #102) whose first dock row is also labelled "Hosts", far below the
# fold, and the text selector resolves to that one instead of the menu
# entry (a click on an off-screen target then fails).
type ctrl+shift+1
settle
click "Continue"
settle
expect "Background opacity"
expect "Background image"
