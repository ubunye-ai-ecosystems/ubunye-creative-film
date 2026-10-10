"""Apply (or check) the Ubunye Film rebrand on top of upstream FilmCraft.

    python -I ubunye/brand/apply.py           apply every rule (idempotent, safe to rerun)
    python -I ubunye/brand/apply.py --check   change nothing; exit 1 and list what is missing
                                              or what leaks (upstream marks, ids, links, names)

Run it from anywhere; paths are relative to the repository root. Python 3 standard library only.

After an upstream merge: resolve conflicts in rebranded files by taking upstream's side
(`git checkout --theirs <file>`), run this script, then `--check`. A rule whose upstream text
changed is reported as STALE: fix the rule here (never hand-edit the result only).

What it does, in order:
  1. renames upstream's app-id files (Linux desktop/metainfo/MIME, Flatpak manifests) to ours,
     carrying upstream's latest content over and deleting the old file;
  2. text rules: app id, vendor, links and product name in packaging and a few code files;
  3. exact snippet patches in upstream source files (strict: STALE when not found);
  4. whole-file overlays from ubunye/brand/overlay/ (files that are entirely ours);
  5. locale rows (ubunye/brand/i18n.tsv) in upstream's catalogs;
  6. icons: copies ubunye/brand/generated/ over upstream's icon paths, deletes upstream's
     app-id icons and the ArtCraft marks, writes the .attribution sidecars;
  7. README header and links; appended Ubunye sections in NOTICE and ATTRIBUTION.md.
Interface strings are not edited: crates/ui-egui/src/brand.rs renames "FilmCraft" at run time
for every translated string (see brand::localize), in every language.
"""

import argparse
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BRAND = ROOT / "ubunye" / "brand"

# ---------------------------------------------------------------------------------- identity
# The product name lives here only. To rename: change NAME, move the old value into
# PREVIOUS_NAMES, run this script (and make_icons.py if the artwork changed).
NAME = "Ubunye Film"
PREVIOUS_NAMES: list[str] = []
VENDOR = "Ubunye AI Ecosystems"
SUITE = "Ubunye Creative Studio"
DISTRIBUTOR = "ThabangVision"
APP_ID = "org.ubunye.creative.film"
DOC_UTI = "org.ubunye.creative.film.document"
UPGRADE_CODE = "920776E7-7682-4688-8E8B-B25A5CAB0BF4"  # identities.toml, never change
COMPONENT_GUID = "6B0D5C35-3E0B-4B57-9C55-2F1C0E0A7F31"  # our MSI component (upstream's path differs)
PRODUCT_URL = "https://www.thabangvision.com/tools/film"
SUITE_URL = "https://www.thabangvision.com/tools"
SOURCE_URL = "https://github.com/ubunye-ai-ecosystems/ubunye-creative-film"
ISSUES_URL = SOURCE_URL + "/issues"
RAW_URL = "https://raw.githubusercontent.com/ubunye-ai-ecosystems/ubunye-creative-film/main/"
DOWNLOADS = "https://downloads.thabangvision.com/film/"
ZSYNC_BASE = "https://downloads.thabangvision.com/film/stable/"
MAINTAINER = "Ubunye AI Ecosystems <noreply@thabangvision.com>"
UPSTREAM_URL = "https://github.com/storytold/filmcraft"
CREDIT = "Based on FilmCraft by the ArtCraft team"
ADDED = "2026-10-11 by Claude (agent) for Thabang Mashinini-Sekgoto"
ICON_LICENCE = (
    "Ubunye AI Ecosystems artwork adapted from the ThabangVision logo, MIT OR Apache-2.0; "
    "letter outline from DM Serif Display (SIL OFL 1.1)"
)

UPSTREAM_ID = "ai.storyteller.filmcraft"

# ------------------------------------------------------------------------------- file helpers


def read(path: Path) -> str:
    with open(path, "r", encoding="utf-8", newline="") as f:
        return f.read()


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


class Run:
    def __init__(self, check: bool):
        self.check = check
        self.changed: list[str] = []
        self.problems: list[str] = []

    def rel(self, p: Path) -> str:
        return p.relative_to(ROOT).as_posix()

    def update(self, path: Path, fn) -> None:
        """Apply fn(text) -> text to a text file, keeping its line endings."""
        if not path.exists():
            self.problems.append(f"MISSING {self.rel(path)}")
            return
        raw = read(path)
        crlf = "\r\n" in raw
        text = raw.replace("\r\n", "\n")
        new = fn(text)
        if new != text:
            if self.check:
                self.problems.append(f"NOT APPLIED {self.rel(path)}")
            else:
                write(path, new.replace("\n", "\r\n") if crlf else new)
                self.changed.append(self.rel(path))

    def put_bytes(self, path: Path, data: bytes) -> None:
        if path.exists():
            have = path.read_bytes()
            if have == data:
                return
            # text assets (SVG) may be checked out with CRLF (core.autocrlf): same file
            if path.suffix.lower() == ".svg" and have.replace(b"\r\n", b"\n") == data.replace(b"\r\n", b"\n"):
                return
        if self.check:
            self.problems.append(f"NOT APPLIED {self.rel(path)} (differs from ours)")
            return
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        self.changed.append(self.rel(path))

    def put_text(self, path: Path, text: str) -> None:
        """Write text with the file's existing line endings (LF for a new file)."""
        if path.exists():
            raw = read(path)
            if "\r\n" in raw:
                text = text.replace("\r\n", "\n").replace("\n", "\r\n")
            if raw == text:
                return
        if self.check:
            self.problems.append(f"NOT APPLIED {self.rel(path)}")
            return
        write(path, text)
        self.changed.append(self.rel(path))

    def remove(self, path: Path) -> None:
        if not path.exists():
            return
        if self.check:
            self.problems.append(f"LEAK {self.rel(path)} (must be removed)")
            return
        for attempt in range(10):
            try:
                path.unlink()
                break
            except PermissionError:
                # Windows: a scanner or indexer can hold a just-checked-out file for a moment
                if attempt == 9:
                    raise
                time.sleep(0.5)
        self.changed.append(f"deleted {self.rel(path)}")


# ------------------------------------------------------------------------------- name helper

# Phrases where "FilmCraft" names upstream (credit, copyright) and must stay.
PROTECT = [
    "Based on FilmCraft",
    "FilmCraft by the ArtCraft",
    "the FilmCraft contributors",
    "the FilmCraft authors",
    "FilmCraft contributors",
    "upstream FilmCraft",
    "[FilmCraft](https://github.com/storytold/filmcraft)",
]
NAME_RE = re.compile(r"FilmCraft(?![A-Za-z])")


