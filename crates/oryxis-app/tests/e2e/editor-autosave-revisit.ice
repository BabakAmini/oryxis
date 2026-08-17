viewport: 1200x750
mode: Zen
-----
# An edit made AFTER the editor drawer comes back is still saved.
# Leaving the Dashboard hides the drawer, which flushes and drops the
# baseline; returning re-renders the same live form. Without a
# re-baseline on the rising edge, `editor_autosave_kick` recorded the
# snapshot AFTER the first edit's handler, so that edit equalled its
# own baseline and was never written. One keystroke is the whole
# repro: from the second on, the baseline predates the change.
# The label is what carries the assertion, since it also renders on
# the card, outside the text_input `expect` cannot read.
click "Skip"
click "Continue without password"
settle 250
click "Type IP or Hostname"
type "web01"
click "Continue"
settle 250
click "My Server"
type "web01"
settle 250
click "Save"
settle 250
click right "web01"
settle 250
click "Edit"
settle 250
click "Keychain"
settle 250
click "Hosts"
settle 250
click (1120.00, 183.00)
type "9"
settle 250
click (1169.00, 72.00)
settle 250
expect "web019"
