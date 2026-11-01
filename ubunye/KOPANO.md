# Kopano in Ubunye Film

Kopano lets several computers edit one Film project. This file says what Film itself does for
that. The connector that turns Film's parts into Kopano items lives in the Kopano lab
(`ubunye-creative-studio-tools`, `research/kopano/lab/kopano-lab/src/film.rs`).

## Stable ids

Every id in a project (items, bins, tracks, clips, transitions, markers, links, groups) comes from
one counter, `Project::next_id`, saved in the file. Two people who opened the same file used to
start from the same counter, so both made "clip 7" for two different clips.

Now each editing session takes its own block of ids (`crates/project/src/ids.rs`):

- an id is `block * 2^24 + n`; block 0 holds every id made before this change, so old projects
  load and keep their ids;
- when the desktop app or the CLI opens, creates or recovers a project, the counter moves to a
  random block that no id of the project uses yet (1 to 2^29 - 1, so ids stay below 2^53 and
  survive JSON readers that hold numbers as doubles);
- `kopano.idBlock {block}` sets a block chosen by the caller (for example derived from the
  device), `kopano.idBlock {}` reads it.

`Session::stable_ids` switches this on. It is off in `Session::default()` so tests keep
predictable ids; `apps/filmcraft` and `apps/filmcraft-cli` call `enable_stable_ids()`.
"Date Created" order is kept by giving a new item one more than the newest `created` value.

## Parts (lanes)

`kopano.items {sequence?}` returns every sequence as tracks and clips, each split into parts so an
editor, a colourist and a sound person can work at once:

| Part | Holds |
|---|---|
| clip `edit` | `track`, `cut` = [start, duration, sourceIn] (one value, so a trim is atomic), speed, name, label, links, non-colour effects (Motion, Opacity, Time Remapping...) |
| clip `grade` (video clips) | colour effects (category Color Correction, e.g. Lumetri). Film evaluates effects at media time, so a grade is pinned to source timecode: a trim never moves it |
| clip `sound` (audio clips) | gain, Essential Sound, source channels, audio stream, every audio effect |
| track `edit` | name, index, locks, eye or target, video transitions |
| track `mix` (audio tracks) | volume, pan, mute, solo, channels, inserts, mixer, audio fades |

Effects keep their order: each part numbers its own effects (`at`) and a grade effect names the
edit effect it follows (`after`), so adding a grade never renumbers the editor's effects.
Splitting then joining gives back exactly the same clip or track (tested in
`crates/engine/src/kopano_tests.rs`).

`kopano.apply {sequence, clip, edit?, grade?, sound?}` puts parts back (a missing part keeps what
the clip has; a new clip needs its `edit` part), `{sequence, clip, remove: true}` removes a clip,
and `{sequence, track, edit?, mix?}` / `{sequence, track, remove: true}` do the same for tracks.
Each call is one undo step named "Kopano". Applying what `kopano.items` reported changes nothing.

Media is reported by path and size. The connector names media by the BLAKE3 hash of the
original file, so Film needs no hashing dependency.

## Share & Collaborate window

People never need scripts: the people icon in the header (also Window ▸ Share & Collaborate)
opens the window (`crates/ui-egui/src/ubunye/kopano_panel.rs`, client in `kopano.rs`). It talks
to kopanod's local API (127.0.0.1, `KOPANO_API_PORT`, default 47100, token in `KOPANO_HOME`) and
binds this window as `{"kind": "film-control", "port": P}`: the `--control` port, or one the app
opens on demand at the first Share or Join.

- Sign in with ThabangVision: the code to check in the browser, "Open the page again", Cancel;
  the service's sentence when the last sign-in ended; Sign out.
- Shared projects with their people by name and what they may do (owner, can edit, approves,
  can view, invited; green dot when online), never emails. The owner invites by email and can
  take someone off ("Remove"); everyone else can "Leave this project". Recent activity.
- "Shared with me": projects shared on ThabangVision that are not on this computer, each with
  "Open here" (`join-shared`, nothing typed).
- "Share this project" with "I approve changes" (default) or "Work together live". An email
  gets an invite after the project is made (`create-project`, then `invite`); a bare account id
  (lab tests) is a member at once.
- Changes to review (View, Before / After / Done, Approve, Decline, Keep yours / Take theirs),
  the member's own state, and the pick for a live clash.
- Footage: what plays from a proxy, originals arriving, "Get originals" (`fetch-original all`).
  In the Project panel a clip whose footage is on its way says "Playing a proxy", "Original
  arriving" or "On its way" instead of "Media missing", and Film's Link Media prompt is put
  away once the footage is coming.
- The answer to an invite is always "Invite sent." (whether the address has an account is not
  told).
- Advanced (testing), collapsed: device and project ids with Copy, and Join by ids.
- A shared project stays with the project it was shared from. Film's project carries the shared
  project's id (`Project::share_id`, saved with it, kept by crash recovery), and the window
  links a shared project again only on that id or the same saved file, never by footage names.
  Opening another project in a shared window stops sharing there (the service may not read or
  change it) and says so; opening the shared project again links it again. A window whose
  project has neither is linked only by the person ("Link this window").
- Errors are shown in plain words; file paths and the service's raw text go to the log.

kopanod's review summary says "photos" for every item; the window counts the changed keys
itself and says clips and tracks. The word "Kopano" is never on screen.

The app starts kopanod when it ships beside it (`apps/filmcraft/src/ubunye_collab.rs`), and
kopanod makes proxies with the `filmcraft-cli` beside it. Installers: `ubunye/packaging/`.

## Not done yet

- New sequences and project items (bins, media metadata) do not travel yet; both sides need the
  sequence and the media items (the connector imports a missing original by hash when it has
  the file).
- Clips arriving on a track are not checked for overlap with clips already there.
- Masks are carried as they are (clip pixels), with no special handling.
- The doc comment on `Keyframe::time` says "clip time", but the engine stores and evaluates
  keyframes in media time (`TrackItem::effect_time_at`, `keyframe_op`). Worth fixing upstream.
