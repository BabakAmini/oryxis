viewport: 1200x750
mode: Zen
-----
# Manual Lock Vault asks first: it tears down every live session and
# tab (terminal panes, standalone SFTP tabs, RDP/VNC tunnels), so the
# trigger opens a confirm dialog instead of committing. This pins the
# dialog surface (impact note + Cancel/Lock pair), the negative path
# (Cancel keeps the app unlocked), the confirm path (lands on the lock
# screen), and that the confirm latch never survives an unlock (the
# dialog must not reappear over the unlocked app).
expect "Welcome to Oryxis"
click "Skip"
expect "Protect your vault"
# The onboarding password field is not auto-focused; click it like
# password-mask.ice does (its placeholder text is invisible to the
# text selectors).
click (550, 453)
type "testpass123"
# `settle` between the typing and the button, or the run is flaky:
# "Create Vault" only becomes pressable once the field holds a long
# enough password, so a click that arrives while the last keystrokes
# are still in flight hits a dead button, the vault is never created,
# and the failure surfaces two instructions later as a missing
# dashboard (the log gives it away: no "Vault master password set").
# Measured 1 in 5 without it and 0 in 16 with it; `password-mask.ice`
# does not need it because it never presses the button.
settle
click "Create Vault"
# `settle` before the assert: creating the vault runs an Argon2id
# derivation, which is deliberately slow and is slower still on a
# CI runner. Without it the assert can look at the screen while the
# app is still deriving and report the dashboard as missing.
settle
expect "Create host"

# Burger menu -> Lock Vault opens the confirm dialog, not the teardown.
# The menu closes when the dialog arms, so "Lock Vault" after this
# point is unambiguously the dialog's confirm button.
click (19, 20)
settle
click "Lock Vault"
settle
expect "Lock the vault?"
expect "Open connections: 0"
expect "Cancel"
expect "Lock Vault"
screenshot vault-lock-confirm

# Cancel keeps the app unlocked.
click "Cancel"
settle
expect "Create host"

# Confirming really locks.
click (19, 20)
settle
click "Lock Vault"
settle
expect "Lock the vault?"
click "Lock Vault"
settle
expect "Enter your master password to unlock."

# Unlock returns to the dashboard; the confirm latch was cleared by the
# lock, so the dialog must not reappear over the unlocked app. The lock
# screen auto-focuses the password field, so type goes straight in.
type "testpass123"
click "Unlock"
settle
wait 1500
settle
expect "Create host"