def rename_product(text: str) -> str:
    keep = {}
    for i, p in enumerate(PROTECT):
        tok = f"\x00{i}\x00"
        keep[tok] = p
        text = text.replace(p, tok)
    text = NAME_RE.sub(NAME, text)
    for tok, p in keep.items():
        text = text.replace(tok, p)
    return text


def literal(pairs, guard: bool = False):
    """Replace every old with new. guard: skip a pair whose new text is already there (for
    snippet patches whose new text contains the old, so a rerun does not apply them twice)."""

    def fn(text: str) -> str:
        for old, new in pairs:
            if guard and new in text:
                continue
            text = text.replace(old, new)
        return text

    return fn


# ------------------------------------------------------------------------- 1. file renames

RENAMES = [
    ("packaging/linux/ai.storyteller.filmcraft.desktop", f"packaging/linux/{APP_ID}.desktop"),
    ("packaging/linux/ai.storyteller.filmcraft.metainfo.xml.in", f"packaging/linux/{APP_ID}.metainfo.xml.in"),
    ("packaging/linux/ai.storyteller.filmcraft.mime.xml", f"packaging/linux/{APP_ID}.mime.xml"),
    ("packaging/linux/flatpak/ai.storyteller.filmcraft.yml", f"packaging/linux/flatpak/{APP_ID}.yml"),
    ("packaging/linux/flatpak/ai.storyteller.filmcraft.bundle.yml", f"packaging/linux/flatpak/{APP_ID}.bundle.yml"),
]

# ------------------------------------------------------------------------------ 2. text rules

APP_ID_RULES = [
    (UPSTREAM_ID + ".project", DOC_UTI),
    (r"ai\.storyteller\.filmcraft", r"org\.ubunye\.creative\.film"),
    (UPSTREAM_ID, APP_ID),
]

PACKAGING_RULES = APP_ID_RULES + [
    ('<developer id="ai.storyteller">', '<developer id="org.ubunye">'),
    ("Learning Machines LLC", VENDOR),
    ("FilmCraft maintainers <filmcraft@storyteller.ai>", MAINTAINER),
    ("https://getartcraft.com/apps/filmcraft", PRODUCT_URL),
    ("https://discord.gg/artcraft", ISSUES_URL),
    ("https://github.com/storytold/filmcraft/issues", ISSUES_URL),
    ("https://raw.githubusercontent.com/storytold/filmcraft/main/", RAW_URL),
    ("https://github.com/storytold/filmcraft", SOURCE_URL),
    ("%APPDATA%\\FilmCraft", "%APPDATA%\\" + APP_ID),
    ('UpgradeCode="31E6FB14-9DBC-41DD-B24B-26B71FB31B62"', f'UpgradeCode="{UPGRADE_CODE}"'),
    ('Guid="15933017-925B-4963-8992-52000EB26AAA"', f'Guid="{COMPONENT_GUID}"'),
    (
        "Copyright © the FilmCraft authors. MIT OR Apache-2.0.",
        f"Copyright © the FilmCraft authors and {VENDOR}. MIT OR Apache-2.0.",
    ),
    # AppImage update information: our download server, never upstream's releases.
    (
        "  # fetch only the blocks that changed in a newer release, through the .zsync published next to\n"
        "  # each AppImage on GitHub Releases. `latest` is the newest published release that is not a\n"
        "  # pre-release. A fork's builds point at its own releases through GITHUB_REPOSITORY.\n"
        '  REPO="${GITHUB_REPOSITORY:-storytold/filmcraft}"\n'
        '  UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|filmcraft-*-linux-$ARCH.AppImage.zsync"\n',
        "  # fetch only the blocks that changed in a newer release, through the .zsync published on\n"
        f"  # {ZSYNC_BASE} (Ubunye: never upstream's GitHub Releases). The release upload\n"
        "  # must publish the stable AppImage and its .zsync under these fixed names.\n"
        f'  UPDATE_INFO="zsync|{ZSYNC_BASE}ubunye-creative-film-linux-$ARCH.AppImage.zsync"\n',
    ),
]


def forward(text: str) -> str:
    """Earlier product names (PREVIOUS_NAMES) become NAME."""
    for prev in PREVIOUS_NAMES:
        text = text.replace(prev, NAME)
    return text


def packaging_text(text: str) -> str:
    text = literal(PACKAGING_RULES)(text)
    return forward(rename_product(text))


# Binary, and release signing (ubunye/CLAUDE.md rule 8: never touched without approval).
NOT_PACKAGING = {"dmg-layout.DS_Store", "sign.ps1", "import-cert.sh"}


def packaging_files() -> list[Path]:
    out = []
    for p in sorted((ROOT / "packaging").rglob("*")):
        if p.is_file() and p.suffix.lower() not in {".tiff", ".png", ".ico", ".icns", ".svg"} and p.name not in NOT_PACKAGING:
            out.append(p)
    return out


WORKFLOWS = [".github/workflows/release.yml", ".github/workflows/packaging-lint.yml", ".github/workflows/freebsd.yml"]

# --------------------------------------------------------------------------- 3. snippet patches

