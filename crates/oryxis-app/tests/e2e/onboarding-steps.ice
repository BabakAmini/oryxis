viewport: 1240x1500
mode: Zen
-----
# The two configuration steps of the first-run carousel (the marketing
# slides are covered by onboarding.ice, which also proves Skip still
# jumps straight to the password step).
#
# The import step cannot import: no vault exists while this screen is
# up. Accepting it records the intent and the hub opens right after
# the vault is created, which is the whole point of the step and what
# the tail of this test pins.
expect "Welcome to Oryxis"
click "Next"
click "Next"
click "Next"
click "Next"
settle
# Optional features: live toggles, same switches as Settings.
expect "Make it yours"
expect "AI Assistant"
expect "Host monitoring"
click "Next"
settle
# Import offer.
expect "Bring your hosts along"
expect "Import my hosts"
click "Import my hosts"
settle
# Accepting jumps to the password step (nothing to import into yet).
expect "Protect your vault"
click "Continue without password"
settle
# ... and the vault's first frame carries the import hub.
expect "Import hosts"
expect "Choose file..."
