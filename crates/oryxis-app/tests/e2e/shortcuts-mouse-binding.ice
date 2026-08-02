viewport: 1200x750
mode: Zen
-----
# Settings > Shortcuts: middle-click paste is an ordinary CHORD on
# "Paste selection (primary)", not a setting of its own. This pins the
# two surfaces staying in sync, in both directions:
#
#   * the factory row carries the "Middle click" chip and no per-row
#     "Reset" (it matches the factory list, so nothing to reset),
#   * unticking Terminal Settings' "Middle-click pastes" drops the
#     chord, which makes the row diverge and grow that "Reset",
#   * re-ticking restores the exact factory list.
#
# Reboot survival (the drop is a real binding row, and the one-shot
# middle-click migration must not hand the chord back on the next boot)
# is checked interactively with the daemon's `reset`, which the batch
# grammar doesn't take.
#
# `expect` is exact-match, which is what makes "Reset" a usable signal:
# the only other Reset-ish labels on this screen ("Reset all to
# defaults", "Reset font zoom") never match it.
#
# The capture flow itself (pressing a mouse button to RECORD one) is
# owner QA: the harness grammar has no middle click.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
settle
# Toolbar gear (icon only, no text selector).
click (1175, 64)
settle
click "Shortcuts"
settle
expect "Middle click"

click "Terminal Settings"
settle
click (95, 68)
type "middle"
settle
expect "Middle-click pastes"
# Off: the chord leaves the binding table.
click (1142, 70)
settle
click "Shortcuts"
settle
find "Middle click"
expect "Reset"

# On again: back to the exact factory list, chip included.
click "Terminal Settings"
settle
click (95, 68)
type "middle"
settle
click (1142, 70)
settle
click "Shortcuts"
settle
expect "Middle click"
