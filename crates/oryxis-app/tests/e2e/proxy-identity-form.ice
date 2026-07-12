viewport: 1200x750
mode: Zen
-----
# D1 unified form chrome on the reference editor: empty submit shows
# the standard inline error above the Cancel/Save footer, a valid fill
# saves and lands the card in the list.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
click "Proxies"
expect "New Proxy"
click "New Proxy"
expect "Add"
click "Add"
expect "Label is required"
click (988, 137)
type "corp-bastion"
click (988, 282)
type "proxy.corp.local"
click "Add"
settle 500
expect "corp-bastion"
