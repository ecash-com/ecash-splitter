#!/usr/bin/env python3
"""Regenerate every icon asset from app_icon.png. Needs Pillow; see README.md."""

import os
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ASSETS = os.path.join(ROOT, "assets")
src = Image.open(os.path.join(ROOT, "app_icon.png")).convert("RGBA")

# --- macOS master ---------------------------------------------------------
# macOS does not auto-mask app icons, so the rounded shape and the surrounding
# padding are baked in. Apple's grid for 1024pt: an 824x824 rounded square,
# corner radius 185, centred on a transparent canvas.
CANVAS, SQUARE, RADIUS = 1024, 824, 185
art = src.resize((SQUARE, SQUARE), Image.LANCZOS)
mask = Image.new("L", (SQUARE, SQUARE), 0)
ImageDraw.Draw(mask).rounded_rectangle((0, 0, SQUARE - 1, SQUARE - 1), RADIUS, fill=255)
art.putalpha(mask)
macos = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
macos.paste(art, ((CANVAS - SQUARE) // 2,) * 2, art)
macos.save(os.path.join(ASSETS, "icon-macos-1024.png"))
src.save(os.path.join(ASSETS, "icon-square-1024.png"))

# --- macOS iconset, for `iconutil -c icns /tmp/ecx.iconset` ---------------
os.makedirs("/tmp/ecx.iconset", exist_ok=True)
for px, name in [
    (16, "icon_16x16"), (32, "icon_16x16@2x"),
    (32, "icon_32x32"), (64, "icon_32x32@2x"),
    (128, "icon_128x128"), (256, "icon_128x128@2x"),
    (256, "icon_256x256"), (512, "icon_256x256@2x"),
    (512, "icon_512x512"), (1024, "icon_512x512@2x"),
]:
    macos.resize((px, px), Image.LANCZOS).save(f"/tmp/ecx.iconset/{name}.png")

# --- Windows and Linux: full bleed, the OS draws its own affordances ------
src.save(os.path.join(ASSETS, "icon.ico"),
         sizes=[(s, s) for s in (16, 24, 32, 48, 64, 128, 256)])
for s in (16, 32, 48, 64, 128, 256, 512):
    d = os.path.join(ASSETS, "linux", f"{s}x{s}")
    os.makedirs(d, exist_ok=True)
    src.resize((s, s), Image.LANCZOS).save(os.path.join(d, "ecash-splitter.png"))

print("done. now: iconutil -c icns /tmp/ecx.iconset -o assets/icon.icns")
