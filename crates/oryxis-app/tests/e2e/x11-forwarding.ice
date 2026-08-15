viewport: 1240x1500
mode: Zen
-----
# X11 forwarding: the per-host toggle lives in the SSH host editor's
# Authentication subgroup, immediately under "Forward SSH Agent" (both
# are channel requests sent before the shell starts, so users look for
# them together).
#
# The viewport is deliberately tall: the Authentication rows sit well
# below the fold at the default height, and the emulator cannot scroll
# a side-panel scrollable with the wheel.
settle 250
click "Skip"
click "Continue without password"
settle 250
expect "Create host"
click "Type IP or Hostname"
type "x11test.example.com"
click "Continue"
settle 250
expect "New Host"
# Two-tier editor: both forwarding rows live in the collapsed
# Authentication section; open it first.
click "Authentication"
settle 300
# Both forwarding rows render, in this order.
expect "Forward SSH Agent"
expect "Forward X11"
screenshot x11-forwarding-row
