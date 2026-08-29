viewport: 1200x750
mode: Zen
-----
# Issue #193: the Cloud panel's "Open Plugins" CTA has to SWITCH to
# Settings, not merely select a section behind the view the user is
# already on. It once sent the bare `ChangeSettingsSection`, which
# assumes Settings is on screen, so the button looked dead: the click
# landed, the section changed, and nothing moved.
#
# Precondition: no cloud-provider plugin installed, which is what the
# wiped sandbox gives and what CI's `-p oryxis-app` build gives. A
# local `cargo build --workspace` leaves the dev plugin binaries next
# to the app, and a dev binary counts as installed, so this file then
# meets the account list instead of the explainer.
click "Skip"
click "Continue without password"
settle
click "Cloud Accounts"
settle
expect "No cloud provider installed"
click "Open Plugins"
settle
# The Settings sidebar proves the view switched; the provider card
# proves it landed on the Plugins section rather than wherever
# Settings was last left.
expect "Features & Plugins"
expect "Amazon Web Services"
