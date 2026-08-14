viewport: 1400x1100
mode: Zen
-----
# Issue: Tab (or arrows) from a MOUSE-focused Settings field must walk
# the recorded rows (the panel contract), not park the keynav ring on
# the first content row — the Security section's "Vault Password"
# toggle — and scroll the page away. Regressed as: type in the export
# password, press Tab, and the ring lands on the dialog's reveal eye,
# so Enter toggles the eye instead of flipping the master-password
# switch (which would open the "Set Password" form). The sync
# passphrase field (set-sync-sftp-passphrase) has the same problem.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
settle
# Through the burger menu rather than the toolbar gear, so the flow
# doesn't depend on the gear's pixel position at this viewport width.
click (19, 20)
settle
click "Settings"
settle
expect "Security & Privacy"
click "Security & Privacy"
settle
expect "Export Vault"
click "Export Vault"
settle
# Mouse-focus the export password field (the path the bug needs) and
# type a digit.
click #set-export-password
settle
type "2"
settle
# Tab must walk to the dialog's eye; Enter toggles the reveal. If the
# ring had jumped to the Vault Password toggle (the bug), Enter would
# have opened the set-password form instead.
type tab
type enter
settle
absent "Set Password"
expect "Export Vault"
# Arrows stay native while a mouse-focused field owns the keyboard:
# no ring jump, so Enter still cannot reach the Vault Password toggle.
click #set-export-password
settle
type down
settle
type enter
settle
absent "Set Password"
expect "Export Vault"
# Tab must land ON an input row with the ring IDLE (the search-zone
# invariant): from the Port field, Tab walks to the adjacent Keepalive
# input with real iced focus and no ring. If the row were ringed on top
# of the focus, the arrows below would walk the ring up to the "New
# connection defaults" header row and Enter would collapse the card,
# hiding the row the final expect needs. Sidebar click by coordinates:
# the Security section still on screen shows "Connection history",
# which a substring text selector for "Connection" could hit.
click (59, 201)
settle
expect "Keepalive (override)"
click #set-connection-default-port
settle
type tab
settle
type up
type up
type up
settle
type enter
settle
expect "Keepalive (override)"
