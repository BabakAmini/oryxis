viewport: 1200x750
mode: Zen
-----
# Issue #174: the quick-connect line names its own protocol.
#
# The people who asked for this live on gear that speaks Telnet or
# hangs off a console server, and the ad-hoc box was SSH-only: the
# parser refused a URL outright, so `telnet://sw02` produced no card at
# all. This walks the four answers the parser now gives, all of them
# readable as text (the card's title carries the protocol), so nothing
# here depends on pixel positions.
#
# The username is pinned in every target on purpose: an unstated one
# resolves to the OS user, which differs between a developer's machine
# and CI.
expect "Welcome to Oryxis"
click "Skip"
click "Continue without password"
expect "Create host"
# One saved host, so the dashboard leaves its empty state and the
# search box (which doubles as the quick-connect entry) exists.
# A bare hostname (not an IP literal) opens the editor rather than
# offering an ad-hoc dial, which is what makes this a SAVE.
click #empty-quick-host
type "myserver.example.com"
settle 250
type enter
settle 250
expect "New Host"
# The empty-state box fills the ADDRESS; a host still needs its label
# before the vault will take it.
click #editor-label
type "lab-host"
settle 250
click "Save"
settle 250
expect "lab-host"

# 1. No scheme: the protocol is undecided, so the card offers badges
#    and dials SSH until one is picked.
click #search-dashboard
type "root@web01"
expect "Quick connect: root@web01"
expect "SSH"
expect "Telnet"

# 2. A typed scheme decides it, and seeds the conventional port. The
#    badges are gone: the text already answered the question, and a
#    picker that could contradict it would be a second source of truth.
click #search-dashboard
type ctrl+a
type "telnet://root@sw02"
expect "Quick connect (Telnet): root@sw02:23"
# The badges are gone (their SSH chip is the tell; matching on "Telnet"
# would hit the card title, which the assertions read by substring).
absent "SSH"

# 3. A device path is Serial without any scheme at all: `/dev/tty*` is
#    a host under no protocol, so there is nothing to choose between.
click #search-dashboard
type ctrl+a
type "/dev/ttyUSB0"
expect "Quick connect (Serial): /dev/ttyUSB0"

# 4. Raw needs a port and has no conventional one (console servers
#    number their lines per vendor), so a portless raw:// target is
#    refused rather than dialled somewhere arbitrary.
click #search-dashboard
type ctrl+a
type "raw://console"
absent "Quick connect"
click #search-dashboard
type ctrl+a
type "raw://console:2001"
expect "Quick connect (Raw): console:2001"
