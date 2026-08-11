viewport: 1200x750
mode: Zen
-----
# Issue #148: Settings > About counted only SSH keys under "Keychain",
# so a vault whose keychain held identities reported 0. Seed one key
# plus one identity and assert the row reads 2.
#
# The count is the only "2" on the About screen: every other stat is 0
# on a freshly wiped sandbox, so the assertion fails if the row ever
# drops one of the two lists again.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click "Keychain"
expect "Generate key"
click "Generate key"
expect "Algorithm"
click (988, 137)
type "e2e-key"
click "Generate"
settle 800
expect "Key generated and saved to the vault"
click "Done"
settle 300
expect "e2e-key"
# The split "+ ADD" button only exists once the keychain is non-empty;
# its dropdown carries the identity entry. Screenshot first so the
# menu anchors on the freshly drawn button bounds.
screenshot vault-stats-keychain-keys
click (1155, 120)
expect "New Identity"
click "New Identity"
expect "Save Identity"
click (960, 134)
type "e2e-identity"
click (960, 213)
type "root"
click "Save Identity"
settle 300
expect "e2e-identity"
click (1175, 64)
expect "About"
click "About"
expect "Vault Statistics"
expect "Keychain"
expect "2"
screenshot vault-stats-keychain-about