NL = "\n"
PATCHES: list[tuple[str, list[tuple[str, str]]]] = [
    (
        "Cargo.toml",
        [('repository = "https://github.com/storytold/filmcraft"', f'repository = "{SOURCE_URL}"')],
    ),
    (
        ".mcp.json",
        [('    "filmcraft": {', '    "ubunye-creative-film": {'), ('    "filmcraft-headless": {', '    "ubunye-creative-film-headless": {')],
    ),
    (
        "crates/ui-egui/Cargo.toml",
        [("# ArtCraft wordmark (PNG) for the About dialog and Home screen.", "# Our mark (PNG, crate::brand) for the About dialog and Home screen.")],
    ),
    (
        "crates/ui-egui/src/i18n/mod.rs",
        [
            # t() and tr_ctx() draw text; Language::tr stays a pure catalog lookup (menu_items and
            # the catalog tests use it with English keys).
            (
                "pub fn t(text: &str) -> &str {" + NL + "    current().tr(text)" + NL,
                "pub fn t(text: &str) -> &str {" + NL + "    crate::brand::localize(current().tr(text))" + NL,
            ),
            (
                "    lang.catalog().and_then(|c| c.contextual(context, text)).unwrap_or_else(|| lang.tr(text))" + NL,
                "    crate::brand::localize(lang.catalog().and_then(|c| c.contextual(context, text)).unwrap_or_else(|| lang.tr(text)))" + NL,
            ),
        ],
    ),
    (
        "crates/ui-egui/src/lib.rs",
        [
            (
                "let hint = if !self.ui.status.is_empty() { self.ui.status.clone() } else { self.hint_text() };",
                "let hint = crate::brand::localize_owned(if !self.ui.status.is_empty() { self.ui.status.clone() } else { self.hint_text() });",
            )
        ],
    ),
    (
        "crates/ui-egui/src/links.rs",
        [
            (
                "//! FilmCraft is part of the ArtCraft family: the community lives on the ArtCraft Discord, every" + NL
                + "//! app has a page on the ArtCraft website and a public GitHub repository." + NL,
                "//! Modified by Ubunye AI Ecosystems, 2026: the links are ours (crate::brand); the command ids" + NL
                + "//! are upstream's, so `help.discord` opens the Ubunye community." + NL,
            ),
            (
                "/// The ArtCraft community Discord." + NL
                + 'pub const DISCORD: &str = "https://discord.gg/artcraft";' + NL
                + "/// The ArtCraft website." + NL
                + 'pub const WEBSITE: &str = "https://getartcraft.com";' + NL
                + "/// FilmCraft's page on the ArtCraft website." + NL
                + 'pub const APP_PAGE: &str = "https://getartcraft.com/apps/filmcraft";' + NL
                + "/// FilmCraft's source repository." + NL
                + 'pub const GITHUB: &str = "https://github.com/storytold/filmcraft";' + NL
                + "/// New issue on FilmCraft's repository." + NL
                + 'pub const ISSUES: &str = "https://github.com/storytold/filmcraft/issues";' + NL,
                "/// The Ubunye community." + NL
                + "pub const DISCORD: &str = crate::brand::COMMUNITY_URL;" + NL
                + "/// The ThabangVision website." + NL
                + "pub const WEBSITE: &str = crate::brand::WEBSITE_URL;" + NL
                + "/// The product page." + NL
                + "pub const APP_PAGE: &str = crate::brand::PRODUCT_URL;" + NL
                + "/// The source repository." + NL
                + "pub const GITHUB: &str = crate::brand::SOURCE_URL;" + NL
                + "/// New issue on the repository." + NL
                + "pub const ISSUES: &str = crate::brand::ISSUES_URL;" + NL,
            ),
            (
                '    ("help.discord", "Join the ArtCraft Discord…", DISCORD),' + NL
                + '    ("help.website", "ArtCraft Website", WEBSITE),' + NL
                + '    ("help.appPage", "FilmCraft on getartcraft.com", APP_PAGE),' + NL,
                '    ("help.discord", "Join the Ubunye Community…", DISCORD),' + NL
                + '    ("help.website", "ThabangVision Website", WEBSITE),' + NL
                + '    ("help.appPage", "FilmCraft on thabangvision.com", APP_PAGE),' + NL,
            ),
            (
                "    fn urls_follow_the_artcraft_scheme() {" + NL
                + '        assert_eq!(APP_PAGE, format!("{WEBSITE}/apps/{APP}"));' + NL
                + '        assert_eq!(GITHUB, format!("https://github.com/storytold/{APP}"));' + NL,
                "    fn urls_follow_the_ubunye_scheme() {" + NL
                + '        assert_eq!(APP_PAGE, format!("{WEBSITE}/tools/film"));' + NL
                + f'        assert_eq!(GITHUB, "{SOURCE_URL}");' + NL
                + '        assert_eq!(APP, "filmcraft");' + NL,
            ),
        ],
    ),
    (
        "crates/ui-egui/src/menus.rs",
        [
            (
                '    uic!("help.discord", "Join the ArtCraft Discord…", ["Help"], None),' + NL
                + '    uic!("help.website", "ArtCraft Website", ["Help"], None),' + NL
                + '    uic!("help.appPage", "FilmCraft on getartcraft.com", ["Help"], None),' + NL,
                '    uic!("help.discord", "Join the Ubunye Community…", ["Help"], None),' + NL
                + '    uic!("help.website", "ThabangVision Website", ["Help"], None),' + NL
                + '    uic!("help.appPage", "FilmCraft on thabangvision.com", ["Help"], None),' + NL,
            ),
            (
                '    let label = if it.checked.is_some() { format!("      {}", it.label) } else { it.label.clone() };',
                '    let label = crate::brand::localize_owned(if it.checked.is_some() { format!("      {}", it.label) } else { it.label.clone() });',
            ),
        ],
    ),
    (
        "crates/ui-egui/src/header.rs",
        [
            (
                "    // Community: a labelled Discord button, always one click away." + NL
                + "    {" + NL
                + '        let label = "Discord";' + NL,
                "    // Community: a labelled button, always one click away (Ubunye: our community link)." + NL
                + "    {" + NL
                + '        let label = tl!("Community");' + NL,
            ),
            (
                '.on_hover_text(tl!("Join the ArtCraft Discord (discord.gg/artcraft)"));' + NL
                + '        app.auto.add("header.discord", r, "Join the ArtCraft Discord");' + NL,
                '.on_hover_text(tl!("Join the Ubunye Community"));' + NL
                + '        app.auto.add("header.discord", r, "Join the Ubunye Community");' + NL,
            ),
        ],
    ),
    (
        "crates/ui-egui/src/icons.rs",
        [("/// Community chat (the ArtCraft Discord): a speech bubble", "/// Community chat (the community link): a speech bubble")],
    ),
    (
        "crates/ui-egui/src/panels/import_mode.rs",
        [
            (
                "    // ArtCraft wordmark (first-party trademark, docs/brand/), then the section title.",
                "    // Our mark and name (crate::brand), then the section title.",
            ),
            ('tl!("Join us on Discord")', 'tl!("Join the Ubunye Community")'),
        ],
    ),
    (
        "crates/ui-egui/src/panels/dialogs.rs",
        [
            (
                "/// Help ▸ About FilmCraft, in three tabs: About (version and the ArtCraft community links),",
                "/// Help ▸ About, in three tabs: About (version, upstream credit and the community links),",
            ),
            (
                "/// About ▸ About: version, licence and the ArtCraft community links. Joining the Discord is the" + NL
                + "/// first, accented button." + NL,
                "/// About ▸ About: our mark, name, version, the plain-text upstream credit, licence and the" + NL
                + "/// community links (Ubunye AI Ecosystems). Joining the community is the first, accented button." + NL,
            ),
            (
                "    // ArtCraft wordmark (first-party trademark, docs/brand/)." + NL
                + "    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::hover());" + NL
                + "    crate::brand::paint_wordmark(ui, egui::pos2(r.min.x, r.center().y), 20.0, app.ui.dark);" + NL
                + "    ui.add_space(6.0);" + NL
                + '    ui.heading("FilmCraft");' + NL
                + '    ui.label(tlf!("Version {version}", version = env!("CARGO_PKG_VERSION")));' + NL
                + '    ui.label(tl!("A clean-room, pure-Rust non-linear video editor. Part of the ArtCraft family."));' + NL,
                "    // Ubunye: our mark and name (crate::brand) and the plain-text upstream credit." + NL
                + "    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), egui::Sense::hover());" + NL
                + "    crate::brand::paint_mark(ui, egui::pos2(r.min.x, r.center().y), 56.0);" + NL
                + "    ui.add_space(6.0);" + NL
                + "    ui.heading(crate::brand::NAME);" + NL
                + '    ui.label(tlf!("Version {version}", version = env!("CARGO_PKG_VERSION")));' + NL
                + "    ui.label(crate::brand::PART_OF);" + NL
                + "    let credit = ui.hyperlink_to(crate::brand::BASED_ON, crate::brand::UPSTREAM_URL);" + NL
                + '    app.auto.add("about.upstream", credit.rect, crate::brand::BASED_ON);' + NL
                + '    ui.label(tl!("A clean-room, pure-Rust non-linear video editor."));' + NL,
            ),
            (
                '    link(ui, "discord", Icon::Chat, tl!("Join the ArtCraft Discord"), links::DISCORD, true);',
                '    link(ui, "discord", Icon::Chat, tl!("Join the Ubunye Community"), links::DISCORD, true);',
            ),
            (
                '    link(ui, "website", Icon::Globe, "getartcraft.com", links::WEBSITE, false);' + NL
                + '    link(ui, "appPage", Icon::Globe, tl!("FilmCraft on getartcraft.com"), links::APP_PAGE, false);' + NL,
                '    link(ui, "website", Icon::Globe, "thabangvision.com", links::WEBSITE, false);' + NL
                + '    link(ui, "appPage", Icon::Globe, tl!("FilmCraft on thabangvision.com"), links::APP_PAGE, false);' + NL,
            ),
        ],
    ),
    (
        "crates/ui-egui/src/panels/keyboard.rs",
        [('pub const HELP_URL: &str = "https://github.com/storytold/filmcraft/tree/main/docs";', "pub const HELP_URL: &str = crate::brand::DOCS_URL;")],
    ),
    (
        "crates/engine/src/autosave.rs",
        [
            (
                "/// macOS `~/Library/Application Support/FilmCraft`, Windows `%APPDATA%\\FilmCraft`," + NL
                + "/// elsewhere `$XDG_DATA_HOME/filmcraft` or `~/.local/share/filmcraft`." + NL,
                f"/// macOS `~/Library/Application Support/{APP_ID}`, Windows `%APPDATA%\\{APP_ID}`," + NL
                + f"/// elsewhere `$XDG_DATA_HOME/{APP_ID}` or `~/.local/share/{APP_ID}` (Ubunye: named after" + NL
                + "/// the app id, so an installed upstream FilmCraft never shares data with us; its folder is not read)." + NL,
            ),
            (
                '        return env("HOME").map(|h| h.join("Library/Application Support/FilmCraft"));' + NL
                + "    }" + NL
                + "    if cfg!(windows) {" + NL
                + '        return env("APPDATA").map(|h| h.join("FilmCraft"));' + NL
                + "    }" + NL
                + '    env("XDG_DATA_HOME").map(|d| d.join("filmcraft")).or_else(|| env("HOME").map(|h| h.join(".local/share/filmcraft")))' + NL,
                '        return env("HOME").map(|h| h.join("Library/Application Support").join(APP_DIR));' + NL
                + "    }" + NL
                + "    if cfg!(windows) {" + NL
                + '        return env("APPDATA").map(|h| h.join(APP_DIR));' + NL
                + "    }" + NL
                + '    env("XDG_DATA_HOME").map(|d| d.join(APP_DIR)).or_else(|| env("HOME").map(|h| h.join(".local/share").join(APP_DIR)))' + NL,
            ),
            (
                "pub fn default_data_dir() -> Option<PathBuf> {" + NL,
                "pub fn default_data_dir() -> Option<PathBuf> {" + NL + f'    const APP_DIR: &str = "{APP_ID}";' + NL,
            ),
        ],
    ),
    (
        "crates/engine/src/previews.rs",
        [('crate::temp_dir().join("FilmCraft Previews")', f'crate::temp_dir().join("{NAME} Previews")')],
    ),
    (
        "crates/automation/src/server.rs",
        [
            ('"screenshot of the live FilmCraft window"', f'"screenshot of the live {NAME} window"'),
            ('const INSTRUCTIONS: &str = "FilmCraft video editor (Premiere Pro-class).', f'const INSTRUCTIONS: &str = "{NAME} video editor (Premiere Pro-class).'),
        ],
    ),
    (
        "apps/filmcraft-cli/src/main.rs",
        [
            ("filmcraft-cli: drive FilmCraft from the shell", f"filmcraft-cli: drive {NAME} from the shell"),
            ("  --data-dir <dir>              FilmCraft data directory for user export presets (headless;", f"  --data-dir <dir>              {NAME} data directory for user export presets (headless;"),
            ('outln!("filmcraft-cli {}", env!("CARGO_PKG_VERSION"))', f'outln!("filmcraft-cli {{}} ({NAME})", env!("CARGO_PKG_VERSION"))'),
        ],
    ),
    (
        "apps/filmcraft-web/src/api.rs",
        [('js_sys::Error::new("FilmCraft is not running yet")', f'js_sys::Error::new("{NAME} is not running yet")')],
    ),
    (
        "apps/filmcraft/build.rs",
        [
            (
                '            .set("ProductName", "FilmCraft")' + NL
                + '            .set("FileDescription", "FilmCraft video editor")' + NL
                + '            .set("LegalCopyright", "Copyright (c) the FilmCraft contributors. MIT OR Apache-2.0.")' + NL,
                f'            .set("ProductName", "{NAME}")' + NL
                + f'            .set("CompanyName", "{VENDOR}")' + NL
                + f'            .set("FileDescription", "{NAME} video editor")' + NL
                + f'            .set("LegalCopyright", "Copyright (c) the FilmCraft contributors and {VENDOR}. MIT OR Apache-2.0.")' + NL,
            )
        ],
    ),
    (
        "apps/filmcraft/src/main.rs",
        [
            (f'const APP_ID: &str = "{UPSTREAM_ID}";', "const APP_ID: &str = filmcraft_ui_egui::brand::APP_ID;"),
            ('.set_title("FilmCraft")', ".set_title(filmcraft_ui_egui::brand::NAME)"),
            ('.with_title("FilmCraft")', ".with_title(filmcraft_ui_egui::brand::NAME)"),
            ("    let started = eframe::run_native(" + NL + '        "FilmCraft",' + NL, "    let started = eframe::run_native(" + NL + "        filmcraft_ui_egui::brand::NAME," + NL),
            ('log::info!("FilmCraft {} ({} {})", env!', 'log::info!("{} {} ({} {})", filmcraft_ui_egui::brand::NAME, env!'),
            (
                'let msg = format!("FilmCraft could not start its window: {e}\\n\\nUpdating the graphics driver usually fixes this.");',
                'let msg = format!("{} could not start its window: {e}\\n\\nUpdating the graphics driver usually fixes this.", filmcraft_ui_egui::brand::NAME);',
            ),
            ('tell(&format!("filmcraft {}\\n", env!("CARGO_PKG_VERSION")), false);', 'tell(&format!("{} (filmcraft) {}\\n", filmcraft_ui_egui::brand::NAME, env!("CARGO_PKG_VERSION")), false);'),
            ("            tell(args::USAGE, false);", '            tell(&format!("{}\\n\\n{}", filmcraft_ui_egui::brand::NAME, args::USAGE), false);'),
        ],
    ),
    (
        "apps/filmcraft/src/native_menu.rs",
        [
            ('Submenu::new("FilmCraft", true)', "Submenu::new(filmcraft_ui_egui::brand::NAME, true)"),
            ('MenuItem::with_id("app.about", "About FilmCraft", true, None)', 'MenuItem::with_id("app.about", format!("About {}", filmcraft_ui_egui::brand::NAME), true, None)'),
            ('MenuItem::with_id("help.discord", "Join the ArtCraft Discord…", true, None)', 'MenuItem::with_id("help.discord", "Join the Ubunye Community…", true, None)'),
            ("MenuItem::with_id(it.id.clone(), &it.label, true,", "MenuItem::with_id(it.id.clone(), filmcraft_ui_egui::brand::localize(&it.label), true,"),
            ("mi.set_text(&it.label);", "mi.set_text(filmcraft_ui_egui::brand::localize(&it.label));"),
        ],
    ),
    (
        "apps/filmcraft/src/app_nap.rs",
        [('NSString::from_str("FilmCraft plays and renders video in the background")', f'NSString::from_str("{NAME} plays and renders video in the background")')],
    ),
    (
        "apps/filmcraft/src/dev_icon.rs",
        [
            ('desktop_file_content(crate::APP_ID, "FilmCraft", exe)', "desktop_file_content(crate::APP_ID, filmcraft_ui_egui::brand::NAME, exe)"),
            (f'desktop_file_content("{UPSTREAM_ID}", "FilmCraft", ', f'desktop_file_content("{APP_ID}", filmcraft_ui_egui::brand::NAME, '),
        ],
    ),
    (
        "apps/filmcraft-web/web/index.html",
        [
            ("<title>FilmCraft</title>", f"<title>{NAME}</title>"),
            ("Loading FilmCraft…", f"Loading {NAME}…"),
            (">FilmCraft stopped working<", f">{NAME} stopped working<"),
            ('"FilmCraft failed to start:\\n"', f'"{NAME} failed to start:\\n"'),
        ],
    ),
]

