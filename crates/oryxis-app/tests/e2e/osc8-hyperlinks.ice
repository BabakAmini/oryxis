viewport: 1200x750
mode: Zen
-----
# C3: OSC 8 hyperlinks. Open a Local Shell, print a link whose visible
# label ("CLICK-ME") differs from its target, then hover the label and
# confirm the bottom-left reveal chip exposes the real target. The label
# lives in the terminal canvas (invisible to `expect`), but the reveal
# chip is a real text widget and IS asserted. A second link uses a
# `javascript:` scheme: the allowlist must refuse it, showing a
# "not allowed" chip instead of the target (no pointer / open affordance).
# `timeout 500` once a PTY is live: the zen emulator never quiesces with a
# running shell.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click (91, 20)
expect "Local Shell"
timeout 500
click "Local Shell"
settle 600
# Emit an OSC 8 link: ESC]8;;URI ST  LABEL  ESC]8;; ST.
#
# Printed TWENTY times, and each block starts with `clear`, because the
# hover below is a PIXEL coordinate: tying it to one printed row made it
# depend on how many rows the shell had already drawn, which is a
# property of the prompt and so of the machine. A CI runner draws a
# different number, the hover landed on a blank row, and the assert
# blamed the reveal chip for never appearing.
type "clear; printf '\\e]8;;https://example.com\\e\\\\CLICK-ME\\e]8;;\\e\\\\\\n%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20"
type enter
settle 700
# Hover the CLICK-ME label; the reveal chip should surface the target.
move (40, 123)
settle 500
screenshot osc8-reveal
expect "https://example.com"
# Now a disallowed scheme: its target must be withheld behind a notice.
# Its own `clear` puts this block at the same rows, so the two halves
# cannot drift apart as the first block's height changes.
move (600, 400)
type "clear; printf '\\e]8;;javascript:alert(1)\\e\\\\EVIL-LINK\\e]8;;\\e\\\\\\n%.0s' 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20"
type enter
settle 700
move (40, 123)
settle 500
screenshot osc8-blocked
expect "Link type not allowed: javascript"
