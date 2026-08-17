viewport: 1200x750
mode: Zen
-----
# Middle-click pastes the last selection. On X11 / Wayland that round
# trips through the system PRIMARY buffer (the widget publishes the
# selection there and the host reads it back); everywhere else it comes
# from the pane's own remembered selection. Either way the same text
# has to land on the command line, which is what this asserts, so the
# test stays platform-independent.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click (92, 20)
settle
click "Local Shell"
timeout 500
wait 4000
# Filling the screen with the same line keeps the pixel selection
# below independent of how many rows the shell drew before it: a CI
# runner's prompt is a different height from a dev box's, and an
# empty selection does not clear the clipboard, so a mis-aimed grab
# reports the PREVIOUS clipboard value instead of saying it missed.
type "clear; yes HELLO_PRIMARY | head -20"
type enter
wait 4000

# Select the echoed word. Copy-on-select puts it on the clipboard, which
# is the assertable half of the gesture.
press (8, 123)
move (60, 123)
move (118, 123)
release (118, 123)
settle
clipboard is "HELLO_PRIMARY"

# Middle-click pastes it back onto the (empty) prompt. The terminal is a
# canvas, so the pasted text is only visible in the screenshot.
click middle (400, 300)
wait 1200
screenshot primary-middle-click-paste
