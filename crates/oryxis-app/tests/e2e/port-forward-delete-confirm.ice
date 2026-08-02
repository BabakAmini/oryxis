viewport: 1200x750
mode: Zen
-----
# Issue #119: deleting a port-forward rule asks first, and the prompt
# names the rule the way its card does so you can tell which one you
# picked. A rule can carry a jump chain, a bound address and an
# auto-start flag, and there is no undo short of restoring a portable
# export.
#
# Guards the gate in `RequestDeletePortForwardRule`, which is where BOTH
# affordances land (the hover trash and the Delete row in the edit
# panel), so neither can regress back to a one-click teardown.
#
# The `find` calls are LOAD-BEARING, not assertions. The headless
# emulator only rebuilds its widget tree when something walks it, so a
# coordinate click into a surface that just mounted (a panel, a
# hover-revealed button) lands on nothing until one does. `find` runs
# that walk without writing a PNG, which is why it is used here instead
# of the `screenshot` the same problem is usually solved with.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"

# A rule needs a host to travel through, so make one first. Continue
# opens the panel with the hostname field already focused; the label is
# separate and also required.
click "Continue"
settle 400
find "Label"
type "db.example.com"
click (988, 182)
type "db-prod"
click "Save"
settle 700

# One local forward, filled by coordinate: these are text_inputs, whose
# VALUES text selectors cannot see.
click "Port Forwarding"
settle 400
click "New Port Forward"
settle 400
find "Name"
click (988, 145)
type "db tunnel"
click (988, 442)
type "8080"
click (988, 517)
type "db.internal"
click (988, 591)
type "5432"
# "Via host" is a pick_list; its options are not reachable by text
# selector, so the single entry is clicked where it drops.
click (822, 293)
settle 300
find "db-prod"
click (852, 335)
settle 300
click "Save"
settle 700
expect "db tunnel"

# The kebab is hover-revealed (floating-action convention), so the
# cursor has to be over the card AND the tree rebuilt before the click
# can land on a button that did not exist a moment ago.
move (200, 190)
settle 300
find "db tunnel"
click (377, 189)
settle 400
# Edit sits beside Delete on purpose: a card menu offering only the
# destructive action reads like that is all the card can do.
expect "Edit"
click "Delete"
settle 400
expect "Remove?"
# Cancel, not Close: the negative line of a destructive prompt must not
# read as a second way to do the thing (#112).
expect "Cancel"
# The body names the rule the way its card does, label AND ports,
# because several rules can share a label. That line cannot be asserted
# with `expect`, and not for want of trying: the dialog body is a
# SELECTABLE rich_text (so the user can copy a failure message out of
# it), and iced's `rich_text::operate` only ever publishes `selectable`,
# never `text`. Plain `text` publishes both. So the body is invisible to
# every text selector by construction, and making it visible would mean
# editing the fork. The shot is the record instead.
screenshot port-forward-delete-confirm

# Cancelling keeps the rule.
click "Cancel"
settle 400
expect "db tunnel"

# Confirming really removes it. Same kebab, same hover-then-walk dance.
move (200, 190)
settle 300
find "db tunnel"
click (377, 189)
settle 600
click "Delete"
settle 600
expect "Remove?"
click "Remove"
settle 600
expect "Forward a port"
