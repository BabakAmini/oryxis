viewport: 1200x750
mode: Zen
-----
# Every clipboard access in the app goes through the iced runtime, which
# serves one at a time on its own thread. Two concurrent Win32 clipboard
# opens in one process are FATAL (STATUS_HEAP_CORRUPTION inside
# user32!GetClipboardData, no panic, no log): that is the 2026-07-29 field
# crash, Ctrl+V in the SFTP path bar. This test walks the three paths that
# used to bypass the runtime, so a regression to a direct `arboard` call
# shows up as a failing assert here.
#
# The emulated clipboard is the oracle: since the app writes through the
# runtime, `clipboard is "..."` sees the app's own copies.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"

# 1. Paste into the SFTP path bar (the crashing gesture). The path input
#    is a text_input, so the runtime performs the read; the value is not
#    text-selectable, hence the screenshot.
click (91, 20)
expect "SFTP"
click "SFTP"
settle
expect "Select a host"
click (785, 166)
settle
clipboard "/tmp"
click (300, 105)
settle
click (300, 108)
type ctrl+v
settle
screenshot sftp-path-paste

# 2. Terminal copy-on-select: the widget queues the copy and the app
#    performs it through the runtime, so it lands in the emulated
#    clipboard (before the fix it went straight to the system clipboard).
click (333, 20)
expect "Local Shell"
click "Local Shell"
timeout 500
wait 2500
type "echo HELLO_CLIP"
type enter
wait 1500
press (8, 123)
move (60, 123)
move (95, 123)
release (95, 123)
settle
clipboard is "HELLO_CLIP"

# 3. Ctrl+Shift+V: the paste read is a Task now, and the text comes back
#    as TerminalPasteResolved before reaching the PTY.
clipboard "echo PASTED_OK"
type ctrl+shift+v
wait 1500
screenshot terminal-paste
