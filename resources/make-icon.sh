#!/usr/bin/env bash
# Regenerates resources/logo.ico from logo.svg.
#
# Windows picks the ICO entry whose size matches what it is about to draw
# (16 in the title bar and Explorer lists, 32 in Alt+Tab, 48 in large-icon
# views, 256 in the taskbar and jumbo views). With a single 256 entry the
# system downscales at draw time with GDI, which is visibly worse than
# rendering each size from the vector, so every size gets its own entry.
#
# Requires rsvg-convert (librsvg2-bin) and Pillow.
set -euo pipefail

cd "$(dirname "$0")"

sizes=(16 24 32 48 64 128 256)
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

for size in "${sizes[@]}"; do
    rsvg-convert -w "$size" -h "$size" -o "$tmp/$size.png" logo.svg
done

python3 - "$tmp" "${sizes[@]}" <<'PY'
import struct
import sys

from PIL import Image

tmp, *sizes = sys.argv[1:]
sizes = [int(size) for size in sizes]


def dib(image):
    """The classic ICO payload: a bottom-up BGRA bitmap whose header claims
    double the height, followed by the 1bpp AND mask (all zero, since the
    alpha channel is what Windows composites with on every version we
    support). Written by hand because Pillow encodes every entry as PNG,
    and PNG entries below 256 are only understood from Vista on: makensis
    and older shells still expect a DIB there."""
    width, height = image.size
    header = struct.pack(
        "<IiiHHIIiiII", 40, width, height * 2, 1, 32, 0, 0, 0, 0, 0, 0
    )
    rows = [
        b"".join(
            struct.pack("BBBB", b, g, r, a)
            for r, g, b, a in (
                image.getpixel((x, y)) for x in range(width)
            )
        )
        for y in reversed(range(height))
    ]
    mask_stride = ((width + 31) // 32) * 4
    return header + b"".join(rows) + bytes(mask_stride * height)


def png(image):
    from io import BytesIO

    buffer = BytesIO()
    image.save(buffer, format="PNG")
    return buffer.getvalue()


images = [Image.open(f"{tmp}/{size}.png").convert("RGBA") for size in sizes]

# The big entries ride as PNG (the Vista convention that keeps the file
# small: an uncompressed 128 DIB alone is bigger than every other entry
# combined). The small ones, the sizes legacy tooling actually reads,
# stay DIBs.
payloads = [png(image) if size >= 128 else dib(image) for size, image in zip(sizes, images)]

offset = 6 + 16 * len(sizes)
directory = b""

for size, payload in zip(sizes, payloads):
    directory += struct.pack(
        "<BBBBHHII",
        size % 256,
        size % 256,
        0,
        0,
        1,
        32,
        len(payload),
        offset,
    )
    offset += len(payload)

with open("logo.ico", "wb") as ico:
    ico.write(struct.pack("<HHH", 0, 1, len(sizes)))
    ico.write(directory)

    for payload in payloads:
        ico.write(payload)
PY

echo "logo.ico: ${sizes[*]}"
