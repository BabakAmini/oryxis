# Community themes

Terminal and UI themes contributed by Oryxis users, browsable at
**<https://oryxis.app/themes>**.

A theme here is exactly the file Oryxis exports, plus three lines saying
where it came from. There is no new format to learn and nothing to build.

## Adding one

The whole path stays in the browser:

1. In Oryxis, build the theme you want (Settings > Terminal > Themes, or
   Settings > Interface > App theme), then use its **Export** action.
2. Open the file and add `author`, `source` and `license` next to `name`
   (see the fields below).
3. On GitHub, go to `themes/terminal/` (or `themes/ui/`), press
   **Add file > Create new file**, name it `<slug>.json`, paste, and
   **Propose new file**. GitHub forks and branches for you.

`index.json` is regenerated automatically when the pull request merges,
so you never edit it.

The slug is the file name: lower case, words joined by `-`
(`tokyo-night-storm.json`). It has to be unique within its directory.

## The fields

A terminal theme is a Windows Terminal colour scheme, which is what
Oryxis exports and imports:

```json
{
  "name": "Night Owl",
  "author": "Sarah Drasner",
  "source": "https://github.com/sdras/night-owl-vscode-theme",
  "license": "MIT",

  "background": "#011627",
  "foreground": "#d6deeb",
  "cursorColor": "#80a4c2",

  "black": "#011627",       "brightBlack": "#575656",
  "red": "#ef5350",         "brightRed": "#ef5350",
  "green": "#22da6e",       "brightGreen": "#22da6e",
  "yellow": "#c5e478",      "brightYellow": "#ffeb95",
  "blue": "#82aaff",        "brightBlue": "#82aaff",
  "purple": "#c792ea",      "brightPurple": "#c792ea",
  "cyan": "#21c7a8",        "brightCyan": "#7fdbca",
  "white": "#ffffff",       "brightWhite": "#ffffff"
}
```

A UI theme is the Oryxis envelope, again straight from Export:

```json
{
  "oryxis_ui_theme": 1,
  "name": "My Theme",
  "author": "you",
  "source": "https://example.com/optional",
  "license": "MIT",
  "colors": { "bg_primary": "#0a0b0f", "...": "..." }
}
```

`author` and `license` are required; `source` is optional but expected
whenever the palette is a port of someone else's design. `author` is how
you want to be credited, not necessarily a GitHub handle.

For a theme you designed yourself, `license` is your call: put an SPDX
id (`MIT`, `CC0-1.0`, ...) if you have a preference, or `Unspecified` if
you would rather not decide yet. `Unspecified` is honest and it lists;
what it does not do is tell anyone what they may do with your palette
elsewhere, so an id is friendlier to whoever finds it later.

Oryxis ignores the three attribution keys when importing, so a file from
this directory pastes straight into Settings > Import unchanged.

## What gets accepted

**Anything that parses.** A theme is somebody's taste, and refusing one
over a contrast measurement would be arrogant.

What the generator does instead is measure every submission against the
same floor the built-in palettes hold (foreground on background at
4.0:1, cursor on background at 2.0:1) and set a `low_contrast` flag in
the index when it falls short. The site shows a quiet badge on those
cards, so the choice stays with whoever installs it.

Two things are refused, and neither is about taste: a file that is not
valid JSON or is missing a required colour (it would fail to import
anyway), and a palette identical to one already here under a different
name.

## Porting someone else's theme

Take the values from the **original** source, not from another port, and
keep them faithful even where you would have picked differently. A port
that quietly "fixes" its source is no longer that theme.

Say where it came from in `source` and carry its `license`. If the
original has no license at all, it is not ours to redistribute.
