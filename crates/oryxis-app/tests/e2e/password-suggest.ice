viewport: 1200x750
mode: Zen
-----
# Issue #117: a pane blocking on a password prompt offers the
# credentials the vault holds, at the caret, and never sends one on its
# own.
#
# `read -s -p` is the fixture rather than `sudo`: it prints the exact
# prompt with echo off and blocks, which is the whole shape the
# detector keys on, without needing a privileged binary on the CI box.
#
# Setup is coordinate-driven because none of it is text-selectable: a
# key must exist before the keychain toolbar (and its "+ ADD" split
# button) renders at all, and the identity form's fields are
# text_inputs, whose placeholders the text selector cannot match.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click "Keychain"
expect "Add a key"
# A key first: the empty keychain shows no toolbar, so "+ ADD" has
# nowhere to be until the list is non-empty.
click "Generate key"
expect "Generate key"
click (989, 137)
type "qa-key"
click "Generate"
settle 300
click "Done"
settle 300
# The "+ ADD" chevron. The screenshot before it is not decoration: menus
# anchored on real widget bounds read the LAST DRAWN rect, and the
# emulator only draws on `screenshot`.
screenshot pwsuggest-keychain
click (1155, 120)
expect "New Identity"
click "New Identity"
expect "Save Identity"
click (989, 147)
type "ops-admin"
click (1000, 225)
type "wilson"
click (1000, 304)
type "hunter2"
click "Save Identity"
settle 300
expect "ops-admin"
screenshot pwsuggest-identity

# A local shell: the popup's identity rows are origin-independent, so
# this covers the whole path without a server.
click (92, 20)
expect "Local Shell"
click "Local Shell"
settle 500
# A live PTY never lets the emulator quiesce, so every instruction from
# here would otherwise burn the full timeout.
timeout 500
wait 1500
# The popup anchors at the caret, which needs the pane's DRAWN rect.
# The emulator only draws on `screenshot`, so this one is load-bearing:
# without it the pane has never reported its bounds and the popup has
# nowhere to open. A real window draws every frame.
screenshot pwsuggest-shell

# 1. The prompt raises the popup, anchored at the caret.
type "read -s -p '[sudo] password for wilson: ' PW"
type enter
wait 1200
expect "Stored passwords"
expect "ops-admin"
screenshot pwsuggest-popup

# 2. Esc hides it and the prompt is still waiting: nothing was sent.
type escape
wait 400
absent "Stored passwords"
type enter
wait 600

# 3. A prompt asking the user to CHOOSE a password never offers one.
# `passwd` would help them overwrite their own password with itself.
type "read -s -p 'New password: ' PW2"
type enter
wait 1200
absent "Stored passwords"
type enter
wait 600

# 4. Clicking a row sends that credential, exactly and only it. No
# `move` first, deliberately: the click must work with the cursor
# already sitting where the popup opened, which is the normal case
# (the popup anchors at the caret, where the user last clicked) and
# the one where iced never fires a hover.
type "read -s -p 'Password: ' PW3"
type enter
wait 1200
expect "Stored passwords"
click (200, 198)
wait 800
absent "Stored passwords"
type "echo [$PW3]"
type enter
wait 800
# What landed in the shell can only be checked visually: text selectors
# do not reach inside the terminal canvas. The screenshot must show
# `[hunter2]`, the stored password and nothing else.
screenshot pwsuggest-sent

# 5. The popup follows its prompt. Cancelling the prompt (Ctrl+C ends
# the `read`, the shell prompt returns) must close the popup on the
# next output: a pick on a leftover popup would type the password into
# an ECHOING shell, putting it on screen, in the scrollback and in the
# session recording.
type "read -s -p 'Password: ' PW4"
type enter
wait 1200
expect "Stored passwords"
type ctrl+c
wait 800
absent "Stored passwords"
