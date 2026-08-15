viewport: 1200x750
mode: Zen
-----
# Issue #168: typing an address on the numeric keypad with NumLock off
# (or with Windows' legacy Shift+digit conversion) delivers ArrowUp /
# ArrowDown mid-word. The vault-area keynav router used to claim those
# from ring idle unconditionally: it blurred the focused quick-host
# field, ringed a content row, scrolled the list, and every keystroke
# after that went nowhere. The router now resolves iced's real focus
# first (`vault_ring_idle_resolve`) and leaves the key with any
# focused text input that is not the view's search field.
#
# The emulated clipboard is the value oracle (text selectors never see
# text_input values): if the arrows steal focus, ctrl+a / ctrl+c reach
# no widget and the seeded sentinel survives, failing the assert. The
# arrows are sent while the field is still empty on purpose: the
# fork's text_input moves the caret on Up/Down, so arrows between
# typed chunks would make the expected string caret-order dependent.
settle 250
click "Skip"
click "Continue without password"
settle 250
expect "Create host"
clipboard "sentinel"
click "Type IP or Hostname"
type up
type down
type "root@192.0.2.7"
type ctrl+a
type ctrl+c
clipboard is "root@192.0.2.7"
