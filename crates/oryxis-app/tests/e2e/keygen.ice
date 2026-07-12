viewport: 1200x750
mode: Zen
-----
# B4: generate an Ed25519 key from the keychain empty state. The
# result screen confirms the vault save and shows the public half;
# the new card lands in the keys list.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click "Keychain"
expect "Generate key"
click "Generate key"
expect "Algorithm"
click "Generate"
expect "Label is required"
click (988, 137)
type "e2e-key"
click "Generate"
settle 800
expect "Key generated and saved to the vault"
expect "Copy public key"
click "Done"
settle 300
expect "e2e-key"
