viewport: 1240x2100
mode: Zen
-----
# Highlight rules (C6): creating one, and what the list says about it.
#
# What the terminal DOES with a rule is not visible to a text selector
# (the grid is a canvas), so the colouring itself is covered by unit
# tests in oryxis-terminal and by screenshot during QA. What this file
# pins is the surface: the block exists, the editor opens as a modal and
# refuses a rule that cannot match, and a saved rule becomes a row
# carrying its pattern.
#
# Tall viewport on purpose: the block sits below the Appearance card in
# a long section.
click "Skip"
click "Continue without password"
settle
# Through the burger rather than the toolbar gear, so the flow does not
# depend on the gear's pixel position at this width.
click (19, 20)
settle
click "Settings"
settle
click "Terminal Settings"
settle
expect "Highlight rules"
expect "No rules yet."
click "Add rule"
settle
# The editor is a modal, titled for what it is about to do, and it says
# what a "pattern" is instead of leaving an example to be read as one.
expect "New highlight rule"
expect "The text to look for in what the terminal prints."
# An empty pattern is refused: it would match at every position, so
# saving it would paint the whole screen in the rule's colour.
click "Save"
expect "Enter something to match."
# A real rule. The name is what a notification would be titled with;
# the pattern is what is actually looked for.
click #set-hl-rule-name
type "Disk full"
click #set-hl-rule-pattern
type "No space left"
click "Save"
settle
# The row carries both: the name on top, the pattern as its summary.
expect "Disk full"
expect "No space left"
absent "No rules yet."
absent "New highlight rule"
# Reopening the rule shows the stored values back (the editor works on
# a copy, so this is what proves the copy was committed).
click "Edit"
settle
expect "Edit highlight rule"
# Esc closes the modal like Cancel does, and abandons only the copy.
type escape
settle
absent "Edit highlight rule"
expect "Disk full"
# Enter saves the form from any field (Save is the modal's default row),
# and a second rule ADDS rather than replacing: the panel-scope version of
# this path lost the earlier rules to a stray `on_submit` behind the modal.
click "Add rule"
settle
type "Out of memory"
click #set-hl-rule-pattern
type "OOM killer"
type enter
settle
absent "New highlight rule"
expect "Disk full"
expect "Out of memory"
expect "OOM killer"
# Deleting asks first, and Cancel means the rule stays.
click "Delete"
expect "Delete this rule?"
click "Cancel"
settle
expect "No space left"