# Code files that also get the plain app-id rename (comments and include paths).
APP_ID_FILES = ["apps/filmcraft/src/main.rs", "apps/filmcraft/src/dev_icon.rs"]

# ------------------------------------------------------------------------------ 5. locales

LOCALE_RENAMES = {
    "Join the ArtCraft Discord…": "Join the Ubunye Community…",
    "ArtCraft Website": "ThabangVision Website",
    "FilmCraft on getartcraft.com": "FilmCraft on thabangvision.com",
    "Join the ArtCraft Discord (discord.gg/artcraft)": "Join the Ubunye Community",
    "Join the ArtCraft Discord": "Join the Ubunye Community",
    "Join us on Discord": "Join the Ubunye Community",
    "A clean-room, pure-Rust non-linear video editor. Part of the ArtCraft family.": "A clean-room, pure-Rust non-linear video editor.",
}
LOCALE_COMMENTS = [("(FilmCraft, ArtCraft, ", "(FilmCraft, "), ("（FilmCraft、ArtCraft、", "（FilmCraft、")]


def load_locale_rows() -> dict[str, list[tuple[str, str]]]:
    rows: dict[str, list[tuple[str, str]]] = {}
    for line in read(BRAND / "i18n.tsv").replace("\r\n", "\n").split("\n"):
        if not line or line.startswith("#"):
            continue
        lang, src, tr = line.split("\t")
        rows.setdefault(lang, []).append((src, tr))
    return rows


