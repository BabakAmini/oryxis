viewport: 1200x750
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
click "HOST"
expect "New Host"
type "10.9.9.9"
scroll pixels (0, -400) (990, 400)
settle 500
click (1000, 320)
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
