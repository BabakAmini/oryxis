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
# The selection below is a PIXEL range, so it only copies what is
# actually drawn at that row. Tying it to a SINGLE echoed line made
# it depend on where the shell's prompt ended up: a CI runner's
# `runner@fv-az...` prompt is long enough to wrap at this width and
# push the output down a row, and an empty selection does not clear
# the clipboard, so the assert reported the "/tmp" the SFTP step
# left there instead of saying it grabbed the wrong row.
#
# Filling the screen with the SAME line removes the dependency: any
# row in the block yields the same text, so the test survives a
# prompt of any length. `clear` puts the block at a known top.
wait 4000
type "clear; yes HELLO_CLIP | head -20"
type enter
wait 4000
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