def locale_fn(ours: list[tuple[str, str]]):
    tr_of = dict(ours)

    def fn(text: str) -> str:
        text = literal(LOCALE_COMMENTS)(text)
        lines = text.split("\n")
        present = {ln.split("\t")[1] for ln in lines if ln.startswith("\t") and ln.count("\t") >= 2}
        out = []
        for ln in lines:
            parts = ln.split("\t")
            if len(parts) == 3 and parts[0] == "" and parts[1] in LOCALE_RENAMES:
                new = LOCALE_RENAMES[parts[1]]
                if new in present or new not in tr_of:
                    continue
                present.add(new)
                out.append(f"\t{new}\t{tr_of[new]}")
                continue
            out.append(ln)
        missing = [s for s, _ in ours if s not in present]
        if missing:
            while out and out[-1] == "":
                out.pop()
            out += [f"\t{s}\t{tr_of[s]}" for s in missing] + [""]
        return "\n".join(out)

    return fn


# --------------------------------------------------------------------------------- 6. icons

HICOLOR = ["16x16", "24x24", "32x32", "48x48", "64x64", "128x128", "256x256", "512x512"]
ICON_TITLES = {
    "filmcraft.svg": "canonical vector (copy of ubunye/brand/assets/icon.svg)",
    "filmcraft-small.svg": "vector (copy of ubunye/brand/assets/icon.svg; upstream's lighter trace slot)",
    "filmcraft-1024.png": "1024 px render",
    "filmcraft-macos-512.png": "macOS runtime Dock icon (512 px, Apple grid margin)",
    "filmcraft.ico": "Windows .ico (16-256 px)",
    "filmcraft.icns": "macOS .icns (Apple grid margin)",
}


