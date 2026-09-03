#!/usr/bin/env python3
"""Rasterise the app icon from the shared UI's icon.svg.

The web app, the phone app and this app all use the same record-shaped mark.
Rather than keep a second, hand-drawn copy that drifts, this reads the circles
straight out of ui/public/icon.svg and redraws them at the sizes Tauri bundles.

The SVG is four concentric shapes on a 24x24 viewBox, so parsing it properly
would be more machinery than the job needs -- but the values are read from the
file, not hardcoded here, so a colour change in the UI package still lands.

    python scripts/make-icons.py

Regenerate whenever ui/ is bumped and the icon changed. This includes the macOS
.icns, which is written directly rather than shelled out to `iconutil` -- that
only exists on a Mac, and needing a Mac to produce an icon would mean the Mac
bundle could never be built anywhere else, including CI's own checkout.
"""

import io
import pathlib
import re
import struct
import sys

from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
SVG = ROOT / "ui" / "public" / "icon.svg"
OUT = ROOT / "src-tauri" / "icons"

VIEWBOX = 24.0
# Tauri's expected names; icon.png is the generic source, the rest are what the
# Windows and Linux bundlers look for.
PNG_SIZES = {
    "32x32.png": 32,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": 1024,
}
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
# The .icns entries Tauri's bundler looks for, as (OSType, pixel size). All of
# these accept a PNG payload, which is what makes writing the container here
# rather than via iconutil practical.
ICNS_ENTRIES = [
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
    (b"ic14", 512),
]
# Supersampling factor. Circles this small alias badly without it.
SS = 8


def parse(svg_text):
    """Pull the background rect and the concentric circles out of the SVG."""
    rect = re.search(r'<rect[^>]*fill="(#[0-9a-fA-F]+)"', svg_text)
    if not rect:
        sys.exit(f"{SVG}: no background <rect fill=...> found")
    circles = [
        (float(cx), float(cy), float(r), fill)
        for cx, cy, r, fill in re.findall(
            r'<circle[^>]*cx="([\d.]+)"[^>]*cy="([\d.]+)"[^>]*r="([\d.]+)"[^>]*fill="(#[0-9a-fA-F]+)"',
            svg_text,
        )
    ]
    if not circles:
        sys.exit(f"{SVG}: no <circle> elements found")
    return rect.group(1), circles


def render(size, background, circles):
    """Draw one square icon at `size` px, supersampled then downscaled."""
    big = size * SS
    scale = big / VIEWBOX
    img = Image.new("RGBA", (big, big), background)
    draw = ImageDraw.Draw(img)
    for cx, cy, r, fill in circles:
        x, y, rr = cx * scale, cy * scale, r * scale
        draw.ellipse([x - rr, y - rr, x + rr, y + rr], fill=fill)
    return img.resize((size, size), Image.LANCZOS)


def write_icns(path, background, circles):
    """Write a macOS .icns without iconutil.

    The format is plainer than its reputation: the magic 'icns', the total
    file length, then one record per icon of OSType, record length (counting
    its own 8-byte header) and payload. Every OSType used here takes a PNG
    payload directly, so there is no Apple-specific packing to reproduce.
    Both integers are big-endian.
    """
    records = bytearray()
    for ostype, size in ICNS_ENTRIES:
        buf = io.BytesIO()
        render(size, background, circles).save(buf, format="PNG")
        payload = buf.getvalue()
        records += struct.pack(">4sI", ostype, len(payload) + 8) + payload

    path.write_bytes(b"icns" + struct.pack(">I", len(records) + 8) + bytes(records))


def main():
    if not SVG.exists():
        sys.exit(f"{SVG} is missing -- is the ui/ submodule initialised?")
    background, circles = parse(SVG.read_text(encoding="utf-8"))
    OUT.mkdir(parents=True, exist_ok=True)

    for name, size in PNG_SIZES.items():
        render(size, background, circles).save(OUT / name)
        print(f"  {name} ({size}px)")

    render(256, background, circles).save(
        OUT / "icon.ico", sizes=[(s, s) for s in ICO_SIZES]
    )
    print(f"  icon.ico ({', '.join(str(s) for s in ICO_SIZES)})")

    write_icns(OUT / "icon.icns", background, circles)
    print(f"  icon.icns ({len(ICNS_ENTRIES)} entries)")

    print(f"\nWrote {len(PNG_SIZES) + 2} files to {OUT}")


if __name__ == "__main__":
    main()
