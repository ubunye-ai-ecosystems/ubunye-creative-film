// Copyright Ubunye AI Ecosystems, 2026. Licensed under MIT OR Apache-2.0.
//! Kopano: co-editing one project from several computers.
//!
//! Two things live here.
//!
//! 1. **Stable ids.** A session with [`Session::stable_ids`] on moves the project's id counter
//!    into a block of its own whenever a project is opened or created
//!    (`filmcraft_project::ids`), so two computers never make the same clip id.
//!    `kopano.idBlock` reads the block or sets one derived from the device.
//!
//! 2. **Lanes.** `kopano.items` hands out the timeline as small separate parts so an editor, a
//!    colourist and a sound person can work at once; `kopano.apply` puts one part back. Each
//!    timeline clip is split into:
//!    - `edit`: where and how it sits (track, `cut` = [start, duration, sourceIn], speed, name,
//!      label, links, the non-colour video effects such as Motion and Opacity),
//!    - `grade` (video clips): the colour effects (category Color Correction, e.g. Lumetri).
//!      Film evaluates effects at media time (`TrackItem::effect_time_at`), so a grade's
//!      keyframes are already pinned to source timecode: a trim never moves a grade off its
//!      frames, and a trim and a grade are independent parts,
//!    - `sound` (audio clips): clip gain, Essential Sound, channels and every audio effect.
//!
//!    Each track gives an `edit` part (name, locks, eye/target, video transitions) and, for audio
//!    tracks, a `mix` part (volume, pan, mute, solo, inserts, mixer, audio fades).
//!    Splitting then joining gives back exactly the clip and track (tested): nothing is lost.
//!    Effect order: each part numbers its own effects (`at`), and a grade effect names the edit
//!    effect it follows (`after`), so adding a grade never renumbers the editor's effects.
//!
//! Media is reported by path; the Kopano connector names it by the BLAKE3 hash of the file.

use std::collections::BTreeMap;

use filmcraft_project::{ClipId, EffectKind, ItemId, Project, Track, TrackId, TrackItem, TrackKind};
use serde_json::{Map, Value, json};

use crate::commands::{CommandSpec, always, bad, u64_p};
use crate::{EngineError, Result, Session};

/// Clip fields that travel in the `sound` part.
const SOUND_FIELDS: [&str; 4] = ["gain_db", "essential", "source_channels", "audio_stream"];
/// Audio track fields that travel in the `mix` part.
const MIX_FIELDS: [&str; 8] = ["muted", "solo", "channels", "volume_db", "pan", "effects", "mixer", "transitions"];

// ---------------- stable ids ----------------

/// 64 bits that differ between computers and between runs. No new dependency: std's per-process
/// random hash keys mixed with the clock, the process id and a counter.
fn entropy() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    #[cfg(not(target_arch = "wasm32"))]
    {
        h.write_u32(std::process::id());
        if let Ok(d) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            h.write_u128(d.as_nanos());
        }
    }
    h.finish()
}

impl Session {
    /// Turn on per-session id blocks (the desktop app and the CLI do; tests and embedders that
    /// need predictable ids leave it off) and move the current project into a fresh block.
    pub fn enable_stable_ids(&mut self) {
        self.stable_ids = true;
        enter_fresh_block(self);
    }
}

/// Called whenever a session gets a new project (open, recover, new): with stable ids on, new
/// ids come from a block no other session uses. Not an undo step and not an edit.
pub(crate) fn enter_fresh_block(s: &mut Session) {
    if !s.stable_ids {
        return;
    }
    std::sync::Arc::make_mut(&mut s.project).enter_random_id_block(entropy());
}

fn id_block(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(block) = u64_p(p, "block") {
        let proj = std::sync::Arc::make_mut(&mut s.project);
        proj.enter_id_block(block).map_err(|e| bad("kopano.idBlock", e.to_string()))?;
    }
    Ok(json!({"block": s.project.current_id_block(), "nextId": s.project.next_id, "stableIds": s.stable_ids}))
}

// ---------------- lanes: clips ----------------

fn is_grade(kind: TrackKind, e: &filmcraft_project::EffectInstance) -> bool {
    kind == TrackKind::Video && e.def().is_some_and(|d| d.kind == EffectKind::Video && d.category.contains(&"Color Correction"))
}

