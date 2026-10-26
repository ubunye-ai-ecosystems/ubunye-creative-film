# Installers with ThabangVision collaboration

One download per operating system installs Ubunye Film together with `kopanod`, the small
background service that does the collaboration (inside the code it is the Kopano protocol;
people only ever see "ThabangVision collaboration"). It is the same service Ubunye Light ships:
one per computer, shared by every Ubunye app.

| System | Download | Where kopanod goes | Start at sign-in | Removed by |
|---|---|---|---|---|
| Windows | `.msi` (and a portable `.zip`) | beside `filmcraft.exe` | a value in the user's Run key | the MSI uninstall |
| macOS | `.dmg` | `Ubunye Film.app/Contents/MacOS/` | a LaunchAgent | (drag to Trash: see below) |
| Linux | `.AppImage`, `.deb`, `.rpm`, `.tar.gz` | `/usr/bin` (AppImage: copied to `~/.local/share/kopano/bin`) | a systemd user unit | `.deb` / `.rpm` removal |

Why this way: each one is the package Film already ships, with one more program inside. No
second download, no second installer, nothing to set up.

## Footage proxies need filmcraft-cli

When a teammate's footage is not on this computer, kopanod makes small proxies of it with a
headless Film engine, `filmcraft-cli`. kopanod looks for it in this order: the
`KOPANO_FILM_CLI` variable, then beside `kopanod` itself, then on `PATH`.

- Windows MSI and zip, Linux deb, rpm and tar: `filmcraft-cli` is already beside `kopanod`.
- macOS: the app bundle normally has no CLI (it ships as its own zip), so with collaboration
  `packaging/macos/package.sh` also puts `filmcraft-cli` in `Contents/MacOS/`, signed with the app.
- AppImage: see "Not done yet".

## What happens on the first start

Film does it, as the signed-in person, silently (`apps/filmcraft/src/ubunye_collab.rs`):

1. finds `kopanod` beside itself (a developer build has none: nothing happens);
2. `kopanod init`: makes this computer's device key (into Windows Credential Manager or the
   macOS Keychain);
3. `kopanod autostart on`, unless start at sign-in already points at this `kopanod`;
4. starts `kopanod run` if nothing answers yet.

Until the person signs in with ThabangVision, `kopanod` only answers Film on 127.0.0.1 and
stays offline.

## Uninstall

- Windows: the MSI turns start at sign-in off for the person removing it and stops every
  `kopanod` running from the install folder (`windows/collab.wxs`). An upgrade only stops it.
- Linux `.deb` / `.rpm`: `linux/prerm.sh` stops it and removes each user's unit.
- macOS and AppImage have no uninstaller. The macOS LaunchAgent stays until removed under
  System Settings > General > Login Items.
- The service folder (`%LOCALAPPDATA%\Kopano`, `~/Library/Application Support/Kopano`,
  `~/.local/share/kopano`) stays, like Film's own projects.

## Building

- Windows: `ubunye/packaging/windows/collab-package.ps1 -Arch x64 -KopanoLab <lab>`
- macOS: `ubunye/packaging/macos/collab-package.sh <lab> --arch universal`
- Linux: `ubunye/packaging/linux/collab-package.sh <lab>`

`<lab>` is `research/kopano/lab` of ubunye-creative-studio-tools. Each script builds `kopanod`
and then runs the normal `packaging/<os>/package.*` with `UBUNYE_EXTRA_BIN_DIR` set (a small
hook in those scripts). CI: `.github/workflows/ubunye-collab-packages.yml`, with input
`kopano_ref` (default `fbc9bc26eccef753bd7fb5b6d2f07a1f2010f3b5`, the kopanod tested with Film).

## Not done yet

- Light and Film on one Linux computer: both `.deb` / `.rpm` packages hold `/usr/bin/kopanod`,
  so the package manager refuses the second one. Needs a shared package for the service, or
  one app's packages that leave it out.
- Removing one app's `.deb` / `.rpm` or MSI stops the service the other app also uses; the
  other app starts it again when it next opens.
- AppImage: Film copies `kopanod` to `~/.local/share/kopano/bin` so start at sign-in survives
  the AppImage's temporary mount, but `filmcraft-cli` is not copied with it, so proxies only
  work when the CLI is on `PATH` or `KOPANO_FILM_CLI` is set.
- When Light's `kopanod` is the one running (installed first), it has no `filmcraft-cli`
  beside it on Windows or macOS, so it can not make proxies for Film footage.
- Code signing. Without it Windows Smart App Control blocks the programs on new PCs and macOS
  asks people to allow the app. The workflow has a marked place for the secrets; the release
  signing in `release.yml` is unchanged.
