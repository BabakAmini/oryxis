viewport: 1400x900
mode: Zen
-----
# Two-tier host editor, P3 + P4: the create flow carries the preset
# chips, and an EXISTING host is saved by CLOSING it.
#
# Rewritten for the close-only contract. The earlier revision persisted
# on a 700 ms debounce and this test asserted that: the footer legend
# ("Changes are saved automatically") and a card that renamed itself
# while the panel was still open. Both are gone on purpose. The debounce
# was the origin of a whole bug class, because every mid-typing save
# re-sorts the host list under whatever is holding a position into it;
# one write per editing session removes the class instead of guarding
# each site, and the footer says nothing because there is nothing left
# to explain.
#
# So the assertions INVERT: after the rename the card still reads the
# OLD name (nothing was written yet), and only the X close makes the
# new one appear. Input VALUES are invisible to text selectors, so the
# card label in the grid is the evidence either way: it only changes
# when the vault row did.
#
# The host is deliberately not named "...Save...": selectors match by
# SUBSTRING, so `absent "Save"` would hit the card, not the button.
click "Skip"
click "Continue without password"
expect "Create host"
click "Continue"
expect "New Host"
expect "Start from"
expect "Basic SSH"
expect "Via bastion"
click "IP or Hostname"
type "10.9.8.7"
click (1190.00, 219.00)
type "PanelHost"
click "Save"
settle 500
expect "PanelHost"
click right "PanelHost"
settle 300
click "Edit"
settle 400
expect "Edit Host"
absent "Start from"
absent "Changes are saved automatically"
absent "Save"
click (1190.00, 182.00)
type ctrl+a
type "RenamedOnClose"
wait 1100
settle 300
expect "PanelHost"
absent "RenamedOnClose"
click (1369.00, 72.00)
settle 400
expect "RenamedOnClose"
