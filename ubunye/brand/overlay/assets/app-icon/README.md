# @NAME@ app icon

The icon of @NAME@ (@SUITE@, @VENDOR@). It replaces upstream FilmCraft's icon at the
same paths, so upstream's build and packaging scripts pick it up unchanged.

## Source

- Master artwork: `ubunye/brand/assets/icon.svg` (512-unit tile, `rx=112`), adapted from the
  ThabangVision logo. The letters are outlines (from DM Serif Display, SIL OFL 1.1), so no font
  is needed to render it.
- Licence: [LICENSE.txt](LICENSE.txt).

## Files

| File | What |
|---|---|
| `filmcraft.svg`, `filmcraft-small.svg` | copies of the master artwork |
| `filmcraft-1024.png` | 1024 px render |
| `filmcraft-macos-512.png` | runtime Dock icon on macOS (Apple grid: 824 of 1024 px, transparent margin) |
| `filmcraft.icns` | macOS bundle icon |
| `filmcraft.ico` | Windows icon, 16 to 256 px (embedded in `filmcraft.exe` by `apps/filmcraft/build.rs`) |
| `hicolor/<size>/apps/@APP_ID@.png`, `hicolor/scalable/…svg` | Linux icon theme, used by `packaging/linux/@APP_ID@.desktop` |

The web app's `apps/filmcraft-web/web/favicon.png` (128 px) and the mark in the About dialog and
Home screen (`crates/ui-egui/src/brand.rs`, the 256 px PNG) come from the same artwork.

## Regenerate

1. Replace `ubunye/brand/assets/icon.svg`.
2. Run `python -I ubunye/brand/apply.py`. It sees the artwork changed, renders every size into
   `ubunye/brand/generated/` with `ubunye/brand/make_icons.py` (needs `resvg` and Pillow), copies
   them here and rewrites the `.attribution` sidecars.
