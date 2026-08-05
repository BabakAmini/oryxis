viewport: 1240x1500
mode: Zen
-----
# The one Import entry (the add-host catalog feeds both the "+ Host"
# dropdown and this first-run empty state, so the empty state IS the
# cheapest place to prove the entry exists) and the hub it opens.
# The hub lists every supported source: the list is the contract with
# the user, so a format that stops being detected must also stop
# being advertised, and this expect is what forces that pairing.
#
# The pickers themselves are native dialogs the harness can't drive;
# the parsers are covered by unit tests (importers::*), and the
# detection routing by importers::detect::tests.
click "Skip"
click "Continue without password"
settle
expect "Import"
click "Import"
settle
expect "Import hosts"
expect "Oryxis export (.oryxis)"
expect "OpenSSH config (~/.ssh/config)"
expect "PuTTY / KiTTY sessions (.reg export)"
expect "WinSCP sites (WinSCP.ini / .reg export)"
expect "mRemoteNG (confCons.xml)"
expect "MobaXterm (MobaXterm.ini)"
expect "Xshell / SecureCRT / FinalShell (session folder)"
expect "Termius export / CSV (any host table)"
expect "Choose file..."
expect "Choose folder..."
# Cancel closes it without touching the vault.
click "Cancel"
settle
absent "Import hosts"
