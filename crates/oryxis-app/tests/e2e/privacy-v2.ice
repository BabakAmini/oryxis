viewport: 1240x1500
mode: Zen
-----
# Issue #78 Privacy Mode v2: mask lists, per-class gates, label
# redaction and the session-override hotkey. Canvas masking is
# validated via the saved screenshots (expects can't see the terminal
# canvas); the expects keep the flow honest. Starts from first-run:
# the batch runner wipes the sandbox before every test. The tall
# viewport keeps the host editor's Username field above the fold.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"

# Save a host whose label embeds the address (the block-5 label leak)
# and whose username joins the privacy terms (block 4).
click "Type IP or Hostname"
type "webprod01.acme-corp.com"
click "Continue"
settle 500
type "webprod01.acme-corp.com (10.0.4.7)"
click "Username"
type "koobs"
click "Save"
settle 500

# Enable Privacy Mode; the v2 rows appear under the master toggle:
# four per-class gates (block 1) and the two mask lists (block 4).
click (1215, 64)
expect "Security & Privacy"
click "Security & Privacy"
expect "Privacy mode"
click (1181, 413)
settle 800
expect "Mask public IP addresses"
expect "Mask private and loopback IPs"
expect "Mask usernames"
expect "Mask saved hostnames"
expect "Always mask these words"
expect "Never mask these usernames"

# Host card: the label renders redacted (screenshot-validated).
click (57, 20)
settle 500
screenshot privacy-v2-card

# Terminal: saved username masks (koobs), the seeded never-mask list
# keeps root readable, the saved hostname masks, private IP masks.
# A live PTY never quiesces, so drop the per-instruction timeout.
timeout 500
type ctrl+k
settle 500
expect "Local Shell"
click "Local Shell"
expect "● bash (default), connected"
settle 800
type "echo owner koobs koobs; echo owner root root"
type enter
type "echo host webprod01.acme-corp.com ip 192.168.0.4"
type enter
settle 800
screenshot privacy-v2-masked

# Session override (block 2): Ctrl+Shift+M forces the mode off (the
# toast announces it, but Zen-mode settling can fast-forward its 6 s
# clear timer, so the stable assertion is the status-bar chip that an
# armed override keeps visible; the screenshots carry the unmasked /
# remasked terminal proof).
type ctrl+shift+m
settle 400
expect "Privacy"
screenshot privacy-v2-override-off
type ctrl+shift+m
settle 400
expect "Privacy"
expect "● bash (default), connected"
screenshot privacy-v2-override-back
