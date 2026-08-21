# Icons

Generated from `../app_icon.png` (1024×1024, alpha).

| File | Platform | Shape |
|---|---|---|
| `icon.icns` | macOS | rounded square, padded |
| `icon.ico` | Windows | full bleed — 16, 24, 32, 48, 64, 128, 256 |
| `linux/<size>/ecash-splitter.png` | Linux | full bleed — 16…512, freedesktop hicolor layout |
| `icon-macos-1024.png` | master | the macOS shape, for regeneration |
| `icon-square-1024.png` | master | the source square, unchanged |

**Why macOS gets a different shape.** macOS does not auto-mask app icons the way iOS does, so a
full-bleed square renders as a square sitting among rounded ones in the Dock. Apple's grid for a
1024pt icon is an **824×824 rounded square, corner radius 185**, centred on a transparent canvas
— that padding is what makes it sit correctly next to other apps. Windows and Linux want full
bleed and draw their own affordances.

## Regenerating

Needs Pillow and macOS's built-in `iconutil`:

```sh
python3 -m venv /tmp/iconenv && /tmp/iconenv/bin/pip install Pillow
/tmp/iconenv/bin/python assets/generate.py
iconutil -c icns /tmp/ecx.iconset -o assets/icon.icns
```

`iconutil` is macOS-only, so regenerating the `.icns` needs a Mac. The `.ico` and Linux PNGs do
not. (Note `rcodesign` lets us *sign* from Linux — see `../docs/signing-and-notarization.md` —
but the icon still has to be built on a Mac or committed, which is why these are committed.)
