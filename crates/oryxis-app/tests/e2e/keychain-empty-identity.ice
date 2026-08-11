viewport: 1200x750
mode: Zen
-----
# The empty keychain used to offer the two key CTAs only, so a fresh
# vault had no way to reach the identity form: the "+ ADD" dropdown that
# carries it only exists once the keychain is non-empty. The empty state
# now renders the whole add catalog (views/add_actions.rs), hero plus
# secondary buttons, and every one of them is a keyboard row.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click "Keychain"
expect "Add a key"
expect "Generate key"
expect "Import Key"
expect "Import public key"
expect "Certificate"
expect "New Identity"
# Tab walks Search -> sub-nav -> content (no toolbar on this screen), so
# the third one rings the hero CTA. Down then walks the secondary stack:
# Import Key, Import public key, Certificate, New Identity, five rows in
# catalog order, and Enter opens the last one exactly like the click
# below does.
type tab
type tab
type tab
type down
type down
type down
type down
type down
type enter
settle 300
expect "Save Identity"
click "Cancel"
settle 300
click "New Identity"
expect "Save Identity"
click (960, 134)
type "e2e-identity"
click (960, 213)
type "root"
click "Save Identity"
settle 300
expect "e2e-identity"
screenshot keychain-empty-identity