fn obj(v: Value) -> Result<Map<String, Value>> {
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(EngineError::Other("expected a JSON object".into())),
    }
}

/// The key of each effect of a clip: `<effect>`, then `<effect>~2`, `~3`... for repeats.
fn fx_keys(c: &TrackItem) -> Vec<String> {
    let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
    c.effects
        .iter()
        .map(|e| {
            let n = seen.entry(e.effect.as_str()).or_insert(0);
            *n += 1;
            if *n == 1 { e.effect.clone() } else { format!("{}~{}", e.effect, n) }
        })
        .collect()
}

/// Some effects of a clip as an object keyed by [`fx_keys`]; each entry is the effect plus its
/// place in this part (`at`) and, with `anchors`, the key of the edit effect it follows (`after`,
/// null at the front).
fn fx_object(c: &TrackItem, keys: &[String], picked: &[usize], anchors: Option<&[usize]>) -> Result<Value> {
    let mut out = Map::new();
    for (n, &i) in picked.iter().enumerate() {
        let (Some(e), Some(key)) = (c.effects.get(i), keys.get(i)) else { continue };
        let mut v = serde_json::to_value(e).map_err(|e| EngineError::Other(e.to_string()))?;
        v["at"] = json!(n);
        if let Some(base) = anchors {
            let prev = base.iter().rev().find(|&&b| b < i).and_then(|&b| keys.get(b));
            v["after"] = json!(prev);
        }
        out.insert(key.clone(), v);
    }
    Ok(Value::Object(out))
}

/// The parts of one clip.
#[derive(Clone, Debug, PartialEq)]
pub struct ClipLanes {
    pub edit: Value,
    pub grade: Option<Value>,
    pub sound: Option<Value>,
}

/// Split a clip into its parts.
pub fn split_clip(c: &TrackItem, track: TrackId, kind: TrackKind) -> Result<ClipLanes> {
    let mut all = obj(serde_json::to_value(c).map_err(|e| EngineError::Other(e.to_string()))?)?;
    all.remove("id");
    all.remove("effects");
    let cut = json!([c.start.0, c.duration.0, c.source_in.0]);
    for k in ["start", "duration", "source_in"] {
        all.remove(k);
    }
    let mut sound = Map::new();
    for k in SOUND_FIELDS {
        if let Some(v) = all.remove(k) {
            sound.insert(k.into(), v);
        }
    }
    let keys = fx_keys(c);
    let (grade_fx, other): (Vec<usize>, Vec<usize>) = (0..c.effects.len()).partition(|&i| c.effects.get(i).is_some_and(|e| is_grade(kind, e)));
    let mut edit = all;
    edit.insert("track".into(), json!(track.0));
    edit.insert("cut".into(), cut);
    let (grade, sound) = match kind {
        TrackKind::Video => {
            edit.insert("fx".into(), fx_object(c, &keys, &other, None)?);
            // a video clip's sound fields are defaults; they stay with the edit part
            edit.extend(sound);
            (Some(json!({"fx": fx_object(c, &keys, &grade_fx, Some(&other))?})), None)
        }
        TrackKind::Audio => {
            edit.insert("fx".into(), json!({}));
            sound.insert("fx".into(), fx_object(c, &keys, &other, None)?);
            (None, Some(Value::Object(sound)))
        }
    };
    Ok(ClipLanes { edit: Value::Object(edit), grade, sound })
}

/// One part's effects: (place in the part, the edit effect it follows, key, effect).
type PartFx = Vec<(u64, Option<String>, String, filmcraft_project::EffectInstance)>;

fn take_fx(part: &mut Map<String, Value>) -> Result<PartFx> {
    let Some(fx) = part.remove("fx") else { return Ok(vec![]) };
    let mut out = vec![];
    for (key, mut v) in obj(fx)? {
        let at = v.get("at").and_then(Value::as_u64).ok_or_else(|| EngineError::Other(format!("effect {key} has no place")))?;
        let after = v.get("after").and_then(Value::as_str).map(String::from);
        if let Some(m) = v.as_object_mut() {
            m.remove("at");
            m.remove("after");
        }
        let e = serde_json::from_value(v).map_err(|e| EngineError::Other(format!("effect {key}: {e}")))?;
        out.push((at, after, key, e));
    }
    out.sort_by_key(|x| x.0);
    Ok(out)
}

