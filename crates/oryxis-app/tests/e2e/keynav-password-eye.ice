viewport: 1200x900
mode: Zen
-----
# Issue #52: the password reveal eye is a keyboard stop right after
# its field. Tab from the password input rings the eye, Enter toggles
# the reveal, Space toggles it back. text_input VALUES are invisible
# to `expect`, so the toggle evidence lives in the screenshots
# (plaintext + eye-off icon in the revealed shot, dots in the masked
# ones).
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
# First run has no toolbar: Continue on the empty first-run screen is
# the "+ Host" path and opens the same editor.
click "Continue"
expect "New Host"
type "10.9.9.9"
# Two-tier editor: the password row is part of the always-visible
# essential tier; the 900-tall viewport keeps it above the fold now
# that the preset-chip row (P3) sits above the form.
settle 500
click (1000, 757)
type "hunter2"
type tab
settle 300
screenshot keynav-eye-ringed
type enter
settle 300
screenshot keynav-eye-revealed
type " "
settle 300
screenshot keynav-eye-masked
expect "New Host"
