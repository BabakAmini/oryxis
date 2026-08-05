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