/// Join parts back into a clip: the inverse of [`split_clip`]. Returns the clip's track too.
pub fn join_clip(id: ClipId, lanes: &ClipLanes) -> Result<(TrackId, TrackItem)> {
    let mut edit = obj(lanes.edit.clone())?;
    let track = edit.remove("track").and_then(|v| v.as_u64()).map(TrackId).ok_or_else(|| EngineError::Other("clip has no track".into()))?;
    let cut = edit.remove("cut").ok_or_else(|| EngineError::Other("clip has no cut".into()))?;
    let cut: [i64; 3] = serde_json::from_value(cut).map_err(|e| EngineError::Other(format!("cut: {e}")))?;
    let base = take_fx(&mut edit)?;
    let mut anchored: PartFx = vec![];
    for part in [&lanes.grade, &lanes.sound].into_iter().flatten() {
        let mut part = obj(part.clone())?;
        anchored.extend(take_fx(&mut part)?);
        edit.extend(part);
    }
    // the edit part's effects in order, each followed by the effects anchored after it; effects
    // anchored at the front come first, those whose anchor is gone come last
    let mut effects = vec![];
    let mut take = |after: Option<&str>, out: &mut Vec<filmcraft_project::EffectInstance>| {
        let (now, rest): (PartFx, PartFx) = std::mem::take(&mut anchored).into_iter().partition(|x| x.1.as_deref() == after);
        anchored = rest;
        out.extend(now.into_iter().map(|x| x.3));
    };
    take(None, &mut effects);
    for (_, _, key, e) in base {
        effects.push(e);
        take(Some(&key), &mut effects);
    }
    effects.extend(anchored.into_iter().map(|x| x.3));
    edit.insert("id".into(), json!(id.0));
    edit.insert("start".into(), json!(cut[0]));
    edit.insert("duration".into(), json!(cut[1]));
    edit.insert("source_in".into(), json!(cut[2]));
    edit.insert("effects".into(), serde_json::to_value(effects).map_err(|e| EngineError::Other(e.to_string()))?);
    let item: TrackItem = serde_json::from_value(Value::Object(edit)).map_err(|e| EngineError::Other(format!("clip {}: {e}", id.0)))?;
    if item.duration.0 <= 0 || item.start.0 < 0 {
        return Err(EngineError::Other(format!("clip {} has an empty or negative cut", id.0)));
    }
    Ok((track, item))
}

// ---------------- lanes: tracks ----------------

/// The parts of one track: `edit`, and `mix` for audio tracks.
pub fn split_track(t: &Track, index: usize) -> Result<(Value, Option<Value>)> {
    let mut all = obj(serde_json::to_value(t).map_err(|e| EngineError::Other(e.to_string()))?)?;
    all.remove("id");
    all.remove("items");
    all.insert("index".into(), json!(index));
    if t.kind == TrackKind::Video {
        return Ok((Value::Object(all), None));
    }
    let mut mix = Map::new();
    for k in MIX_FIELDS {
        if let Some(v) = all.remove(k) {
            mix.insert(k.into(), v);
        }
    }
    Ok((Value::Object(all), Some(Value::Object(mix))))
}

/// Join track parts into a track (no clips) and its index.
pub fn join_track(id: TrackId, edit: &Value, mix: Option<&Value>) -> Result<(usize, Track)> {
    let mut all = obj(edit.clone())?;
    let index = all.remove("index").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
    if let Some(m) = mix {
        all.extend(obj(m.clone())?);
    }
    all.insert("id".into(), json!(id.0));
    all.insert("items".into(), json!([]));
    let t: Track = serde_json::from_value(Value::Object(all)).map_err(|e| EngineError::Other(format!("track {}: {e}", id.0)))?;
    Ok((usize::try_from(index).unwrap_or(usize::MAX), t))
}

// ---------------- commands ----------------

fn kind_name(k: TrackKind) -> &'static str {
    match k {
        TrackKind::Video => "video",
        TrackKind::Audio => "audio",
    }
}

