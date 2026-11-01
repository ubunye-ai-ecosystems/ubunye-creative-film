"""Render every app icon file from ubunye/brand/assets/icon.svg into ubunye/brand/generated/.

Mirrors upstream's packaging/icons.sh (same sizes, same macOS grid margin) but writes into our
own folder; `python -I ubunye/brand/apply.py` then copies the files over upstream's icon paths.
Run it only when the artwork changes; the outputs are committed, so builds never need it.

Needs resvg (RESVG env var, else T:/rust/cargo/bin/resvg.exe, else `resvg` on PATH) and Pillow
(for the .ico and .icns). Usage:

    python ubunye/brand/make_icons.py
"""

import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SVG = HERE / "assets" / "icon.svg"
# Simplified icon (fewer, thicker rays, no glow) used at 32 px and below.
SMALL = HERE / "assets" / "icon-small.svg"
OUT = HERE / "generated"
APP_ID = "org.ubunye.creative.film"
HICOLOR = (16, 24, 32, 48, 64, 128, 256, 512)
ICO = (16, 20, 24, 32, 40, 48, 64, 128, 256)


def resvg() -> str:
    for cand in (os.environ.get("RESVG"), "T:/rust/cargo/bin/resvg.exe", shutil.which("resvg")):
        if cand and Path(cand).exists():
            return cand
    sys.exit("error: resvg not found (set RESVG or install it with cargo install resvg)")


def source_hash() -> str:
    h = hashlib.sha256()
    for f in (SVG, SMALL):
        if f.exists():
            h.update(f.read_bytes().replace(b"\r\n", b"\n"))
    return h.hexdigest()


def render(tool: str, svg: Path, size: int, out: Path) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run([tool, "-w", str(size), "-h", str(size), str(svg), str(out)], check=True)


def main() -> None:
    try:
        from PIL import Image
    except ImportError:
        sys.exit("error: Pillow is needed for the .ico and .icns (pip install pillow)")
    tool = resvg()
    text = SVG.read_text(encoding="utf-8")
    if 'viewBox="0 0 512 512"' not in text:
        sys.exit(f"error: {SVG} must use viewBox=\"0 0 512 512\"")
    if OUT.exists():
        shutil.rmtree(OUT)
    OUT.mkdir(parents=True)
    with tempfile.TemporaryDirectory() as tmp_name:
        tmp = Path(tmp_name)
        # macOS: the tile is 824 of 1024 px inside a transparent margin (Apple's grid), as upstream.
        mac = tmp / "mac.svg"
        mac.write_text(text.replace('viewBox="0 0 512 512"', 'viewBox="-62.14 -62.14 636.27 636.27"', 1), encoding="utf-8")
        shutil.copyfile(SVG, OUT / "filmcraft.svg")
        shutil.copyfile(SMALL if SMALL.exists() else SVG, OUT / "filmcraft-small.svg")
        render(tool, SVG, 1024, OUT / "filmcraft-1024.png")
        render(tool, mac, 512, OUT / "filmcraft-macos-512.png")
        for s in HICOLOR:
            render(tool, SMALL if s <= 32 and SMALL.exists() else SVG, s, OUT / "hicolor" / f"{s}x{s}" / "apps" / f"{APP_ID}.png")
        scalable = OUT / "hicolor" / "scalable" / "apps" / f"{APP_ID}.svg"
        scalable.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(SVG, scalable)
        render(tool, SVG, 128, OUT / "favicon.png")
        # Windows .ico: one exact render per size.
        frames = []
        for s in ICO:
            p = tmp / f"ico-{s}.png"
            render(tool, SMALL if s <= 32 and SMALL.exists() else SVG, s, p)
            frames.append(Image.open(p).convert("RGBA"))
        big = frames[-1]
        big.save(OUT / "filmcraft.ico", format="ICO", sizes=[(s, s) for s in ICO], append_images=frames[:-1])
        # macOS .icns from the Apple-grid render.
        p = tmp / "mac-1024.png"
        render(tool, mac, 1024, p)
        Image.open(p).convert("RGBA").save(OUT / "filmcraft.icns", format="ICNS")
    # The stamp apply.py compares with icon.svg to know the renders are current (LF-normalised).
    digest = source_hash()
    (OUT / "SOURCE.sha256").write_text(digest + "\n", encoding="utf-8", newline="\n")
    print(f"icons written to {OUT}")


if __name__ == "__main__":
    main()
