// Copyright Ubunye AI Ecosystems, 2026. Licensed under MIT OR Apache-2.0.
//! Kopano: stable ids across computers and the edit / grade / sound lanes.

use serde_json::{Value, json};

use crate::Session;
use crate::kopano::{join_clip, join_track, split_clip, split_track};
use filmcraft_project::ClipId;

fn demo(stable: bool) -> Session {
    let mut s = Session::default();
    if stable {
        s.enable_stable_ids();
    }
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn tmp(name: &str) -> String {
    std::env::temp_dir().join(format!("filmcraft-kopano-{}-{name}", std::process::id())).to_string_lossy().into_owned()
}

/// One saved project opened by two people.
fn two_copies(stable: bool) -> (Session, Session) {
    let path = tmp(&format!("shared-{stable}.fcproj"));
    demo(false).execute("file.saveAs", json!({"path": path})).unwrap();
    let open = || {
        let mut s = Session::default();
        if stable {
            s.enable_stable_ids();
        }
        s.execute("file.open", json!({"path": path})).unwrap();
        s
    };
    (open(), open())
}

fn first_v1(s: &Session) -> ClipId {
    s.active_sequence().unwrap().video_tracks[0].items[0].id
}

fn razor(s: &mut Session) {
    // a cut in the middle of the first V1 clip makes one new clip (plus its linked audio)
    let c = s.active_sequence().unwrap().video_tracks[0].items[0].clone();
    let at = c.start.0 + c.duration.0 / 2;
    s.execute("sequence.addEdit", json!({"seconds": at as f64 / filmcraft_time::TICKS_PER_SECOND as f64})).unwrap();
}

fn all_clip_ids(s: &Session) -> std::collections::BTreeSet<u64> {
    let q = s.active_sequence().unwrap();
    q.video_tracks.iter().chain(q.audio_tracks.iter()).flat_map(|t| t.items.iter()).map(|c| c.id.0).collect()
}

/// The bug this fixes: two people open the same file and each cut a clip. Before, both cuts got
/// the same new clip id; now each computer's ids come from its own block.
#[test]
fn two_people_cutting_the_same_file_make_different_clip_ids() {
    let (mut a, mut b) = two_copies(false);
    let start = all_clip_ids(&a);
    razor(&mut a);
    razor(&mut b);
    let new_a: Vec<u64> = all_clip_ids(&a).difference(&start).copied().collect();
    let new_b: Vec<u64> = all_clip_ids(&b).difference(&start).copied().collect();
    assert!(!new_a.is_empty());
    assert_eq!(new_a, new_b, "without stable ids both computers make the same ids (the old bug)");

    let (mut a, mut b) = two_copies(true);
    assert_ne!(a.project.current_id_block(), b.project.current_id_block());
    razor(&mut a);
    razor(&mut b);
    let new_a: std::collections::BTreeSet<u64> = all_clip_ids(&a).difference(&start).copied().collect();
    let new_b: std::collections::BTreeSet<u64> = all_clip_ids(&b).difference(&start).copied().collect();
    assert!(!new_a.is_empty() && !new_b.is_empty());
    assert!(new_a.is_disjoint(&new_b), "stable ids: {new_a:?} vs {new_b:?}");
    // the clips both had before keep their ids: old projects load unchanged
    assert!(start.is_subset(&all_clip_ids(&a)));
}

#[test]
fn ids_stay_predictable_unless_switched_on() {
    let mut s = demo(false);
    assert_eq!(s.project.current_id_block(), 0);
    let r = s.execute("kopano.idBlock", json!({})).unwrap();
    assert_eq!(r["stableIds"], json!(false));
    // a device-derived block can be set explicitly
    let r = s.execute("kopano.idBlock", json!({"block": 4242})).unwrap();
    assert_eq!(r["block"], json!(4242));
    assert!(s.execute("kopano.idBlock", json!({"block": 0})).is_err());
}

#[test]
fn every_clip_and_track_splits_and_joins_back_exactly() {
    let mut s = demo(true);
    let clip = first_v1(&s);
    // give the clip a grade with keyframes, a motion change and its audio partner a gain
    s.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"})).unwrap();
    s.execute("effects.setParam", json!({"clip": clip.0, "effect": "lumetri", "param": "exposure", "value": 0.5})).unwrap();
    s.execute("effects.setParam", json!({"clip": clip.0, "effect": "motion", "param": "rotation", "value": 12.0})).unwrap();
    let q = s.active_sequence().unwrap().clone();
    let mut n = 0;
    for t in q.video_tracks.iter().chain(q.audio_tracks.iter()) {
        let i = q.video_tracks.iter().chain(q.audio_tracks.iter()).filter(|x| x.kind == t.kind).position(|x| x.id == t.id).unwrap();
        let (e, m) = split_track(t, i).unwrap();
        let (back_i, mut back) = join_track(t.id, &e, m.as_ref()).unwrap();
        back.items = t.items.clone();
        assert_eq!((back_i, &back), (i, t));
        for c in &t.items {
            let lanes = split_clip(c, t.id, t.kind).unwrap();
            assert_eq!(join_clip(c.id, &lanes).unwrap(), (t.id, c.clone()), "clip {}", c.id.0);
            n += 1;
        }
    }
    assert!(n > 3);
    // the grade is its own part, the rotation stays with the edit part
    let c = q.find_item(clip).unwrap().1;
    let l = split_clip(c, q.video_tracks[0].id, filmcraft_project::TrackKind::Video).unwrap();
    assert!(l.grade.as_ref().unwrap()["fx"].get("lumetri").is_some());
    assert!(l.edit["fx"].get("lumetri").is_none() && l.edit["fx"].get("motion").is_some());
}

fn sequence_items(s: &mut Session) -> Value {
    let r = s.execute("kopano.items", json!({})).unwrap();
    let seq = s.state.active_sequence.unwrap().0;
    r["sequences"].as_array().unwrap().iter().find(|q| q["id"] == json!(seq)).unwrap().clone()
}

fn clip_parts(items: &Value, clip: ClipId) -> Value {
    items["clips"].as_array().unwrap().iter().find(|c| c["id"] == json!(clip.0)).unwrap().clone()
}

#[test]
fn applying_what_the_app_reports_changes_nothing() {
    let mut s = demo(true);
    let seq = s.state.active_sequence.unwrap().0;
    let before = sequence_items(&mut s);
    let rev = s.revision;
    for c in before["clips"].as_array().unwrap() {
        let r = s.execute("kopano.apply", json!({"sequence": seq, "clip": c["id"], "edit": c["edit"], "grade": c["grade"], "sound": c["sound"]})).unwrap();
        assert_eq!(r["changed"], json!(false));
    }
    for t in before["tracks"].as_array().unwrap() {
        let r = s.execute("kopano.apply", json!({"sequence": seq, "track": t["id"], "edit": t["edit"], "mix": t["mix"]})).unwrap();
        assert_eq!(r["changed"], json!(false));
    }
    assert_eq!(s.revision, rev);
    assert_eq!(sequence_items(&mut s), before);
}

fn exposure(s: &Session, clip: ClipId) -> f64 {
    let (_, c) = s.active_sequence().unwrap().find_item(clip).unwrap();
    c.effects.iter().find(|e| e.effect == "lumetri").and_then(|e| e.param("exposure")).and_then(|p| p.value.as_f64()).unwrap()
}

/// An editor trims while a colourist grades the same clip on another computer; a third copy takes
/// both parts and ends up with the trim and the grade.
#[test]
fn editor_trim_and_colourist_grade_on_one_clip_both_arrive() {
    let (mut editor, mut colourist) = two_copies(true);
    let (mut third, _) = two_copies(true);
    let clip = first_v1(&editor);
    let seq = editor.state.active_sequence.unwrap().0;
    // colourist: a Lumetri grade
    colourist.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"})).unwrap();
    colourist.execute("effects.setParam", json!({"clip": clip.0, "effect": "lumetri", "param": "exposure", "value": 0.8})).unwrap();
    // editor: trims 12 frames off the head
    editor.execute("timeline.trim", json!({"clip": clip.0, "edge": "in", "deltaFrames": 12})).unwrap();
    let e = clip_parts(&sequence_items(&mut editor), clip);
    let g = clip_parts(&sequence_items(&mut colourist), clip);
    third.execute("kopano.apply", json!({"sequence": seq, "clip": clip.0, "edit": e["edit"]})).unwrap();
    third.execute("kopano.apply", json!({"sequence": seq, "clip": clip.0, "grade": g["grade"]})).unwrap();
    let got = clip_parts(&sequence_items(&mut third), clip);
    assert_eq!(got["edit"], e["edit"]);
    assert_eq!(got["grade"], g["grade"]);
    assert!((exposure(&third, clip) - 0.8).abs() < 1e-9);
    let (_, c) = third.active_sequence().unwrap().find_item(clip).unwrap();
    let (_, ce) = editor.active_sequence().unwrap().find_item(clip).unwrap();
    assert_eq!((c.start, c.duration, c.source_in), (ce.start, ce.duration, ce.source_in));
    // one undo step per part
    third.execute("edit.undo", json!({})).unwrap();
    assert!(third.active_sequence().unwrap().find_item(clip).unwrap().1.effects.iter().all(|e| e.effect != "lumetri"));
}

/// The grade is pinned to source time: Film evaluates effects at media time, so a keyframe on a
/// source frame stays on that frame when another computer's trim arrives, and the grade part
/// does not change at all.
#[test]
fn a_trim_arriving_later_keeps_the_grade_on_its_source_frame() {
    let mut s = demo(true);
    let clip = first_v1(&s);
    let seq = s.state.active_sequence.unwrap().0;
    s.execute("effects.apply", json!({"clips": [clip.0], "effect": "lumetri"})).unwrap();
    let (_, c) = s.active_sequence().unwrap().find_item(clip).unwrap();
    let at = c.start.0 + c.duration.0 / 2;
    s.execute("effects.addKeyframe", json!({"clip": clip.0, "effect": "lumetri", "param": "exposure", "time": at})).unwrap();
    let kf = |s: &Session| {
        let (_, c) = s.active_sequence().unwrap().find_item(clip).unwrap();
        let e = c.effects.iter().find(|e| e.effect == "lumetri").unwrap();
        let k = e.param("exposure").unwrap().keyframes.clone();
        assert_eq!(k.len(), 1);
        (k[0].time, c.effect_time_at(filmcraft_time::Tick(at)))
    };
    let (media, eval) = kf(&s);
    assert_eq!(media, eval, "keyframes are stored in media time");
    s.execute("effects.setKeyframe", json!({"clip": clip.0, "effect": "lumetri", "param": "exposure", "mediaTime": media.0, "value": 1.5})).unwrap();
    let before = clip_parts(&sequence_items(&mut s), clip);
    // an edit part that trims 10 frames off the head, as another computer would send it
    let mut edit = before["edit"].clone();
    let ten = s.sequence_rate().tick_of(10).0;
    let cut = edit["cut"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect::<Vec<_>>();
    edit["cut"] = json!([cut[0] + ten, cut[1] - ten, cut[2] + ten]);
    s.execute("kopano.apply", json!({"sequence": seq, "clip": clip.0, "edit": edit})).unwrap();
    let after = clip_parts(&sequence_items(&mut s), clip);
    assert_eq!(after["grade"], before["grade"], "a trim changed the grade part");
    assert_eq!(after["edit"]["cut"], edit["cut"]);
    // the keyframe still sits on the same source frame, and that frame is still on screen at
    // the same timeline time (a regular trim of the head does not move the rest)
    let (still, eval) = kf(&s);
    assert_eq!((still, eval), (media, media));
}

/// A clip made on another computer (new id from that computer's block) arrives whole; removing
/// it through Kopano takes it away again.
#[test]
fn a_new_clip_from_another_computer_arrives_and_can_be_removed() {
    let (mut a, mut b) = two_copies(true);
    let seq = a.state.active_sequence.unwrap().0;
    let start = all_clip_ids(&a);
    razor(&mut a);
    let items = sequence_items(&mut a);
    let new: Vec<Value> = items["clips"].as_array().unwrap().iter().filter(|c| !start.contains(&c["id"].as_u64().unwrap())).cloned().collect();
    assert!(!new.is_empty());
    // the cut also shortened the clip it split: send every changed or new clip
    for c in items["clips"].as_array().unwrap() {
        b.execute("kopano.apply", json!({"sequence": seq, "clip": c["id"], "edit": c["edit"], "grade": c["grade"], "sound": c["sound"]})).unwrap();
    }
    assert_eq!(sequence_items(&mut b)["clips"], items["clips"]);
    for c in &new {
        b.execute("kopano.apply", json!({"sequence": seq, "clip": c["id"], "remove": true})).unwrap();
    }
    assert!(all_clip_ids(&b).iter().all(|id| start.contains(id)));
}