def icon_map() -> list[tuple[str, str, str]]:
    """(generated file, target path, title) for every icon file we ship."""
    out = [(n, f"assets/app-icon/{n}", t) for n, t in ICON_TITLES.items()]
    for s in HICOLOR:
        out.append((f"hicolor/{s}/apps/{APP_ID}.png", f"assets/app-icon/hicolor/{s}/apps/{APP_ID}.png", f"Linux hicolor theme {s.split('x')[0]} px"))
    out.append((f"hicolor/scalable/apps/{APP_ID}.svg", f"assets/app-icon/hicolor/scalable/apps/{APP_ID}.svg", "Linux hicolor theme scalable"))
    out.append(("favicon.png", "apps/filmcraft-web/web/favicon.png", "web app icon (favicon and loading image), 128 px"))
    return out


def sidecar(asset: Path, title: str, notes: str) -> str:
    return (
        f"asset:        {asset.name}\n"
        f"title:        {NAME} app icon, {title}\n"
        f"author:       {VENDOR}\n"
        "source:       original work (Ubunye AI Ecosystems artwork adapted from the ThabangVision logo; ubunye/brand/assets/icon.svg)\n"
        "license:      MIT OR Apache-2.0 (letter outline: OFL-1.1, DM Serif Display)\n"
        "license-file: assets/app-icon/LICENSE.txt\n"
        f"added:        {ADDED}\n"
        f"notes:        {notes} {ICON_LICENCE}.\n"
    )


def icon_source_hash() -> str:
    import hashlib

    return hashlib.sha256((BRAND / "assets" / "icon.svg").read_bytes().replace(b"\r\n", b"\n")).hexdigest()


def icons(run: Run) -> None:
    gen = BRAND / "generated"
    stamp = gen / "SOURCE.sha256"
    fresh = stamp.exists() and read(stamp).strip() == icon_source_hash()
    if not fresh:
        if run.check:
            run.problems.append("STALE ubunye/brand/generated: icon.svg changed (run apply.py, or make_icons.py)")
            return
        # Rendering needs resvg and Pillow, so it runs as its own (non-isolated) process.
        r = subprocess.run([sys.executable, str(BRAND / "make_icons.py")])
        if r.returncode != 0:
            run.problems.append("FAILED make_icons.py (needs resvg and Pillow); icons not regenerated")
            return
        run.changed.append("regenerated ubunye/brand/generated from icon.svg")
    if not gen.exists():
        run.problems.append("MISSING ubunye/brand/generated (run python ubunye/brand/make_icons.py)")
        return
    # upstream's app-id icons and the ArtCraft marks go
    for p in sorted((ROOT / "assets/app-icon/hicolor").rglob(f"{UPSTREAM_ID}.*")):
        run.remove(p)
    for p in sorted((ROOT / "docs/brand").glob("*")):
        if p.name != "LICENSE-brand.txt":
            run.remove(p)
    made = "Rendered from ubunye/brand/assets/icon.svg by ubunye/brand/make_icons.py (resvg, Pillow for .ico/.icns) and copied here by ubunye/brand/apply.py."
    for src, dst, title in icon_map():
        s = gen / src
        if not s.exists():
            run.problems.append(f"MISSING ubunye/brand/generated/{src} (run make_icons.py)")
            continue
        run.put_bytes(ROOT / dst, s.read_bytes())
        run.put_text(ROOT / (dst + ".attribution"), sidecar(ROOT / dst, title, made))
        run.put_text(s.with_name(s.name + ".attribution"), sidecar(s, title, "Rendered by ubunye/brand/make_icons.py; the source apply.py copies."))
    icon = BRAND / "assets" / "icon.svg"
    run.put_text(icon.with_name("icon.svg.attribution"), sidecar(icon, "master artwork (512 tile)", "Master file; every other icon is rendered from it."))


# ------------------------------------------------------------------------- 7. README, notices

README_HEADER = f"""<!-- ubunye:header:start -->
<p align="center"><img src="assets/app-icon/filmcraft-1024.png" alt="" width="128"></p>

<h1 align="center">{NAME}</h1>

> **{NAME}** is the video editor of [{SUITE}]({SUITE_URL}) by {VENDOR}, distributed by {DISTRIBUTOR}.
> Based on [FilmCraft](https://github.com/storytold/filmcraft) by the ArtCraft team, used under MIT OR Apache-2.0.
> Product page: <{PRODUCT_URL}> · Downloads: <{DOWNLOADS}> · Source: <{SOURCE_URL}> · Issues: <{ISSUES_URL}>
>
> The rest of this README is upstream's, with the product name changed where it describes this
> distribution. Fork rules are in [ubunye/](ubunye/); the rebrand is described in
> [ubunye/REBRANDING.md](ubunye/REBRANDING.md).
<!-- ubunye:header:end -->
"""

README_LINKS = f"""<p align="center">
  <a href="{PRODUCT_URL}"><b>{NAME} on thabangvision.com</b></a> ·
  <a href="{SUITE_URL}">{SUITE}</a> ·
  <a href="{SOURCE_URL}">Source on GitHub</a>
</p>
"""

README_SUITE = f"""## {SUITE}

{NAME} is part of [{SUITE}]({SUITE_URL}) by {VENDOR}, distributed by {DISTRIBUTOR},
alongside the suite's photo and photo-library tools. Questions, bug reports and ideas: [open an issue]({ISSUES_URL}).

"""

README_REGEX = [
    # upstream's logo block and title (everything before and including the <h1>)
    (r"\A.*?<h1 align=\"center\">FilmCraft</h1>\n", README_HEADER),
    (r"  By the ArtCraft team\.\n", "  Based on FilmCraft by the ArtCraft team.\n"),
    (r"<p align=\"center\">\n  <a href=\"https://discord\.gg/artcraft\">.*?</p>\n\n", ""),
    (r"<p align=\"center\">\n  <a href=\"https://getartcraft\.com/apps/filmcraft\"><b>FilmCraft on getartcraft\.com</b></a>.*?</p>\n", README_LINKS),
    (r"> \[!NOTE\]\n> \*\*ArtCraft is a community.*?discord\.gg/artcraft\)\.\*\*\n\n", ""),
    (r"## The Crafting Apps\n.*?(?=## License and credits)", README_SUITE),
    (r"<p align=\"center\">\n  <a href=\"https://getartcraft\.com/\"><img alt=\"ArtCraft\".*?</p>\n", f'<p align="center"><sub>{NAME} is made by {VENDOR}. Based on FilmCraft by the ArtCraft team and community.</sub></p>\n'),
    (
        r"The ArtCraft name, wordmark and logos in \[`docs/brand/`\]\(docs/brand/\) are trademarks of the\n.*?Forks and modified versions must remove them\.\n",
        "The name, wordmark and logos of the ArtCraft Team are its trademarks. As\n"
        "[`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt) requires of modified versions, they are not part of this distribution.\n",
    ),
]

