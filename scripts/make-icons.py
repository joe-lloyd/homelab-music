#!/usr/bin/env python3
"""Rasterise the app icon from the shared UI's icon.svg.

The web app, the phone app and this app all use the same record-shaped mark.
Rather than keep a second, hand-drawn copy that drifts, this reads the circles
straight out of ui/public/icon.svg and redraws them at the sizes Tauri bundles.

The SVG is four concentric shapes on a 24x24 viewBox, so parsing it properly
would be more machinery than the job needs -- but the values are read from the
file, not hardcoded here, so a colour change in the UI package still lands.

    python scripts/make-icons.py

Regenerate whenever ui/ is bumped and the icon changed. macOS .icns is not
produced here: it needs a Mac (or `cargo tauri icon`), and CI makes it there.
"""

import pathlib
import re
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
    print(f"\nWrote {len(PNG_SIZES) + 1} files to {OUT}")
    print("macOS .icns is generated on the Mac runner; see .github/workflows.")


if __name__ == "__main__":
    main()
