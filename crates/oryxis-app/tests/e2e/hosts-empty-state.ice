viewport: 1200x750
mode: Zen
-----
# First-run Hosts screen. With an empty vault there is no toolbar
# (nothing to search / sort / filter / re-layout), and the "+ Host ▾"
# menu's own entries render as buttons under an "or" divider instead
# of hiding behind a chevron. The keyboard reaches the block through
# the content zone: Tab rings the hostname field, Enter focuses it,
# and submitting opens the pre-filled host editor.
settle 250
click "Skip"
click "Continue without password"
settle 250
expect "Create host"
expect "or"
expect "Import"
expect "Import ~/.ssh/config"
screenshot hosts-empty-state
type tab
type enter
type "10.0.0.5"
settle 250
type enter
settle 250
expect "New Host"
