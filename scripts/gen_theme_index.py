#!/usr/bin/env python3
"""Regenerate themes/index.json from the community theme directory.

Run from anywhere:

    python3 scripts/gen_theme_index.py          # write
    python3 scripts/gen_theme_index.py --check  # fail if stale, write nothing

The index inlines every palette. That is what makes the gallery page one
request instead of one-per-theme, and it is affordable only because a
palette is eighteen short strings; if a future theme kind carries
anything bulkier, this decision is the one to revisit.

CONTRAST FLOOR: the two thresholds below are the SAME numbers the
built-in palettes are held to, in
`crates/oryxis-terminal/src/colors.rs` (`text_is_readable_in_every_builtin_palette`
and `the_cursor_is_visible_in_every_builtin_palette`). They are
duplicated across a language boundary on purpose, since a Rust generator
would need those test-only helpers made public for a script that runs
once per pull request. If you move one, move the other: that file names
this script for the same reason.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
THEMES = ROOT / "themes"
INDEX = THEMES / "index.json"

# See CONTRAST FLOOR above before changing either of these.
MIN_TEXT_CONTRAST = 4.0
MIN_CURSOR_CONTRAST = 2.0

HEX = re.compile(r"^#[0-9a-fA-F]{6}$")

# Windows Terminal names magenta "purple"; Oryxis reads the same keys.
ANSI_KEYS = [
    "black", "red", "green", "yellow", "blue", "purple", "cyan", "white",
    "brightBlack", "brightRed", "brightGreen", "brightYellow",
    "brightBlue", "brightPurple", "brightCyan", "brightWhite",
]
TERMINAL_KEYS = ["background", "foreground", "cursorColor"] + ANSI_KEYS

# The 21 named colours of the Oryxis UI envelope, in export order. Must
# stay in step with `UI_COLOR_KEYS` in
# `crates/oryxis-app/src/theme_export.rs`, which is the file the app
# writes and reads.
UI_KEYS = [
    "bg_primary", "bg_sidebar", "bg_surface", "bg_hover", "bg_selected",
    "text_primary", "text_secondary", "text_muted", "accent", "accent_hover",
    "success", "warning", "error", "terminal_bg", "terminal_fg",
    "terminal_cursor", "border", "border_focus", "button_bg",
    "button_bg_hover", "button_text",
]


class ThemeError(Exception):
    """A submission that cannot be indexed, reported with its path."""


def luminance(hex_color: str) -> float:
    """WCAG relative luminance of an `#rrggbb` string."""

    def channel(v: float) -> float:
        return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4

    r, g, b = (int(hex_color[i:i + 2], 16) / 255.0 for i in (1, 3, 5))
    return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)


def contrast(a: str, b: str) -> float:
    x, y = luminance(a), luminance(b)
    return (max(x, y) + 0.05) / (min(x, y) + 0.05)


def norm_hex(value: object, field: str) -> str:
    """Accept `#rgb` and `#rrggbb` in any case, emit lower `#rrggbb`.

    The app's importer is this lenient, so the index has to be too, or a
    file that installs fine would fail to list.
    """
    if not isinstance(value, str):
        raise ThemeError(f"{field}: expected a colour string")
    v = value.strip()
    if len(v) == 4 and v.startswith("#") and all(c in "0123456789abcdefABCDEF" for c in v[1:]):
        v = "#" + "".join(c * 2 for c in v[1:])
    if not HEX.match(v):
        raise ThemeError(f"{field}: {value!r} is not a #rrggbb colour")
    return v.lower()


def read_meta(data: dict, path: pathlib.Path) -> dict:
    name = data.get("name")
    if not isinstance(name, str) or not name.strip():
        raise ThemeError("name: missing")
    author = data.get("author")
    if not isinstance(author, str) or not author.strip():
        raise ThemeError("author: missing (say how you want to be credited)")
    license_ = data.get("license")
    if not isinstance(license_, str) or not license_.strip():
        raise ThemeError("license: missing")
    entry = {
        "slug": path.stem,
        "name": name.strip(),
        "author": author.strip(),
        "license": license_.strip(),
    }
    source = data.get("source")
    if isinstance(source, str) and source.strip():
        entry["source"] = source.strip()
    return entry


def load_terminal(path: pathlib.Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    entry = read_meta(data, path)
    colors = {k: norm_hex(data.get(k), k) for k in TERMINAL_KEYS}
    entry["kind"] = "terminal"
    entry["colors"] = colors
    # Measured, never enforced: the flag is the whole moderation policy
    # for readability (themes/README.md), so a palette that misses the
    # floor still ships, labelled.
    entry["low_contrast"] = (
        contrast(colors["foreground"], colors["background"]) < MIN_TEXT_CONTRAST
        or contrast(colors["cursorColor"], colors["background"]) < MIN_CURSOR_CONTRAST
    )
    return entry


def load_ui(path: pathlib.Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("oryxis_ui_theme") != 1:
        raise ThemeError('oryxis_ui_theme: expected 1 (is this a terminal theme?)')
    entry = read_meta(data, path)
    raw = data.get("colors")
    if not isinstance(raw, dict):
        raise ThemeError("colors: missing")
    colors = {k: norm_hex(raw.get(k), f"colors.{k}") for k in UI_KEYS}
    entry["kind"] = "ui"
    entry["colors"] = colors
    entry["low_contrast"] = (
        contrast(colors["text_primary"], colors["bg_primary"]) < MIN_TEXT_CONTRAST
    )
    return entry


def collect() -> list[dict]:
    entries: list[dict] = []
    errors: list[str] = []
    for kind, loader in (("terminal", load_terminal), ("ui", load_ui)):
        directory = THEMES / kind
        if not directory.is_dir():
            continue
        for path in sorted(directory.glob("*.json")):
            try:
                entries.append(loader(path))
            except ThemeError as e:
                errors.append(f"{path.relative_to(ROOT)}: {e}")
            except json.JSONDecodeError as e:
                errors.append(f"{path.relative_to(ROOT)}: invalid JSON: {e}")
    # A duplicate palette under a second name is the one submission worth
    # refusing on content: it is the same theme twice in the list.
    seen: dict[str, str] = {}
    for entry in entries:
        key = entry["kind"] + json.dumps(entry["colors"], sort_keys=True)
        if key in seen:
            errors.append(
                f"themes/{entry['kind']}/{entry['slug']}.json: identical colours "
                f"to {seen[key]}"
            )
        else:
            seen[key] = f"{entry['slug']}.json"
    if errors:
        raise SystemExit("theme index: " + "\n theme index: ".join(errors))
    entries.sort(key=lambda e: (e["kind"], e["name"].lower()))
    return entries


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit non-zero if the committed index is out of date",
    )
    args = parser.parse_args()

    payload = {"version": 1, "themes": collect()}
    rendered = json.dumps(payload, indent=2, ensure_ascii=False) + "\n"

    if args.check:
        current = INDEX.read_text(encoding="utf-8") if INDEX.exists() else ""
        if current != rendered:
            print(
                "themes/index.json is out of date; run "
                "`python3 scripts/gen_theme_index.py`",
                file=sys.stderr,
            )
            return 1
        print(f"themes/index.json is current ({len(payload['themes'])} themes)")
        return 0

    INDEX.write_text(rendered, encoding="utf-8")
    print(f"wrote {INDEX.relative_to(ROOT)} ({len(payload['themes'])} themes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
