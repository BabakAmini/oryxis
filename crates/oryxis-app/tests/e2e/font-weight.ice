viewport: 1400x2200
mode: Zen
-----
# Terminal font weight (#155): the picker under the font family, and
# the honesty line that fires when the picked family has no face at the
# picked weight.
#
# The bundled default (SauceCodePro Nerd Font) ships Regular and Medium
# and nothing heavier, so SemiBold is a request it cannot serve
# exactly: the hint must appear. That single assertion pins the whole
# availability path (BUNDLED_MONO_WEIGHTS -> the font scan ->
# terminal_font_serves_weight -> the view), which no unit test can
# reach, plus the i18n key behind the line.
#
# Nothing here downloads: a pack family would, so the flow never leaves
# the bundled font.
click "Skip"
click "Continue without password"
settle
click (19, 20)
settle
click "Settings"
settle
click "Terminal Settings"
settle
expect "Terminal Font Weight"
# The picker sits ~45 px under its label (label 17 high + 8 gap + the
# 40 px row). Read the label's own bounds with `texts` if the card
# above it grows and these drift.
click (300, 1250)
settle 300
# Dropdown options, top to bottom: Regular / Medium / SemiBold / Bold.
# It opens UPWARDS over the label, so SemiBold lands above the picker.
click (300, 1170)
settle 300
expect "This font has no face at the selected weight, so the terminal uses the closest one it has."
screenshot font-weight-semibold
