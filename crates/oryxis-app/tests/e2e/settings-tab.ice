viewport: 1400x1000
mode: Zen
-----
# Settings opens as a tab in the strip (issue #120): one instance, it
# survives switching away, and closing it leaves the surface.
click "Skip"
click "Continue without password"
settle
# Through the burger menu rather than the toolbar gear, so the flow
# doesn't depend on the gear's pixel position at this viewport width.
click (19, 20)
settle
click "Settings"
settle
expect "Interface"
# Back to Home. The only "Settings" text on the vault surface is the
# strip chip, so the click below both proves the tab is there and
# returns to it.
click (57, 20)
settle
expect "Create host"
click "Settings"
settle
expect "Interface"
# Reopening from the menu focuses the same tab instead of minting a
# second one; if it ever forked, the chip after it would shift right and
# the close click below would miss.
click (19, 20)
settle
click "Settings"
settle
expect "Interface"
# The chip's X. It sits in the LEADING slot (the badge's place) under the
# default close-button-side, exactly like a session tab, and shows because
# the tab is active. Closing it drops the tab AND leaves Settings, because
# a close that left you staring at the screen you just closed would be a
# dead end.
click (96, 20)
settle
expect "Create host"