fn sequence_items(p: &Project, seq: ItemId) -> Result<Value> {
    let it = p.item(seq).ok_or_else(|| bad("kopano.items", format!("no sequence {}", seq.0)))?;
    let s = it.as_sequence().ok_or_else(|| bad("kopano.items", format!("item {} is not a sequence", seq.0)))?;
    let (mut tracks, mut clips) = (Vec::new(), Vec::new());
    for list in [&s.video_tracks, &s.audio_tracks] {
        for (index, t) in list.iter().enumerate() {
            let (edit, mix) = split_track(t, index)?;
            tracks.push(json!({"id": t.id.0, "kind": kind_name(t.kind), "edit": edit, "mix": mix}));
            for c in &t.items {
                let l = split_clip(c, t.id, t.kind)?;
                clips.push(json!({"id": c.id.0, "kind": kind_name(t.kind), "edit": l.edit, "grade": l.grade, "sound": l.sound}));
            }
        }
    }
    Ok(json!({"id": seq.0, "name": it.name, "tracks": tracks, "clips": clips}))
}

fn items(s: &mut Session, p: &Value) -> Result<Value> {
    let proj = &s.project;
    let seqs: Vec<ItemId> = match u64_p(p, "sequence") {
        Some(id) => vec![ItemId(id)],
        None => proj.sequences().map(|i| i.id).collect(),
    };
    let sequences = seqs.into_iter().map(|id| sequence_items(proj, id)).collect::<Result<Vec<_>>>()?;
    let media: Vec<Value> = proj
        .items
        .values()
        .filter_map(|it| {
            let m = it.as_media()?;
            let path = match &m.media {
                filmcraft_project::MediaRef::File { path } => Some(path.clone()),
                filmcraft_project::MediaRef::Generator(_) => None,
            };
            Some(json!({"item": it.id.0, "name": it.name, "path": path, "size": m.identity.map(|i| i.size)}))
        })
        .collect();
    Ok(json!({"idBlock": proj.current_id_block(), "media": media, "sequences": sequences}))
}

fn track_kind_of(seq: &filmcraft_project::Sequence, id: TrackId) -> Option<TrackKind> {
    seq.video_tracks.iter().chain(seq.audio_tracks.iter()).find(|t| t.id == id).map(|t| t.kind)
}

