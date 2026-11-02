#!/usr/bin/env python3
"""Smoke test of the LIVE Ubunye app: what downloads.thabangvision.com serves right now.

    python ubunye/scripts/live_smoke.py --app film --out OUT_DIR
    python ubunye/scripts/live_smoke.py --app film --out OUT_DIR --cli PATH   # only the media checks

1. Read https://downloads.thabangvision.com/<app>/latest.json and download this platform's installers
   (Linux: .deb, .rpm and AppImage; Windows: .msi and portable .zip; macOS: .dmg), checking size and sha256.
2. Install them the way a user does (apt, dnf in a Fedora container, msiexec, the DMG's .app).
3. Check the installed identity: name, version = latest.json, no upstream names in what the user sees,
   the updater and product links inside the shipped binary, and that the collaboration service is installed.
4. Run the installed app on real samples and check every output with a decoder that is not the app's own.
   On macOS the real window starts too and is driven over its control channel.

Writes OUT_DIR/summary.json and the outputs; exits non-zero on the first failed check. Same structure as
Ubunye Light's copy of this file; the app-specific part is the section named after the app.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path

UA = "ubunye-live-smoke/1.0 (+https://github.com/ubunye-ai-ecosystems)"
DOWNLOADS = "https://downloads.thabangvision.com"

# Internal format tests only: never shown as product demos.
MEDIA = {
    "bbb.mp4": (
        # Big Buck Bunny (c) Blender Foundation, CC BY 3.0: 10 s, H.264, 640x360, 30 fps, no sound
        "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/360/Big_Buck_Bunny_360_10s_1MB.mp4",
        "77145c94c11f3754207499158df22406e1fe7635553c1c86dc5e881dfeb32016",
    ),
}

APPS = {
    "film": {
        "name": "Ubunye Film",
        "app_id": "org.ubunye.creative.film",
        "deb_package": "ubunye-creative-film",  # since 0.7.1
        "product_page": "https://www.thabangvision.com/tools/film",
        "repo": "ubunye-ai-ecosystems/ubunye-creative-film",
        "upstream_app_id": "ai.storyteller.filmcraft",
        "upstream_words": ["FilmCraft", "ArtCraft", "storytold", "getartcraft"],
        "credits": ["Based on FilmCraft by the ArtCraft team", "based on FilmCraft", "the FilmCraft authors"],
        "mac_bundle": "Ubunye Film.app",
        "mac_exe": "Ubunye Film",
        "gui": "filmcraft",
        "cli": "filmcraft-cli",
        "cli_version_names_app": True,
    },
}

RESULTS: list[dict] = []


class Fail(Exception):
    pass


def check(ok: bool, what: str, detail: str = "") -> None:
    RESULTS.append({"check": what, "ok": bool(ok), "detail": detail[:500]})
    print(("PASS " if ok else "FAIL ") + what + (f": {detail}" if detail else ""), flush=True)
    if not ok:
        raise Fail(what + (f": {detail}" if detail else ""))


def fetch(url: str, dest: Path | None = None) -> bytes | None:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    for attempt in range(4):
        try:
            with urllib.request.urlopen(req, timeout=300) as r:
                if dest is None:
                    return r.read()
                with open(dest, "wb") as fh:
                    shutil.copyfileobj(r, fh, 1 << 20)
                return None
        except Exception as e:  # network blips on runners: retry
            if attempt == 3:
                raise
            print(f"retry {url}: {e}", flush=True)
            time.sleep(5 * (attempt + 1))
    return None


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def run(cmd: list, env: dict | None = None, timeout: int = 900, check_rc: bool = True, quiet: bool = False) -> subprocess.CompletedProcess:
    print("$ " + " ".join(str(c) for c in cmd), flush=True)
    p = subprocess.run([str(c) for c in cmd], capture_output=True, text=True, encoding="utf-8", errors="replace",
                       env=env, timeout=timeout, stdin=subprocess.DEVNULL)
    if not quiet and p.stdout.strip():
        print(p.stdout[-4000:], flush=True)
    if not quiet and p.stderr.strip():
        print(p.stderr[-4000:], file=sys.stderr, flush=True)
    if check_rc and p.returncode != 0:
        check(False, "command succeeds", f"{cmd[0]} exited {p.returncode}: {p.stderr[-300:]}")
    return p


def os_name() -> str:
    return {"Linux": "linux", "Windows": "windows", "Darwin": "macos"}[platform.system()]


# ---------------------------------------------------------------------------------------------
# download + identity helpers
# ---------------------------------------------------------------------------------------------
def download_live(app: str, work: Path) -> tuple[dict, dict[str, Path]]:
    latest = json.loads(fetch(f"{DOWNLOADS}/{app}/latest.json"))
    version = latest["version"]
    print(f"live {app} {version}, published {latest.get('published')}", flush=True)
    want = {
        "linux": (".deb", ".rpm", ".AppImage"),
        "windows": (".msi", "-portable.zip"),
        "macos": (".dmg",),
    }[os_name()]
    files: dict[str, Path] = {}
    for f in latest["files"]:
        suffix = next((w for w in want if f["name"].endswith(w)), None)
        if not suffix or ("x86_64" not in f["name"] and "x64" not in f["name"] and "universal" not in f["name"]):
            continue
        dest = work / f["name"]
        fetch(f["url"], dest)
        check(dest.stat().st_size == f["size"], f"download size {f['name']}", f"{dest.stat().st_size} vs {f['size']}")
        if f.get("sha256"):
            check(sha256(dest) == f["sha256"], f"download sha256 {f['name']}")
        check(version in f["name"], f"file name carries the live version {f['name']}")
        files[suffix] = dest
    for w in want:
        check(w in files, f"latest.json lists a {w} for {os_name()}")
    return latest, files


def no_upstream_in_text(cfg: dict, text: str, where: str) -> None:
    cleaned = text
    for credit in cfg["credits"]:
        cleaned = cleaned.replace(credit, "")
    # case-sensitive: the binaries keep upstream's lower-case names
    found = [w for w in cfg["upstream_words"] if w in cleaned]
    check(not found, f"no upstream names in {where}", ", ".join(found))


def binary_identity(cfg: dict, exe: Path) -> None:
    data = exe.read_bytes()
    for must in (cfg["product_page"], cfg["repo"], cfg["app_id"]):
        check(must.encode() in data, f"{exe.name} carries {must}")
    check(cfg["upstream_app_id"].encode() not in data, f"{exe.name} has no upstream app id {cfg['upstream_app_id']}")
    # informational: upstream strings that may legitimately remain (the credit link, library internals)
    for w in (b"storytold/", b"getartcraft.com"):
        RESULTS.append({"check": f"info: occurrences of {w.decode()} in {exe.name}", "ok": True, "detail": str(data.count(w))})


def cli_identity(cfg: dict, cli: Path, version: str, env: dict) -> None:
    out = run([cli, "--version"], env=env).stdout
    check(version in out, f"{cli.name} --version says {version}", out.strip())
    help_text = run([cli, "--help"], env=env, check_rc=False)
    text = help_text.stdout + help_text.stderr
    named = out if cfg["cli_version_names_app"] else text
    check(cfg["name"] in named, f"{cli.name} names {cfg['name']}")
    no_upstream_in_text(cfg, text, f"{cli.name} --help")


def picture(p: Path, size: tuple[int, int] | None = None) -> dict:
    """The file exists, decodes with Pillow, has the size asked for, and is a picture rather than one flat colour."""
    from PIL import Image, ImageStat

    check(p.is_file() and p.stat().st_size > 0, f"output exists and is non-empty: {p.name}", str(p.stat().st_size) if p.exists() else "missing")
    with Image.open(p) as im:
        im.load()
        w, h = im.size
        stat = ImageStat.Stat(im.convert("L").resize((64, 64)))
        info = {"file": p.name, "bytes": p.stat().st_size, "width": w, "height": h, "mode": im.mode, "format": im.format,
                "mean": round(stat.mean[0], 1), "stddev": round(stat.stddev[0], 1)}
    check(w > 0 and h > 0, f"output decodes: {p.name}", json.dumps(info))
    check(info["stddev"] > 3, f"output is not a flat colour: {p.name}", f"stddev {info['stddev']}")
    if size:
        check((w, h) == size, f"output size {size[0]}x{size[1]}: {p.name}", f"{w}x{h}")
    return info


def samples(work: Path) -> dict[str, Path]:
    out = {}
    d = work / "media"
    d.mkdir(exist_ok=True)
    for name, (url, digest) in MEDIA.items():
        p = d / name
        fetch(url, p)
        check(sha256(p) == digest, f"sample sha256 {name}")
        out[name] = p
    return out


# ---------------------------------------------------------------------------------------------
# install, per platform (exercise() is the app's own part)
# ---------------------------------------------------------------------------------------------
def linux(cfg: dict, latest: dict, files: dict, imgs: dict, out: Path, env: dict) -> None:
    version = latest["version"]
    name = run(["dpkg-deb", "-f", files[".deb"], "Package"]).stdout.strip()
    check(name == cfg["deb_package"], f"deb package is named {cfg['deb_package']}", name)
    run(["sudo", "apt-get", "update", "-qq"])
    run(["sudo", "apt-get", "install", "-y", "-qq", str(files[".deb"])], timeout=1800)
    q = run(["dpkg-query", "-W", "-f=${Package} ${Version} ${Maintainer}", name]).stdout
    check(version in q, "deb package version = latest.json", q)
    desktop = Path(f"/usr/share/applications/{cfg['app_id']}.desktop")
    check(desktop.is_file(), f"desktop entry {desktop}")
    text = desktop.read_text(encoding="utf-8")
    check(f"Name={cfg['name']}" in text, "desktop entry names the app", text[:200])
    no_upstream_in_text(cfg, text, "desktop entry")
    gui = Path(shutil.which(cfg["gui"]) or "/nonexistent")
    cli = Path(shutil.which(cfg["cli"]) or "/nonexistent")
    check(gui.is_file() and cli.is_file(), "deb installs the app and the CLI on the PATH", f"{gui} {cli}")
    check(Path("/usr/bin/kopanod").exists(), "deb sets up /usr/bin/kopanod (collaboration)")
    check(Path("/usr/lib/systemd/user/thabangvision-collab.service").exists(), "deb sets up the collaboration start-at-sign-in unit")
    out_gui = run([gui, "--version"], env=env, timeout=60).stdout
    check(version in out_gui, "app --version", out_gui.strip())
    binary_identity(cfg, gui)
    cli_identity(cfg, cli, version, env)
    exercise(cfg, cli, imgs, out, "deb", env)
    # rpm: a clean Fedora container, as a Fedora user installs it
    r = files[".rpm"]
    run(["docker", "run", "--rm", "-v", f"{r.parent}:/w", "fedora:41", "sh", "-c",
         f"dnf install -y -q /w/{r.name} >/dev/null && {cfg['cli']} --version && test -e /usr/bin/kopanod && echo kopanod-linked"],
        timeout=1800)
    check(True, "rpm installs in Fedora 41, its CLI runs, /usr/bin/kopanod is linked")
    # AppImage: the same payload, unpacked (no FUSE on runners)
    ai = files[".AppImage"]
    ai.chmod(0o755)
    root = out.parent / "appimage"
    shutil.rmtree(root, ignore_errors=True)
    root.mkdir()
    subprocess.run([str(ai), "--appimage-extract"], cwd=root, check=True, capture_output=True)
    acli = root / "squashfs-root" / "usr" / "bin" / cfg["cli"]
    check(acli.is_file(), "AppImage carries the CLI", str(acli))
    check((root / "squashfs-root" / "usr" / "bin" / "kopanod").is_file(), "AppImage carries kopanod")
    cli_identity(cfg, acli, version, env)
    binary_identity(cfg, root / "squashfs-root" / "usr" / "bin" / cfg["gui"])
    exercise(cfg, acli, imgs, out, "appimage", env)


def win_uninstall_entry(name: str) -> dict:
    import winreg

    for view in (winreg.KEY_WOW64_64KEY, winreg.KEY_WOW64_32KEY):
        for hive in (winreg.HKEY_LOCAL_MACHINE, winreg.HKEY_CURRENT_USER):
            try:
                root = winreg.OpenKey(hive, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", 0, winreg.KEY_READ | view)
            except OSError:
                continue
            for i in range(winreg.QueryInfoKey(root)[0]):
                sub = winreg.OpenKey(root, winreg.EnumKey(root, i))
                try:
                    if winreg.QueryValueEx(sub, "DisplayName")[0] == name:
                        vals = {}
                        for k in ("DisplayName", "DisplayVersion", "Publisher", "InstallLocation", "URLInfoAbout", "HelpLink"):
                            try:
                                vals[k] = str(winreg.QueryValueEx(sub, k)[0])
                            except OSError:
                                pass
                        return vals
                except OSError:
                    pass
    return {}


def windows(cfg: dict, latest: dict, files: dict, imgs: dict, out: Path, env: dict) -> None:
    version = latest["version"]
    run(["msiexec", "/i", str(files[".msi"]), "/qn", "/norestart", "/l*v", str(out / "msi-install.log")], timeout=1800)
    entry = win_uninstall_entry(cfg["name"])
    check(bool(entry), f"installed: Apps & features lists {cfg['name']}", json.dumps(entry))
    check(entry.get("DisplayVersion", "").startswith(version), "installed version = latest.json", json.dumps(entry))
    no_upstream_in_text(cfg, json.dumps(entry), "Apps & features entry")
    pf = Path(os.environ["ProgramFiles"])
    found = [d for d in (pf / cfg["name"], pf / "Ubunye AI Ecosystems" / cfg["name"]) if (d / f"{cfg['cli']}.exe").is_file()]
    check(bool(found), f"{cfg['cli']}.exe is in the install folder", str(pf))
    d = found[0]
    check((pf / "ThabangVision" / "Collaboration" / "kopanod.exe").is_file(), "MSI installs kopanod.exe (collaboration)")
    # not `app --version` here: a Windows release build shows it in a message box, which waits for a click
    binary_identity(cfg, d / f"{cfg['gui']}.exe")
    cli = d / f"{cfg['cli']}.exe"
    cli_identity(cfg, cli, version, env)
    exercise(cfg, cli, imgs, out, "msi", env)
    pz = out.parent / "portable"
    shutil.rmtree(pz, ignore_errors=True)
    with zipfile.ZipFile(files["-portable.zip"]) as z:
        z.extractall(pz)
    pcli = next(pz.rglob(f"{cfg['cli']}.exe"), None)
    check(pcli is not None, "portable zip carries the CLI")
    cli_identity(cfg, pcli, version, env)
    exercise(cfg, pcli, imgs, out, "portable", env)
    run(["msiexec", "/x", str(files[".msi"]), "/qn", "/norestart", "/l*v", str(out / "msi-uninstall.log")], timeout=1800)
    check(not win_uninstall_entry(cfg["name"]), "uninstall removes the app")


def macos(cfg: dict, latest: dict, files: dict, imgs: dict, out: Path, env: dict) -> None:
    import plistlib

    version = latest["version"]
    mnt = out.parent / "dmg"
    run(["hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", mnt, files[".dmg"]])
    try:
        src = mnt / cfg["mac_bundle"]
        check(src.is_dir(), f"DMG holds {cfg['mac_bundle']}", ", ".join(x.name for x in mnt.iterdir()))
        app = out.parent / cfg["mac_bundle"]
        shutil.rmtree(app, ignore_errors=True)
        run(["ditto", src, app])
    finally:
        run(["hdiutil", "detach", mnt], check_rc=False)
    run(["codesign", "--verify", "--deep", "--strict", app], check_rc=False)
    info = plistlib.loads((app / "Contents" / "Info.plist").read_bytes())
    check(info.get("CFBundleIdentifier") == cfg["app_id"], "bundle id", str(info.get("CFBundleIdentifier")))
    check(info.get("CFBundleShortVersionString") == version, "bundle version = latest.json", str(info.get("CFBundleShortVersionString")))
    check(info.get("CFBundleDisplayName") == cfg["name"], "bundle display name", str(info.get("CFBundleDisplayName")))
    shown = {k: v for k, v in info.items() if isinstance(v, str) and k not in ("CFBundleExecutable", "CFBundleIconFile")}
    for k, val in shown.items():
        no_upstream_in_text(cfg, val, f"Info.plist {k}")
    macos_dir = app / "Contents" / "MacOS"
    exe = macos_dir / cfg["mac_exe"]
    v = run([exe, "--version"], env=env, timeout=60).stdout
    check(version in v, "app --version", v.strip())
    binary_identity(cfg, exe)
    check((macos_dir / "kopanod").is_file(), "the app carries kopanod (collaboration)")
    cli = macos_dir / cfg["cli"]
    check(cli.is_file(), "the app carries the CLI")
    cli_identity(cfg, cli, version, env)
    exercise(cfg, cli, imgs, out, "dmg", env)
    window(cfg, exe, cli, imgs, out, env)


def wait_for_port(proc: subprocess.Popen, port: int, seconds: int = 180) -> socket.socket:
    deadline = time.time() + seconds
    while True:
        try:
            return socket.create_connection(("127.0.0.1", port), timeout=300)
        except OSError:
            check(proc.poll() is None, "app keeps running while it starts", f"exit {proc.poll()}")
            if time.time() > deadline:
                check(False, "app opens its control port", "timed out")
            time.sleep(2)


def stop(proc: subprocess.Popen) -> None:
    proc.terminate()
    try:
        proc.wait(20)
    except subprocess.TimeoutExpired:
        proc.kill()


# ---------------------------------------------------------------------------------------------
# Film
# ---------------------------------------------------------------------------------------------
def ffmpeg() -> str:
    import imageio_ffmpeg  # an independent decoder, not the app's own

    return imageio_ffmpeg.get_ffmpeg_exe()


def decode(path: Path) -> dict:
    """Decode every frame with ffmpeg: what is in the file, frame count, how loud the sound is."""
    check(path.is_file() and path.stat().st_size > 0, f"output exists and is non-empty: {path.name}", str(path.stat().st_size) if path.exists() else "missing")
    p = run([ffmpeg(), "-hide_banner", "-nostats", "-i", path, "-map", "0:v:0", "-map", "0:a:0?", "-af", "volumedetect", "-f", "null", "-"], quiet=True, check_rc=False)
    err = p.stderr
    check(p.returncode == 0, f"ffmpeg decodes {path.name}", err[-300:] if p.returncode else "")
    v = re.search(r"Stream #0:\d+.*?: Video: (\w+).*?, (\d{2,5})x(\d{2,5})", err)
    a = re.search(r"Stream #0:\d+.*?: Audio: (\w+)", err)
    frames = re.findall(r"frame=\s*(\d+)", err)
    vol = re.search(r"mean_volume: (-?[\d.]+) dB", err)
    return {"file": path.name, "bytes": path.stat().st_size,
            "video": v.group(1) if v else None, "width": int(v.group(2)) if v else 0, "height": int(v.group(3)) if v else 0,
            "audio": a.group(1) if a else None, "frames": int(frames[-1]) if frames else 0,
            "mean_volume": float(vol.group(1)) if vol else None}


def psnr(a: Path, b: Path) -> float:
    p = run([ffmpeg(), "-hide_banner", "-nostats", "-i", a, "-i", b, "-lavfi", "[0:v][1:v]psnr", "-f", "null", "-"], quiet=True, check_rc=False)
    m = re.search(r"average:([\d.]+|inf)", p.stderr)
    check(m is not None, f"ffmpeg compares {a.name} with {b.name}", p.stderr[-300:] if not m else "")
    return float(m.group(1))


def clip_with_sound(imgs: dict[str, Path]) -> Path:
    """The sample has no sound. Add a tone, leaving its H.264 untouched, so the sound path is tested too."""
    src = imgs["bbb.mp4"]
    dst = src.with_name("clip.mp4")
    if not dst.exists():
        run([ffmpeg(), "-hide_banner", "-y", "-i", src, "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
             "-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-shortest", dst], quiet=True)
    return dst


def make_project(cli: Path, clip: Path, project: Path, base: list, env: dict) -> None:
    r = json.loads(run([cli, *base, "--save-as", project, "import", clip, "--compact"], env=env, quiet=True).stdout)
    check(r.get("errors") == [] and len(r.get("items", [])) == 1, f"imports the clip", json.dumps(r)[:200])
    run([cli, *base, "--project", project, "--save", "exec", "file.newSequenceFromClip", json.dumps({"items": r["items"]}), "--compact"], env=env, quiet=True)
    run([cli, *base, "--project", project, "--save", "exec", "sequence.addEdit", "seconds=3", "--compact"], env=env, quiet=True)
    s = json.loads(run([cli, *base, "--project", project, "inspect", "sequence", "--compact"], env=env, quiet=True).stdout)
    check(s.get("durationFrames") == 300, "the sequence from the clip is 300 frames, reopened from the saved project", str(s.get("durationFrames")))


def exercise(cfg: dict, cli: Path, imgs: dict[str, Path], out: Path, tag: str, env: dict) -> None:
    clip = clip_with_sound(imgs)
    source = json.loads(run([cli, "probe", clip, "--compact"], env=env, quiet=True).stdout)
    check(source["video"]["width"] == 640 and "H.264" in source["video"]["codec"], f"probe reads the clip ({tag})", json.dumps(source["video"])[:200])
    data = out.parent / f"data-{tag}"
    base = ["--data-dir", data]
    project = out / f"{tag}.fcproj"
    make_project(cli, clip, project, base, env)
    p = ["--project", project]

    # The whole clip to H.264, then checked against the source frame by frame
    full = out / f"{tag}-full.mp4"
    r = run([cli, *base, *p, "export", full, "--format", "h264", "--range", "entire"], env=env, quiet=True)
    check(json.loads(r.stdout[: r.stdout.rindex("}") + 1]).get("offlineMedia") == [], f"export finds its media ({tag})")
    d = decode(full)
    check((d["video"], d["width"], d["height"], d["frames"]) == ("h264", 640, 360, 300), f"H.264 export: 640x360, 300 frames ({tag})", json.dumps(d))
    check(d["audio"] == "aac" and (d["mean_volume"] or -99) > -40, f"H.264 export carries the sound ({tag})", json.dumps(d))
    q = psnr(full, clip)
    check(q > 30, f"H.264 export matches the source picture ({tag})", f"PSNR {q:.1f} dB")

    # A range at half size
    part = out / f"{tag}-part.mp4"
    run([cli, *base, *p, "export", part, "--format", "h264", "--start", "2", "--end", "5", "--scale", "0.5"], env=env, quiet=True)
    d = decode(part)
    check((d["width"], d["height"], d["frames"]) == (320, 180, 90), f"3 s range at half size: 320x180, 90 frames ({tag})", json.dumps(d))

    # A mastering format
    pro = out / f"{tag}-prores.mov"
    run([cli, *base, *p, "export", pro, "--format", "prores", "--start", "0", "--end", "1"], env=env, quiet=True)
    d = decode(pro)
    check((d["video"], d["frames"]) == ("prores", 30), f"ProRes export: 30 frames ({tag})", json.dumps(d))
    pro.unlink()  # large, and already proven

    # One frame of the timeline as a picture
    still = out / f"{tag}-frame.png"
    run([cli, *base, *p, "render", "--seconds", "2", "--out", still], env=env, quiet=True)
    picture(still, (640, 360))
    full.unlink()  # 25 MB per install; the numbers above are the record


def window(cfg: dict, exe: Path, cli: Path, imgs: dict[str, Path], out: Path, env: dict) -> None:
    """The real app window: control channel, then the CLI bridged to it imports and exports through the app."""
    port = 47982
    token = secrets.token_hex(32)
    e = dict(env, FILMCRAFT_CONTROL_TOKEN=token)
    data = out.parent / "data-window"
    log = open(out / "app-stderr.log", "w")
    proc = subprocess.Popen([str(exe), "--control", str(port), "--data-dir", str(data), "--empty", "--no-recover"], env=e, stdout=log, stderr=subprocess.STDOUT)
    try:
        s = wait_for_port(proc, port)
        f = s.makefile("rw", encoding="utf-8", newline="\n")

        def call(method: str, params: dict | None = None, name: str | None = None) -> dict:
            f.write(json.dumps({"id": 1, "token": token, "method": method, "params": params or {}}) + "\n")
            f.flush()
            reply = json.loads(f.readline())
            check(reply.get("ok") is True, name or f"control {method}", json.dumps(reply)[:300])
            return reply.get("result")

        call("ui.inspect", None, "the app window answers ui.inspect")
        bridge = ["--bridge", f"127.0.0.1:{port}"]
        r = json.loads(run([cli, *bridge, "import", clip_with_sound(imgs), "--compact"], env=e, quiet=True).stdout)
        check(r.get("errors") == [] and len(r.get("items", [])) == 1, "the app window imports the clip", json.dumps(r)[:200])
        run([cli, *bridge, "exec", "file.newSequenceFromClip", json.dumps({"items": r["items"]}), "--compact"], env=e, quiet=True)
        dst = out / "app-export.mp4"
        run([cli, *bridge, "export", dst, "--format", "h264", "--start", "0", "--end", "2"], env=e, quiet=True, timeout=300)
        d = decode(dst)
        check((d["width"], d["height"], d["frames"]) == (640, 360, 60), "the app window exports 2 s: 640x360, 60 frames", json.dumps(d))
        shot = out / "app-ui.png"
        call("ui.screenshot", {"path": str(shot)}, "the app window takes a screenshot")
        picture(shot)
    finally:
        stop(proc)
        log.close()


RUNNERS = {"linux": linux, "windows": windows, "macos": macos}


def main() -> int:
    for stream in (sys.stdout, sys.stderr):  # Windows consoles default to cp1252
        stream.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--app", required=True, choices=sorted(APPS))
    ap.add_argument("--out", required=True)
    ap.add_argument("--cli", help="skip download and install: only run the media checks with this CLI")
    a = ap.parse_args()
    cfg = APPS[a.app]
    out = Path(a.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="live-smoke-"))
    env = dict(os.environ)
    env["FILMCRAFT_CONFIG_DIR"] = str(work / "config")  # isolated profile
    summary = {"app": a.app, "os": os_name(), "started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    rc = 0
    try:
        if a.cli:
            exercise(cfg, Path(a.cli).resolve(), samples(work), out, "local", env)
        else:
            latest, files = download_live(a.app, work)
            summary["version"] = latest["version"]
            summary["files"] = [p.name for p in files.values()]
            RUNNERS[os_name()](cfg, latest, files, samples(work), out, env)
    except Fail as e:
        summary["failed"] = str(e)
        rc = 1
    except Exception as e:  # anything unexpected is a failure too, with its message
        summary["failed"] = f"{type(e).__name__}: {e}"
        print(f"FAIL unexpected: {summary['failed']}", flush=True)
        rc = 1
    summary["checks"] = RESULTS
    summary["passed"] = rc == 0
    (out / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    step = os.environ.get("GITHUB_STEP_SUMMARY")
    if step:
        with open(step, "a", encoding="utf-8") as fh:
            fh.write(f"### {cfg['name']} {summary.get('version', '?')} live smoke on {os_name()}: {'PASS' if rc == 0 else 'FAIL'}\n\n")
            for r in RESULTS:
                fh.write(f"- {'PASS' if r['ok'] else 'FAIL'} {r['check']}" + (f" ({r['detail'][:160]})" if r["detail"] else "") + "\n")
            if summary.get("failed"):
                fh.write(f"\n**Failed:** {summary['failed']}\n")
    print(f"\n{'PASS' if rc == 0 else 'FAIL'}: {sum(r['ok'] for r in RESULTS)} checks passed"
          + (f"; failed: {summary.get('failed')}" if rc else ""), flush=True)
    return rc


if __name__ == "__main__":
    sys.exit(main())
