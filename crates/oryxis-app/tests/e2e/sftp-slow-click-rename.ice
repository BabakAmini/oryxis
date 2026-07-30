viewport: 1200x750
mode: Zen
-----
# Slow-click rename in the SFTP pane: a second plain click ON THE NAME of the
# row that is already the lone selection, later than a double-click, opens the
# inline editor. The gates that keep it off real open attempts (the hit test
# against the drawn name, the cancellable deferral) are unit-tested in
# dispatch_sftp::selection; this covers the whole click path end to end.
#
# Two deliberate choices in how the row is targeted:
# - coordinates, not `click "name"`: a text selector aims at the CENTRE of the
#   label widget, which spans the whole Name column, so it would land past the
#   end of a short name and (correctly) arm nothing.
# - the pane filter, so exactly one entry is listed and the row's y is fixed.
#   Without it the sandbox home's own `shots/` directory sorts first (dirs
#   before files) and the click would land on THAT row.
click "Skip"
click "Continue without password"
settle
type ctrl+shift+e
settle
# Dismiss the host picker: this flow only needs the Local pane.
click (785, 166)
settle
# Create the subject file, so the test never depends on what the sandbox
# home happens to contain.
click right (300, 400)
settle
click "New file"
settle
click (599, 365)
type "slowclick.txt"
click "Create"
settle
expect "slowclick.txt"
# Filter down to it: row 1 is "..", so the file is row 2 (y 194).
click (453, 72)
type "slowclick"
settle
expect "slowclick.txt"
# Select it, wait past the dead zone, then click the name again: the editor
# opens after the deferral with no further click.
move (60, 194)
click (60, 194)
wait 1300
click (60, 194)
wait 900
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type "renamed.txt"
type enter
settle
# The filter still says "slowclick", so a listing that still showed the old
# name would keep matching; clear it and assert on the new name.
click (453, 72)
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
type backspace
settle
expect "renamed.txt"
