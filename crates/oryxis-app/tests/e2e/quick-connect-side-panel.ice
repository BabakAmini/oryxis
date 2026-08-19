viewport: 1200x750
mode: Zen
-----
# Issue #175: with the host editor open, Enter in the search box did
# nothing at all.
#
# The keyboard router hands the WHOLE keyboard to the side panel while
# one is open, and the search box is "zone zero" (iced cannot report a
# text_input's focus), so its Enter never reached the activate path and
# quick connect was dead for as long as the panel stayed up. The panel
# now declines a bare Enter it has no ringed row for, and only a NAMED
# target goes through: `user@host` is unambiguous, "whatever sorts
# first" is not.
#
# The dial target is a `.invalid` name (RFC 2606: guaranteed never to
# resolve), so the attempt ends on a fast DNS failure instead of sitting
# out the 15 s connect timeout. What this asserts is that the attempt
# HAPPENS at all.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click #empty-quick-host
type "myserver.example.com"
settle 250
type enter
settle 250
expect "New Host"
click #editor-label
type "lab"
settle 250
click "Save"
settle 250
expect "lab"

# Open the editor on that host, the way the report did.
click right "lab"
settle 250
click "Edit"
settle 250
expect "Edit Host"

# Typing must not connect anything: the panel declines ordinary
# characters too, so a fallthrough on those would dial `roo` on the
# third keystroke of `root@...`.
click #search-dashboard
type "root@nothing.invalid"
settle 300
expect "Quick connect: root@nothing.invalid"
expect "Edit Host"

# Enter reaches it now.
type enter
settle 500
expect "SSH nothing.invalid:22"
