viewport: 1200x750
mode: Zen
-----
# Delete on a selected SFTP row opens the confirm dialog. The key had no
# arm in the SFTP key router at all, so it did nothing at all.
#
# The test creates the row it deletes: the sandbox home is empty at
# first run, so there is nothing here to select otherwise.
click "Skip"
click "Continue without password"
settle 250
# New-tab button and the host picker's close: neither carries text, so
# both are coordinates. Retake them if the tab strip's metrics change.
click (92.00, 20.00)
settle 250
click "SFTP"
settle 250
click (785.00, 166.00)
settle 250
# The local pane's kebab menu, then its New file entry.
click (573.00, 72.00)
settle 250
click "New file"
settle 250
# The modal's input does not take focus on open.
click (599.00, 365.00)
type "delete-me.txt"
click "Create"
settle 250
click "delete-me.txt"
settle 250
type delete
settle 250
expect "Delete this item?"
click "Cancel"
settle 250
# The other half of the arm is that Delete stays inert on the `..` row,
# which cannot be asserted here: `.ice` has no negative expectation, and
# a click aimed behind an open modal lands on its scrim rather than
# failing. Verified by hand instead (two `type up` from the file reach
# `..`, since the list sorts directories first, and Delete then opens
# nothing).
