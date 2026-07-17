viewport: 1200x750
mode: Zen
-----
# B1 phase 4: the SSH Agent block in Settings > Features & Plugins.
# Enabling the agent reveals the config section with the new opt-in
# "Accept keys from other apps" row; toggling it on restarts the
# runtime and the section stays up (a bind error would surface
# inline). The OpenSSH pipe alias row is Windows-only and never
# renders on this platform. The toggle coordinates are stable under
# the fixed viewport (togglers sit outside the text selector).
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click (1175, 64)
expect "Features & Plugins"
click "Features & Plugins"
expect "SSH Agent"
click (1141, 305)
settle 300
expect "Confirm each use"
expect "Accept keys from other apps"
expect "Agent socket"
click (1141, 458)
settle 300
expect "Accept keys from other apps"
screenshot agent-allow-add-on
