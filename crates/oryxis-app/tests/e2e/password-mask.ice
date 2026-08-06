viewport: 1200x750
mode: Zen
-----
# Issue #134: password fields render masked, the eye toggles, and the
# caret survives the round trip. text_input VALUES are invisible to
# `expect`, so the assertions ride on the clipboard: a copy out of a
# masked field yields the bullets that are on screen, and the same copy
# after revealing yields the real value. That also pins the insertion
# point, since a caret sent back to zero would spell "Zhunter2".
expect "Welcome to Oryxis"
click "Skip"
expect "Protect your vault"
click (550, 453)
type "hunter2"
settle 300
screenshot password-mask-masked
click (730, 453)
settle 300
click (730, 453)
settle 300
type "Z"
settle 300
clipboard "seed"
type ctrl+a
type ctrl+c
settle 300
clipboard is "••••••••"
screenshot password-mask-after-toggle
click (730, 453)
settle 300
type ctrl+a
type ctrl+c
settle 300
clipboard is "hunter2Z"
screenshot password-mask-revealed
