# Rebranding FilmCraft → Ubunye Film

Status: **applied** (2026-10-11, branch `ubunye/rebrand`). Product name **Ubunye Film**, part of
**Ubunye Creative Studio**, by Ubunye AI Ecosystems, distributed by ThabangVision.

Everything is re-applied by one script:

```sh
python -I ubunye/brand/apply.py           # apply (idempotent, safe to rerun)
python -I ubunye/brand/apply.py --check   # change nothing; exit 1 and list leaks or missing rules
```

## Obligations
- The ArtCraft name, wordmark and logos are proprietary. Upstream's `docs/brand/LICENSE-brand.txt`
  requires them to be **removed** from any distributed modified version. Done: the logo files are
  deleted (the licence text stays) and nothing compiles them in.
- We say "Based on FilmCraft by the ArtCraft team" in plain text, linked to
  https://github.com/storytold/filmcraft. That credit is in About, the README header and NOTICE.
- LICENSE-MIT, LICENSE-APACHE, NOTICE and ATTRIBUTION.md keep every upstream line. NOTICE and
  ATTRIBUTION.md get an appended Ubunye section (between `ubunye:*:start/end` markers).

## Identity
| Item | Upstream | Ubunye |
|---|---|---|
| Product name | FilmCraft | Ubunye Film (short form "Film") |
| Suite | ArtCraft "Crafting Apps" | Ubunye Creative Studio (distributed by ThabangVision) |
| App / bundle / Flatpak id | `ai.storyteller.filmcraft` | `org.ubunye.creative.film` |
| Project document UTI | `ai.storyteller.filmcraft.project` | `org.ubunye.creative.film.document` (extension `.fcproj` and format unchanged) |
| Vendor / manufacturer | Learning Machines LLC | Ubunye AI Ecosystems |
| Windows MSI UpgradeCode | upstream GUID | `920776E7-7682-4688-8E8B-B25A5CAB0BF4` (from `ubunye-creative-studio-tools/brand/identities.toml`) |
| Per-user data folder | `FilmCraft` / `filmcraft` | `org.ubunye.creative.film` on every OS (upstream's folder is never read) |
| Links | getartcraft.com, discord.gg/artcraft, storytold | thabangvision.com/tools/film, github.com/ubunye-ai-ecosystems |
| AppImage update info | `gh-releases-zsync\|storytold\|filmcraft\|…` | `zsync\|https://downloads.thabangvision.com/film/stable/ubunye-creative-film-linux-<arch>.AppImage.zsync` |
| Icon | owl (upstream) | `ubunye/brand/assets/icon.svg` |

## How it works (keeps upstream merges cheap)
1. **One place for the name.** The identity block at the top of `ubunye/brand/apply.py` (NAME,
   SUITE, VENDOR, APP_ID, links). `ubunye/brand.toml` mirrors it for people. To rename, change
   `NAME`, move the old value into `PREVIOUS_NAMES`, run the script.
2. **Brand layer in the app.** `crates/ui-egui/src/brand.rs` (upstream's ArtCraft wordmark module,
   replaced from the template `ubunye/brand/overlay/crates/ui-egui/src/brand.rs`) holds the name,
   ids, links, our mark and `localize()`. Upstream call sites keep their shape (`brand::paint_wordmark`,
   `links::DISCORD` and so on); `links.rs` points at the brand constants.
3. **Run-time renaming, no locale rewrite.** `i18n::t` and `i18n::tr_ctx` pass every drawn string
   through `brand::localize`, which shows "FilmCraft" as "Ubunye Film" in every language. Upstream's
   `tl!` strings and catalogs stay as they are, and new upstream strings are renamed with no edit.
   `Language::tr` stays a plain catalog lookup, so upstream's catalog tests keep passing. The menu
   bar, macOS menu and status bar also go through it. Only the ArtCraft/Discord rows change in the
   catalogs, from `ubunye/brand/i18n.tsv` (es, ja, zh-cn, pt-br, uk).
4. **Exact snippet patches** for the few places that do not go through `t()` (window title, native
   message boxes, CLI help and version, MCP instructions, WinRes, data folder, About layout). Each
   is an (old, new) pair in `apply.py`; if upstream changes the old text the script says `STALE`.
5. **Text rules for packaging** (`packaging/**` except the signing scripts, and the app-id paths in
   `.github/workflows/{release,packaging-lint,freebsd}.yml`): app id, vendor, links, product name
   (credit and copyright phrases are protected). Upstream's `ai.storyteller.filmcraft.*` packaging
   files are renamed to `org.ubunye.creative.film.*`, carrying upstream's latest content over.
6. **Icons from one SVG.** `ubunye/brand/make_icons.py` (resvg + Pillow) renders every size into
   `ubunye/brand/generated/` (stamped with the SVG's hash); `apply.py` copies them over upstream's
   icon paths, renames the hicolor icons to the new app id, deletes upstream's, and writes all
   `.attribution` sidecars. When `icon.svg` changes, `apply.py` reruns `make_icons.py` itself.
7. **Leak gate.** `apply.py --check` fails on ArtCraft logos, `getartcraft`, `ai.storyteller`,
   "Learning Machines", `storyteller.ai`, Discord and storytold links outside the credit, product
   "FilmCraft" strings in the files that bypass `localize`, a stale icon set or rule, and a missing
   hook. CI (`.github/workflows/ubunye-ci.yml`, job `brand-leak`) runs it and is blocking.

## After an upstream merge
1. For a conflict in a rebranded file, take upstream's side: `git checkout --theirs <file>`.
   For a binary icon conflict, either side (the script rewrites it).
2. `python -I ubunye/brand/apply.py`, then `python -I ubunye/brand/apply.py --check`.
3. On `STALE`: upstream changed the text a rule matches. Update that rule in `apply.py` (not the
   result by hand) and rerun. On `LEAK`/`NAME`: a new upstream string or link; add a rule.
4. Build and run the tests of the crates touched (see `ubunye/patched-paths.txt`).

## Deliberately kept (and why)
- **Adobe and other vendors' names** (Premiere Pro, the "Premiere Pro Compatible" shortcut preset,
  Lumetri, Essential Sound, Ultra Key, Warp Stabilizer, After Effects, Photoshop, Final Cut, Avid,
  DaVinci, parity docs): **not doing, owner decision 2026-10-11.** The earlier inventory item
  "rename Adobe feature labels" is withdrawn.
- **Crate and package names** (`filmcraft-*`), Rust identifiers, binary names (`filmcraft`,
  `filmcraft-cli`), `FILMCRAFT_*` environment variables, `.fcproj`/`.fcgt` and the project schema,
  MCP tool names and the MCP `serverInfo.name` (`filmcraft`; upstream's test pins it), command and
  automation ids (`help.discord` now opens our community), test fixtures and golden files: merge
  cost and compatibility. The `.mcp.json` server keys are renamed.
- **Upstream docs and ADRs** (`docs/`, crate READMEs, AGENTS.md, CLAUDE.md, ROADMAP.md): history.
  Only `README.md` gets our header, links and name.
- **Identity written into exported files** (MXF/AAF/OMF company and product, MP4 `©too`, XMP
  CreatorTool, encoder names such as "FilmCraft H.264", LUT and caption headers, project
  `generator`) and engine error texts: below the UI, covered by tests and golden files, and part of
  interchange keys. Errors shown in the status bar are renamed at display time.
- **Release signing** (`packaging/windows/sign.ps1`, `packaging/macos/import-cert.sh`): not touched
  without approval (ubunye/CLAUDE.md rule 8). sign.ps1 still names FilmCraft and the upstream URL
  in the signature description.
- **`contributors/contributors.json`** and the About ▸ Contributors tab: upstream's credits.

## Open items
- `packaging/macos/dmg/background.tiff` and `dmg-layout.DS_Store` are upstream's renders (owl and
  `FilmCraft.app` position); regenerate with `packaging/macos/dmg/generate.py` on a Mac.
- deb/rpm maintainer default is `Ubunye AI Ecosystems <noreply@thabangvision.com>`; confirm a real
  address (or set `FILMCRAFT_MAINTAINER` in the release job).
- The community link (`help.discord`) points at https://github.com/ubunye-ai-ecosystems until a
  community space exists.
- The release upload must publish the AppImage and `.zsync` under the fixed names above.
- `sign.ps1` description and URL (needs approval).