README_LITERAL = [
    (
        ", apart from the ArtCraft name and logos, which are trademarks of the ArtCraft Team used under [`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt).",
        " (the ArtCraft Team's name and logos are not shipped in this distribution).",
    ),
    (
        "**New to FilmCraft?** Download it from the [FilmCraft page on getartcraft.com](https://getartcraft.com/apps/filmcraft). That's the easiest way to install it.",
        f"**New to FilmCraft?** Download it from the [product page]({PRODUCT_URL}) or from <{DOWNLOADS}>. That's the easiest way to install it.",
    ),
    ("https://github.com/storytold/filmcraft/releases", SOURCE_URL + "/releases"),
    ("[open an issue](https://github.com/storytold/filmcraft/issues) or tell us in [Discord](https://discord.gg/artcraft)", f"[open an issue]({ISSUES_URL})"),
    ("Ask in [Discord](https://discord.gg/artcraft).", f"Ask in the [issues]({ISSUES_URL})."),
    ("repos=storytold/filmcraft", "repos=ubunye-ai-ecosystems/ubunye-creative-film"),
    ("repos=storytold%2Ffilmcraft", "repos=ubunye-ai-ecosystems%2Fubunye-creative-film"),
]


def readme(text: str) -> str:
    for pat, rep in README_REGEX:
        text = re.sub(pat, lambda _m, r=rep: r, text, count=1 if pat.startswith(r"\A") else 0, flags=re.S)
    text = literal(README_LITERAL)(text)
    head, sep, rest = text.partition("<!-- ubunye:header:end -->")
    if not sep:
        return text
    return head + sep + forward(rename_product(rest))


NOTICE_SECTION = f"""
<!-- ubunye:notice:start -->
{NAME}

{NAME} is a modified version of FilmCraft, made by {VENDOR} and distributed by {DISTRIBUTOR}.
{CREDIT} ({UPSTREAM_URL}), used under MIT OR Apache-2.0.
Changes by {VENDOR} are licensed under the same terms (MIT OR Apache-2.0).
The ArtCraft Marks described above are not part of this distribution (removed as
docs/brand/LICENSE-brand.txt requires). The {NAME} icon is {ICON_LICENCE}.
<!-- ubunye:notice:end -->
"""


def attribution_section() -> str:
    rows = [
        f"| `ubunye/brand/assets/icon.svg` | {VENDOR} | Original work: {NAME} app icon, master artwork | {ICON_LICENCE} (`assets/app-icon/LICENSE.txt`) |"
    ]
    for src, dst, title in icon_map():
        rows.append(f"| `{dst}` | {VENDOR} | Original work: {NAME} app icon, {title} | {ICON_LICENCE} (`assets/app-icon/LICENSE.txt`) |")
        rows.append(f"| `ubunye/brand/generated/{src}` | {VENDOR} | Original work: {NAME} app icon, {title} (source copy) | {ICON_LICENCE} (`assets/app-icon/LICENSE.txt`) |")
    return (
        "\n<!-- ubunye:attribution:start -->\n"
        f"## {VENDOR} additions\n\n"
        f"Assets added by {VENDOR} for {NAME}. They replace upstream's app icon at the same paths\n"
        "(upstream's rows above describe upstream's files). The ArtCraft marks listed above are not part of this distribution.\n\n"
        "| File | Author | Source | Licence |\n|---|---|---|---|\n" + "\n".join(rows) + "\n<!-- ubunye:attribution:end -->\n"
    )


def appended(section: str, start: str):
    def fn(text: str) -> str:
        i = text.find(start)
        if i >= 0:
            j = text.find(start.replace(":start", ":end"), i)
            end = text.find("\n", j) + 1 if j >= 0 else len(text)
            # keep the newline that precedes the section
            return text[: i - 1] + section + text[end:]
        return text.rstrip("\n") + "\n" + section

    return fn


# ------------------------------------------------------------------------------------- apply

TEMPLATE_VALUES = {
    "@NAME@": NAME,
    "@SUITE@": SUITE,
    "@VENDOR@": VENDOR,
    "@DISTRIBUTOR@": DISTRIBUTOR,
    "@APP_ID@": APP_ID,
}


def template(text: str) -> str:
    for k, v in TEMPLATE_VALUES.items():
        text = text.replace(k, v)
    return text



def apply(run: Run) -> None:
    # 1. renames, carrying upstream's latest content
    for old, new in RENAMES:
        o, n = ROOT / old, ROOT / new
        if o.exists():
            if run.check:
                run.problems.append(f"LEAK {old} (rename to {new})")
                continue
            write(n, packaging_text(read(o)))
            o.unlink()
            run.changed.append(f"renamed {old} -> {new}")
    # 2. text rules
    for p in packaging_files():
        run.update(p, packaging_text)
    for w in WORKFLOWS:
        run.update(ROOT / w, literal(APP_ID_RULES))
    # 3. snippet patches (strict)
    for rel, pairs in PATCHES:
        path = ROOT / rel
        if not path.exists():
            run.problems.append(f"MISSING {rel}")
            continue
        text = read(path).replace("\r\n", "\n")
        for old, new in pairs:
            if new in text:
                continue
            if any(new.replace(NAME, prev) in text for prev in PREVIOUS_NAMES):
                continue
            if old not in text:
                run.problems.append(f"STALE {rel}: upstream text changed, update the rule for: {old.splitlines()[0][:90]!r}")
        prev = [(new.replace(NAME, p), new) for _, new in pairs for p in PREVIOUS_NAMES]
        run.update(path, literal(prev + pairs, guard=True))
    # app-id rename in those code files' comments and paths, after the exact patches
    for f in APP_ID_FILES:
        run.update(ROOT / f, literal(APP_ID_RULES))
    # 4. overlays (templates: @NAME@ and friends come from the identity block above)
    overlay = BRAND / "overlay"
    for src in sorted(overlay.rglob("*")):
        if src.is_file():
            run.put_text(ROOT / src.relative_to(overlay), template(read(src).replace("\r\n", "\n")))
    # 5. locales
    rows = load_locale_rows()
    for lang in ["es", "ja", "pt-br", "uk", "zh-cn"]:
        run.update(ROOT / f"crates/ui-egui/src/i18n/{lang}.tsv", locale_fn(rows.get(lang, [])))
    # 6. icons
    icons(run)
    # 7. README, NOTICE, ATTRIBUTION (append only)
    run.update(ROOT / "README.md", readme)
    run.update(ROOT / "NOTICE", appended(NOTICE_SECTION, "<!-- ubunye:notice:start -->"))
    run.update(ROOT / "ATTRIBUTION.md", appended(attribution_section(), "<!-- ubunye:attribution:start -->"))


