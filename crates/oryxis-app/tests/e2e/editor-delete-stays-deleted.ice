viewport: 1200x750
mode: Zen
-----
# Deleting the host you have OPEN in the editor keeps it deleted.
# The close-only auto-save flushes on the drawer's visibility edge,
# and `DeleteConnection` closes the drawer without clearing the form:
# with the row already gone from `connections`, its stored group read
# back as "", so the flush found the form dirty and upserted the
# deleted id straight back in. Any host in a group came back on every
# delete. The group's own "0 hosts" is the assertion: a card count is
# real text, where a text_input VALUE would be invisible to `expect`.
click "Skip"
click "Continue without password"
settle 250
click "Type IP or Hostname"
type "web01"
click "Continue"
settle 250
click "My Server"
type "web01"
click "Production, Staging..."
type "Prod"
settle 250
click "Save"
settle 250
expect "1 host"
click right "web01"
settle 250
click "Edit"
settle 250
click right "web01"
settle 250
click "Remove"
settle 250
click "Remove"
settle 250
expect "0 hosts"
