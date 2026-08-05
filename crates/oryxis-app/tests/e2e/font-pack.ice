viewport: 1400x2200
mode: Zen
-----
# The terminal font pack (#109): the picker's download affordance. The
# hint line under the font picker names every pack family not yet
# requested this session, in registry order; on a fresh sandbox that is
# the whole catalog. Its exact text pins the registry family names to
# the picker wiring: a renamed family (the names come from inside the
# pinned TTFs) or a broken PACK_FONTS -> view path changes this string.
#
# The download itself is deliberately not exercised here: the CI batch
# must not depend on the network. The download/verify/cache pipeline is
# the same code as the CJK fonts, and the pins are unit-tested
# (fonts::download_pins_are_well_formed).
click "Skip"
click "Continue without password"
settle
click (19, 20)
settle
click "Settings"
settle
click "Terminal Settings"
settle
expect "Terminal Font"
expect "Available to download (fetched when selected): JetBrainsMono NF, CaskaydiaCove NF, FiraCode Nerd Font, Hack Nerd Font, MesloLGS Nerd Font, RobotoMono Nerd Font, UbuntuMono Nerd Font, Iosevka NF"
