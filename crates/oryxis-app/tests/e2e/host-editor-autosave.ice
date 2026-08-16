viewport: 1400x900
mode: Zen
-----
# Two-tier host editor, P3 + P4: the create flow carries the preset
# chips, an EXISTING host has no Save button (the footer states the
# auto-save contract instead), a debounced edit persists on its own
# (the grid card renames while the panel is still open), and the X
# close flushes an edit still inside the debounce window. Input VALUES
# are invisible to text selectors, so the evidence is the CARD label
# in the grid, which only changes when the vault row did.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click "Continue"
expect "New Host"
# P3: the one-shot starting-point chips, create flow only.
expect "Start from"
expect "Basic SSH"
expect "Via bastion"
click "IP or Hostname"
type "10.9.8.7"
click (1190, 219)
type "AutoSaveHost"
click "Save"
settle 500
expect "AutoSaveHost"
# Edit the saved host: the drawer footer states auto-save, no Save.
click right "AutoSaveHost"
settle 300
click "Edit"
settle 400
expect "Edit Host"
expect "Changes are saved automatically"
absent "Start from"
# Rename and WAIT past the 700 ms debounce: the grid card renames
# while the panel is still open, which is the persist itself.
click (1190, 182)
type ctrl+a
type "RenamedLive"
wait 1100
settle 300
expect "RenamedLive"
# Rename again and close through the X inside the debounce window:
# the close flushes, so the card shows the newest name.
click (1190, 182)
type ctrl+a
type "FlushOnClose"
click (1369, 72)
settle 400
expect "FlushOnClose"
screenshot host-editor-autosave
