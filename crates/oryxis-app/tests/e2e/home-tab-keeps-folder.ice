viewport: 1200x750
mode: Zen
-----
# Two doors into the host list, two answers, and this test pins both.
#
# The Home tab (house chip) comes back to the FOLDER the user was
# standing in; the Hosts entries (sub-nav pill, burger row, and the
# shortcut they render) stay the one-click way back out to the root.
#
# The discriminator is the subgroup: "WebTier" only renders while
# "Prod" is open, so an `expect` on it proves which of the two the
# click landed on. "Groups" is the mirror image: the root list's
# section header, absent inside a folder.
click "Skip"
click "Continue without password"
settle 250
# A group at the root, then a subgroup inside it.
click "New group"
settle 250
click (900.00, 145.00)
type "Prod"
settle 250
click "Save"
settle 250
click "Prod"
settle 250
# The "+ HOST" split menu carries "New subgroup" while a folder is open.
click (1149.00, 119.00)
settle 250
click "New subgroup"
settle 250
click (900.00, 145.00)
type "WebTier"
settle 250
click "Save"
settle 250
expect "WebTier"
# Leave the vault surface entirely, then come back through the house
# chip: the folder survives the round trip.
click "Keychain"
settle 250
click (57.00, 20.00)
settle 250
expect "WebTier"
# The sub-nav pill is the opposite door: root, from inside the folder.
click "Hosts"
settle 250
expect "Groups"
# And its burger row mirrors the pill rather than the house chip (it
# renders the pill's shortcut next to itself, so they must agree).
click "Prod"
settle 250
click (19.00, 20.00)
settle 250
click (55.00, 91.00)
settle 250
expect "Groups"
