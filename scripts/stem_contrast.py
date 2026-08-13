#!/usr/bin/env python3
"""Measure how solid the terminal's glyph stems actually are.

    cargo build -q -p oryxis-app --features harness
    python3 scripts/stem_contrast.py

Prints, for one row of 41 identical `l` glyphs rendered by the REAL app
through the headless harness, the peak coverage of each vertical stem
against the pane's foreground colour (1.00 = the pixel reached the full
foreground, i.e. the stem landed on the pixel grid) and the total ink
per stem in pixels.

This is the measurement behind the "Glyph rendering" section of
CLAUDE.md, and the reason the terminal ships a stroke dilation instead
of an alacritty-style integer cell. Re-run it before changing anything
in that area; the numbers to beat, at 14 px on the bundled
SauceCodePro Nerd Font, scale 1:

    thickness Off      mean peak 0.85   stem ink 0.99 px
    thickness Medium   mean peak 0.97   stem ink 1.51 px

A run always starts on a wiped sandbox, so it measures whatever the
DEFAULT thickness draws. For the other end, set Settings > Terminal >
Text Thickness by hand, take a `screenshot` through the harness, and
point this at the file with `--shot`.

Read one stem row at a time (the printed spread mixes the stem with the
serifs above and below it) and the Off case shows exactly four values,
which is not noise: a merged run is laid out by the shaper at the font's
fractional advance, and cosmic-text bins the subpixel phase into
quarters, so the same character renders at a different weight depending
on which column it fell in.

Needs Pillow (`pip install pillow`) and a free harness port.
"""

from __future__ import annotations

import argparse
import os
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "target" / "debug" / "oryxis"
# One row of 41 `l`: a bare vertical stem, repeated far enough to walk
# every subpixel phase several times over.
SAMPLE = 41


def ctl(port: int, script: str) -> str:
    """Feed a batch of harness commands to the running daemon."""
    out = subprocess.run(
        # `--port` may only PRECEDE `--harness-ctl`: the flag after it
        # is parsed as the command to run.
        [str(BIN), "--port", str(port), "--harness-ctl"],
        input=script,
        capture_output=True,
        text=True,
    )
    return out.stdout


def drive(port: int) -> Path:
    """Boot to a local shell, print the sample, return the screenshot."""
    ctl(
        port,
        "\n".join(
            [
                "reset wipe",
                'click "Skip"',
                'click "Continue without password"',
                "settle",
                "click (19, 20)",
                "settle",
                'click "Local Shell"',
                "settle",
                # A live PTY never quiesces, so every later instruction
                # would otherwise burn the full timeout.
                "timeout 500",
                "wait 1500",
                f'type "printf \\"{"l" * SAMPLE}\\\\n\\""',
                "type enter",
                "wait 1200",
                "screenshot stem-contrast",
            ]
        )
        + "\n",
    )
    home = os.environ.get("ORYXIS_HARNESS_HOME", "/tmp/oryxis-harness")
    return Path(home) / "shots" / "stem-contrast.png"


def measure(shot: Path) -> None:
    from PIL import Image

    im = Image.open(shot).convert("RGB")
    px = im.load()
    width, height = im.size

    # The pane's background is the most common colour in the grid area;
    # the foreground is the brightest pixel the sample row reaches.
    counts: dict[tuple[int, int, int], int] = {}
    for y in range(90, min(height, 400)):
        for x in range(0, min(width, 700)):
            counts[px[x, y]] = counts.get(px[x, y], 0) + 1
    bg = max(counts.items(), key=lambda kv: kv[1])[0]

    def channel(p: tuple[int, int, int]) -> int:
        # Compare on whichever channel separates fore from back most.
        return p[1]

    rows: list[tuple[int, list[float], list[float]]] = []
    for y in range(90, min(height, 400)):
        row = [px[x, y] for x in range(0, min(width, 700))]
        lit = [p for p in row if abs(channel(p) - channel(bg)) > 40]
        if len(lit) < SAMPLE:
            continue
        fg = max(lit, key=channel)
        span = channel(fg) - channel(bg)
        if span <= 0:
            continue

        def cov(p: tuple[int, int, int]) -> float:
            return max(0.0, min(1.0, (channel(p) - channel(bg)) / span))

        xs = [x for x in range(len(row)) if cov(row[x]) > 0.4]
        groups: list[list[int]] = []
        run = [xs[0]]
        for x in xs[1:]:
            if x == run[-1] + 1:
                run.append(x)
            else:
                groups.append(run)
                run = [x]
        groups.append(run)
        if len(groups) < SAMPLE - 1:
            continue
        peaks = [max(cov(row[x]) for x in g) for g in groups]
        ink = [sum(cov(row[x]) for x in g) for g in groups]
        rows.append((y, peaks, ink))

    if not rows:
        sys.exit(f"no row of {SAMPLE} stems found in {shot}")

    # The tallest band of qualifying rows is the stem body; its serifs
    # (top and bottom) carry far more ink and would skew the reading.
    body = sorted(rows, key=lambda r: statistics.mean(r[2]))[: max(1, len(rows) // 2)]
    peaks = [p for _, ps, _ in body for p in ps]
    ink = [i for _, _, inks in body for i in inks]
    distinct = sorted({round(p, 2) for p in peaks})
    print(f"screenshot     {shot}")
    print(f"stems measured {len(peaks)} over {len(body)} rows")
    print(f"peak coverage  min {min(peaks):.2f}  max {max(peaks):.2f}  mean {statistics.mean(peaks):.2f}")
    print(f"distinct peaks {distinct}")
    print(f"stem ink       {statistics.mean(ink):.2f} px")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--port", type=int, default=6799)
    ap.add_argument(
        "--shot",
        type=Path,
        help="skip the harness and measure an existing screenshot",
    )
    args = ap.parse_args()

    if args.shot:
        measure(args.shot)
        return

    if not BIN.exists():
        sys.exit("build it first: cargo build -q -p oryxis-app --features harness")

    log = Path("/tmp/stem-contrast-serve.log")
    with log.open("w") as sink:
        daemon = subprocess.Popen(
            [str(BIN), "--harness-serve", "--port", str(args.port),
             "--viewport", "1240x900"],
            stdout=sink,
            stderr=subprocess.STDOUT,
        )
    try:
        for _ in range(60):
            if "harness listening" in log.read_text(errors="ignore"):
                break
            time.sleep(1)
        else:
            sys.exit(f"harness never came up, see {log}")
        measure(drive(args.port))
    finally:
        ctl(args.port, "quit\n")
        try:
            daemon.wait(timeout=30)
        except subprocess.TimeoutExpired:
            daemon.kill()


if __name__ == "__main__":
    main()
