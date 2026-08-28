viewport: 1200x750
mode: Zen
-----
# The SFTP console's way in (issue #188).
#
# What this guards is the DOOR, not the console. Opening one needs a
# reachable SSH host, which a CI run has none of; the loop behind the
# door is covered by unit tests and by the Docker-gated integration
# suite in oryxis-ssh. The door is the part that disappears silently,
# because it is one conditional row among several in a menu, and nothing
# compiles a menu.
#
# The exclusions (non-SSH protocols, mosh hosts) are asserted in
# `app_tests.rs` instead: they are a question about a Connection, and a
# unit test answers it for every protocol at once rather than driving a
# pick_list by coordinates.
expect "Welcome to Oryxis"
click "Skip"
settle
click "Continue without password"
settle

# A saved SSH host, which is what the entry is gated on.
click "Type IP or Hostname"
type "example.com"
click "Continue"
settle
click "My Server"
type "ssh-host"
click "Save"
settle
expect "ssh-host"

# The card menu offers the console beside the browser tab. Both, because
# they are two answers to one question and issue #188 exists because
# somebody wanted the other one.
click right "ssh-host"
settle
expect "Open SFTP Tab"
# Short label on purpose: this popover is fixed-width and its height
# estimate assumes one line per row, so a label that WRAPS pushes the
# last row (Remove) off the window. That is a real regression this
# suite caught, in `editor-delete-stays-deleted.ice`.
expect "SFTP Console"