# ------------------------------------------------------------------------------------ check

LEAK = re.compile(r"ArtCraft|getartcraft|ai\.storyteller|Learning Machines|storyteller\.ai|discord\.gg/artcraft|storytold")
# Allowed wherever they appear: upstream credit, fonts / standards repos, upstream issue refs.
ALLOWED = re.compile(
    "|".join(
        [
            re.escape(CREDIT),
            r"FilmCraft by the ArtCraft team",
            r"by the ArtCraft team",
            r"ArtCraft Team",
            r"github\.com/storytold/craft-fonts\S*",
            r"storytold/craft-fonts",
            r"github\.com/storytold/craftrules\S*",
            r"github\.com/storytold/filmcraft/(?:issues|pull)/\d+",
            r"storytold/[a-z]+#\d+",
            r"\[FilmCraft\]\(https://github\.com/storytold/filmcraft\)",
            r'UPSTREAM_URL: &str = "https://github\.com/storytold/filmcraft"',
            r'UPSTREAM_URL = "https://github\.com/storytold/filmcraft"',
            r"\(https://github\.com/storytold/filmcraft\)",
        ]
    )
)
# Kept as upstream wrote them: licences, notices, upstream docs and history, credits, our layer.
SKIP_PREFIX = ("docs/", "ubunye/", "contributors/", "LICENSE", "NOTICE", "ATTRIBUTION.md", "UPSTREAM", ".github/workflows/release.yml", ".github/workflows/ubunye-",
               # release signing scripts: left as upstream wrote them until the owner approves
               "packaging/windows/sign.ps1", "packaging/macos/import-cert.sh")
TEXT_EXT = {".rs", ".toml", ".md", ".html", ".js", ".mjs", ".json", ".yml", ".yaml", ".sh", ".ps1", ".py", ".xml", ".in", ".desktop", ".wxs", ".tsv", ".plist", ".attribution", ".txt", ".svg", ""}
# Files whose user-visible strings do not pass through brand::localize: no product "FilmCraft"
# outside comments.
NO_HOOK = [
    "apps/filmcraft/src/main.rs",
    "apps/filmcraft/src/native_menu.rs",
    "apps/filmcraft/src/app_nap.rs",
    "apps/filmcraft/build.rs",
    "apps/filmcraft-web/web/index.html",
    "apps/filmcraft-web/src/api.rs",
    "crates/automation/src/server.rs",
    "packaging/macos/Info.plist.in",
    "packaging/windows/filmcraft.wxs",
    f"packaging/linux/{APP_ID}.desktop",
    f"packaging/linux/{APP_ID}.metainfo.xml.in",
    "packaging/linux/nfpm.yaml",
]
COMMENT = re.compile(r"^\s*(//|#|<!--|\*|--)")


def tracked() -> list[str]:
    out = subprocess.run(["git", "-C", str(ROOT), "ls-files", "--cached", "--others", "--exclude-standard"], capture_output=True, text=True, encoding="utf-8", check=True)
    return [f for f in out.stdout.splitlines() if (ROOT / f).is_file()]


def leaks(run: Run) -> None:
    files = tracked()
    for f in files:
        p = ROOT / f
        if f.startswith(SKIP_PREFIX) or p.suffix.lower() not in TEXT_EXT:
            continue
        # upstream's own docs (other than the README) are kept as written
        if f.endswith(".md") and f != "README.md" and not f.startswith("packaging/"):
            continue
        if "/goldens/" in f:
            continue
        try:
            text = read(p)
        except (UnicodeDecodeError, OSError):
            continue
        for n, line in enumerate(text.splitlines(), 1):
            if LEAK.search(ALLOWED.sub("", line)):
                run.problems.append(f"LEAK {f}:{n}: {line.strip()[:140]}")
    for f in NO_HOOK:
        p = ROOT / f
        if not p.exists():
            run.problems.append(f"MISSING {f}")
            continue
        for n, line in enumerate(read(p).splitlines(), 1):
            if COMMENT.match(line) or "i18n::t(" in line:
                continue
            stripped = line
            for ph in PROTECT:
                stripped = stripped.replace(ph, "")
            if NAME_RE.search(stripped):
                run.problems.append(f"NAME {f}:{n}: {line.strip()[:140]}")
    # README: no upstream product name outside credit phrases (after our header)
    text = read(ROOT / "README.md")
    rest = text.partition("<!-- ubunye:header:end -->")[2]
    for ph in PROTECT:
        rest = rest.replace(ph, "")
    for n, line in enumerate(rest.splitlines(), 1):
        if NAME_RE.search(line):
            run.problems.append(f"NAME README.md (after header) line {n}: {line.strip()[:140]}")
    # the run-time renaming hook and the brand layer are in place
    mod = read(ROOT / "crates/ui-egui/src/i18n/mod.rs")
    if mod.count("crate::brand::localize(") < 2:
        run.problems.append("HOOK crates/ui-egui/src/i18n/mod.rs no longer calls crate::brand::localize")
    for f in files:
        if f.endswith(".rs") and not f.startswith("ubunye/") and "docs/brand/" in read(ROOT / f):
            run.problems.append(f"LEAK {f}: refers to docs/brand/ (ArtCraft marks)")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--check", action="store_true", help="change nothing; exit 1 on anything missing or leaking")
    args = ap.parse_args()
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    run = Run(check=args.check)
    apply(run)
    if args.check:
        leaks(run)
    for c in run.changed:
        print(f"applied: {c}")
    for p in run.problems:
        print(p)
    if run.problems:
        print(f"{len(run.problems)} problem(s)", file=sys.stderr)
        return 1
    print("brand: ok" if args.check else f"brand: applied ({len(run.changed)} change(s))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