/// `kopano.apply {sequence, clip, edit?, grade?, sound?}` or `{sequence, clip, remove: true}`;
/// `{sequence, track, edit?, mix?}` or `{sequence, track, remove: true}`. A missing part keeps
/// what the clip or track has. One undo step.
fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const ID: &str = "kopano.apply";
    let seq = ItemId(u64_p(p, "sequence").ok_or_else(|| bad(ID, "need `sequence`"))?);
    let remove = p.get("remove").and_then(Value::as_bool).unwrap_or(false);
    if let Some(clip) = u64_p(p, "clip").map(ClipId) {
        let old = {
            let sq = s.project.sequence(seq).ok_or_else(|| bad(ID, format!("no sequence {}", seq.0)))?;
            match sq.find_item(clip) {
                Some((tid, c)) => Some((tid, split_clip(c, tid, track_kind_of(sq, tid).unwrap_or_default())?)),
                None => None,
            }
        };
        if remove {
            if old.is_none() {
                return Ok(json!({"changed": false}));
            }
            s.edit("Kopano", |pr, _| {
                let sq = pr.sequence_mut(seq).ok_or_else(|| bad(ID, "sequence vanished"))?;
                for t in sq.video_tracks.iter_mut().chain(sq.audio_tracks.iter_mut()) {
                    t.items.retain(|c| c.id != clip);
                }
                Ok(())
            })?;
            return Ok(json!({"changed": true}));
        }
        let part = |k: &str| p.get(k).filter(|v| !v.is_null()).cloned();
        let (old_edit, old_grade, old_sound) = match &old {
            Some((_, l)) => (Some(l.edit.clone()), l.grade.clone(), l.sound.clone()),
            None => (None, None, None),
        };
        let edit = part("edit").or(old_edit).ok_or_else(|| bad(ID, format!("clip {} is new: need its `edit` part", clip.0)))?;
        let grade = part("grade").or(old_grade);
        let sound = part("sound").or(old_sound);
        let lanes = ClipLanes { edit, grade, sound };
        let (track, item) = join_clip(clip, &lanes)?;
        if old.as_ref().is_some_and(|(t, l)| *t == track && *l == lanes) {
            return Ok(json!({"changed": false}));
        }
        s.edit("Kopano", |pr, _| {
            let sq = pr.sequence_mut(seq).ok_or_else(|| bad(ID, "sequence vanished"))?;
            if track_kind_of(sq, track).is_none() {
                return Err(bad(ID, format!("track {} is not in sequence {}", track.0, seq.0)));
            }
            for t in sq.video_tracks.iter_mut().chain(sq.audio_tracks.iter_mut()) {
                t.items.retain(|c| c.id != clip);
            }
            let t = sq.track_mut(track).ok_or_else(|| bad(ID, "track vanished"))?;
            t.items.push(item);
            t.sort();
            Ok(())
        })?;
        return Ok(json!({"changed": true}));
    }
    if let Some(track) = u64_p(p, "track").map(TrackId) {
        let old = s.project.sequence(seq).ok_or_else(|| bad(ID, format!("no sequence {}", seq.0)))?.track(track).cloned();
        if remove {
            let Some(t) = old else { return Ok(json!({"changed": false})) };
            if !t.items.is_empty() {
                return Err(bad(ID, format!("track {} still has clips", track.0)));
            }
            s.edit("Kopano", |pr, _| {
                let sq = pr.sequence_mut(seq).ok_or_else(|| bad(ID, "sequence vanished"))?;
                sq.video_tracks.retain(|t| t.id != track);
                sq.audio_tracks.retain(|t| t.id != track);
                Ok(())
            })?;
            return Ok(json!({"changed": true}));
        }
        let index_now = s
            .project
            .sequence(seq)
            .and_then(|sq| sq.video_tracks.iter().position(|t| t.id == track).or_else(|| sq.audio_tracks.iter().position(|t| t.id == track)));
        let (old_edit, old_mix) = match (&old, index_now) {
            (Some(t), Some(i)) => {
                let (e, m) = split_track(t, i)?;
                (Some(e), m)
            }
            _ => (None, None),
        };
        let edit = p
            .get("edit")
            .filter(|v| !v.is_null())
            .cloned()
            .or(old_edit.clone())
            .ok_or_else(|| bad(ID, format!("track {} is new: need its `edit` part", track.0)))?;
        let mix = p.get("mix").filter(|v| !v.is_null()).cloned().or(old_mix.clone());
        if old_edit.as_ref() == Some(&edit) && old_mix == mix {
            return Ok(json!({"changed": false}));
        }
        let (index, mut t) = join_track(track, &edit, mix.as_ref())?;
        s.edit("Kopano", |pr, _| {
            let sq = pr.sequence_mut(seq).ok_or_else(|| bad(ID, "sequence vanished"))?;
            let mut items = Vec::new();
            for list in [&mut sq.video_tracks, &mut sq.audio_tracks] {
                if let Some(i) = list.iter().position(|x| x.id == track) {
                    items = list.remove(i).items;
                }
            }
            t.items = items;
            let list = match t.kind {
                TrackKind::Video => &mut sq.video_tracks,
                TrackKind::Audio => &mut sq.audio_tracks,
            };
            list.insert(index.min(list.len()), t);
            Ok(())
        })?;
        return Ok(json!({"changed": true}));
    }
    Err(bad(ID, "need `clip` or `track`"))
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "kopano.idBlock",
            label: "Kopano Id Block",
            menu: &[],
            shortcut: None,
            params: r#"{"block":u64?}"#,
            enabled: always,
            run: id_block,
            journal: false,
        },
        CommandSpec {
            id: "kopano.items",
            label: "Kopano Items",
            menu: &[],
            shortcut: None,
            params: r#"{"sequence":id?}"#,
            enabled: always,
            run: items,
            journal: false,
        },
        CommandSpec {
            id: "kopano.apply",
            label: "Kopano Apply",
            menu: &[],
            shortcut: None,
            params: r#"{"sequence":id,"clip":id?,"track":id?,"edit":obj?,"grade":obj?,"sound":obj?,"mix":obj?,"remove":bool?}"#,
            enabled: always,
            run: apply,
            journal: true,
        },
    ]
}
