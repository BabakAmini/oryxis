viewport: 1200x750
mode: Zen
-----
# Every range-valid quad-dot masks under Privacy Mode, including
# version-shaped tokens (the issue #53 exemption was removed
# 2026-07-19: hostile output could print "version <ip>" to keep a real
# address readable; masking a version string accidentally is the
# accepted error). The terminal canvas is invisible to `expect`, so the
# masking itself is validated via the saved screenshot (EVERY quad row
# as block glyphs: the winget table, the pandoc version line, PING and
# RFC1918); the expects keep the flow honest. Starts from first-run:
# the batch runner wipes the sandbox before every test.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"

# Enable Privacy Mode (Settings > Security & Privacy). The toggle is
# addressed by coordinate; the fixed viewport keeps it stable. Row
# moved 410 -> 450 with the themed settings cards (8ca62618).
click (1175, 64)
expect "Security & Privacy"
click "Security & Privacy"
expect "Privacy mode"
click (1140, 450)
settle 800

# Open a local shell; a live PTY never quiesces, so drop the
# per-instruction timeout first.
timeout 500
type ctrl+k
settle 500
expect "Local Shell"
click "Local Shell"
expect "● bash (default), connected"
settle 800

type "echo 'Python 3  Python.3  3.9.0.2  3.13.0  winget'"
type enter
type "echo 'VS Code  XP9K  1.96.0.0  1.96.0.1'"
type enter
type "echo 'pandoc version 3.9.0.2 installed'"
type enter
type "echo 'PING 8.8.8.8 (8.8.8.8) 56(84) bytes of data.'"
type enter
type "echo 'update available at 192.168.1.10'"
type enter
settle 800
screenshot privacy-version-strings
expect "● bash (default), connected"
