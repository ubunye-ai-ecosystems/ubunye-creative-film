//! Share & Collaborate: paths and token, events and footage in plain words (clips, not photos),
//! a fake `kopanod` over real loopback TCP (incl. a restart), and the window in the real app
//! (headless, egui_kittest) driven with clicks through the control channel.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use filmcraft_engine::Session;
use serde_json::{Value, json};

use super::kopano::*;
use super::kopano_panel::KopanoPanel;
use crate::FilmcraftApp;
use crate::control::ControlRequest;

const ALICE: &str = "c013b977c6b7b2118e21cbcb4cfd63800a5f798409997d4007fbe00b8438d344";
const M1: &str = "2635f5f796bbc17f7d7a42b10a64c795068d3cbeb4a8d6e95157b87d26300486";
const PID: &str = "aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44";
const NEW_PID: &str = "bb11bb22cc33dd44ee55ff66aa11bb22cc33dd44ee55ff66aa11bb22cc33dd44";
const TOKEN: &str = "secret-token";
const HASH_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const HASH_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";

fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
}

#[test]
fn home_folder_and_port_follow_kopanod() {
    let all = [("KOPANO_HOME", ""), ("LOCALAPPDATA", r"C:\Users\a\AppData\Local"), ("HOME", "/home/a"), ("XDG_DATA_HOME", "")];
    assert_eq!(home_with(env(&all), "windows"), Some(PathBuf::from(r"C:\Users\a\AppData\Local").join("Kopano")));
    assert_eq!(home_with(env(&all), "macos"), Some(PathBuf::from("/home/a/Library/Application Support/Kopano")));
    assert_eq!(home_with(env(&all), "linux"), Some(PathBuf::from("/home/a/.local/share/kopano")));
    assert_eq!(home_with(env(&[("HOME", "/home/a"), ("XDG_DATA_HOME", "/data")]), "linux"), Some(PathBuf::from("/data/kopano")));
    for os in ["windows", "macos", "linux"] {
        assert_eq!(home_with(env(&[("KOPANO_HOME", "/k"), ("HOME", "/home/a")]), os), Some(PathBuf::from("/k")));
    }
    assert_eq!(home_with(env(&[]), "windows"), None);
    assert_eq!(api_port_with(env(&[])), DEFAULT_API_PORT);
    assert_eq!(DEFAULT_API_PORT, 47100);
    assert_eq!(api_port_with(env(&[("KOPANO_API_PORT", " 47121 ")])), 47121);
    for bad in ["", "nope", "0", "70000"] {
        assert_eq!(api_port_with(env(&[("KOPANO_API_PORT", bad)])), DEFAULT_API_PORT, "{bad:?}");
    }
}

static N: AtomicUsize = AtomicUsize::new(0);

fn temp_home() -> PathBuf {
    let d = std::env::temp_dir().join(format!("film-collab-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn token_is_read_trimmed_and_a_missing_one_is_explained() {
    let home = temp_home();
    assert!(read_token(&home).unwrap_err().contains("missing"));
    std::fs::write(home.join("api.token"), "  ").unwrap();
    assert!(read_token(&home).unwrap_err().contains("empty"));
    std::fs::write(home.join("api.token"), "abc123\n").unwrap();
    assert_eq!(read_token(&home).unwrap(), "abc123");
}

fn state_with_project() -> State {
    let mut s = State::default();
    s.apply_hello(&json!({"device": ALICE, "name": "Alice", "account": "acct-alice", "projects": [{"id": PID, "name": "Cable way", "app": {"kind": "film-control", "port": 18601}}]}));
    s
}

/// End users see ThabangVision collaboration: never the protocol's name, and a film has clips.
fn plain(text: &str) {
    for word in ["Kopano", "kopano", "protocol", "photo"] {
        assert!(!text.contains(word), "{word:?} on screen: {text}");
    }
}

#[test]
fn events_read_as_plain_words_about_clips() {
    let mut s = state_with_project();
    s.apply_event(&json!({"event": "teammate", "project": PID, "device": M1, "name": "Member1", "online": true}), 1000.0);
    s.apply_event(&json!({"event": "synced", "project": PID, "from": "Member1", "how": "fast-forward", "clashes": 0, "photos": 2}), 2000.0);
    // Film's saves carry no count
    s.apply_event(&json!({"event": "saved", "project": PID, "head": "abcd"}), 3000.0);
    s.apply_event(&json!({"event": "synced", "project": PID, "from": "Member1", "how": "merged", "clashes": 3}), 4000.0);
    s.apply_event(&json!({"event": "synced", "project": PID, "from": "Member1", "how": "same content, settled", "clashes": 0}), 5000.0);
    s.apply_event(&json!({"event": "teammate", "project": PID, "device": M1, "online": false}), 6000.0);
    s.apply_event(&json!({"event": "app", "project": PID, "error": "control port closed"}), 7000.0);
    s.apply_event(&json!({"event": "synced", "project": PID, "from": "Member1", "how": "fast-forward"}), 7500.0);
    s.apply_event(&json!({"event": "something-new"}), 8000.0);
    let lines: Vec<String> = s.activity.iter().rev().map(|a| a.said.text()).collect();
    assert_eq!(
        lines,
        [
            "Member1 is online",
            "Member1 changed 2 clips",
            "Your edit was saved and sent",
            "Your changes and Member1's were put together · 3 clashes",
            "Up to date with Member1",
            "Member1 went offline",
            "Lost touch with this window for a moment; it reconnects by itself",
            "Member1's edit arrived",
        ]
    );
    plain(&lines.join("\n"));
    assert_eq!(s.projects[0].head.as_deref(), Some("abcd"));
    assert_eq!(s.projects[0].app_port, Some(18601));
    assert_eq!(s.projects[0].teammates, [Teammate { device: M1.into(), account: None, name: Some("Member1".into()), online: false }]);
    // the same thing again only counts
    for t in 0..5 {
        s.apply_event(&json!({"event": "synced", "project": PID, "from": "Bob", "how": "fast-forward"}), 9000.0 + f64::from(t));
    }
    assert_eq!((s.activity[0].times, s.activity[0].at_ms), (5, 9004.0));
    for t in 0..(ACTIVITY_KEEP + 10) {
        s.note(None, Said::Changed { who: format!("m{t}"), clips: None }, 0.0);
    }
    assert_eq!(s.activity.len(), ACTIVITY_KEEP);
    assert_eq!(ago(0.4), "just now");
    assert_eq!(ago(f64::NAN), "just now");
    assert_eq!(ago(125.0), "2 min ago");
}

/// The service's `media` reply for a member: one clip plays from a proxy, one original arrives,
/// one is already here.
fn media_reply() -> Value {
    json!({"files": 3, "from_original": 1, "from_proxy": 2, "waiting": 0, "items_waiting": [], "block": 7,
    "items": [
        {"key": HASH_A, "name": "C0003.MP4", "shown_from": "proxy", "original_mb": 16.78, "download": null},
        {"key": HASH_B, "name": "C0024.MP4", "shown_from": "proxy", "original_mb": 46.14,
            "download": {"event": "original", "state": "downloading", "mb_done": 12.5, "mb_total": 46.14}},
        {"key": "33", "name": "C0004.MP4", "shown_from": "original", "original_mb": 8.39, "download": {"state": "ready"}},
    ]})
}

#[test]
fn footage_says_proxy_arriving_and_here() {
    let f = Footage::from_value(&media_reply()).unwrap();
    assert_eq!(f.from_proxy, 2);
    let lines: Vec<String> = f.files.iter().map(FootageFile::line).collect();
    assert_eq!(lines, ["C0003.MP4 · plays from a proxy (16.8 MB original)", "C0024.MP4 · original arriving, 12.5 MB of 46.1 MB", "C0004.MP4 · original here"]);
    assert_eq!(f.file_named("c0003.mp4").map(FootageFile::badge_text), Some("Playing a proxy"));
    assert_eq!(f.file_named("C0024.MP4").map(FootageFile::badge_text), Some("Original arriving"));
    assert!(!f.file_named("C0004.MP4").unwrap().on_its_way());
    assert!(f.can_get_originals() && !f.all_here());
    assert_eq!(f.summary(), "1 original on the way. Exports use the originals once they are here.");
    // nothing arriving yet: the proxies summary
    let mut quiet = media_reply();
    quiet["items"][1]["download"] = Value::Null;
    assert_eq!(Footage::from_value(&quiet).unwrap().summary(), "2 files play from proxies. The originals come when you ask.");
    // a photo project's reply is not footage
    assert_eq!(Footage::from_value(&json!({"photos": 3, "from_preview": 1})), None);
    assert_eq!(mb_text(1234.0), "1.2 GB");
    assert_eq!(mb_text(f64::NAN), "0.0 MB");

    // events move the files on until the next poll, and say so
    let mut s = state_with_project();
    s.apply_media(PID, &media_reply());
    s.apply_event(&json!({"event": "media", "project": PID, "files_waiting": 3, "files_ready": 0}), 1.0);
    s.apply_event(&json!({"event": "media", "project": PID, "files_waiting": 1, "files_ready": 2, "file": "C0024.MP4"}), 2.0);
    s.apply_event(&json!({"event": "media", "project": PID, "files_waiting": 0, "files_ready": 3, "file": "C0003.MP4"}), 3.0);
    s.apply_event(
        &json!({"event": "original", "project": PID, "key": HASH_A, "file": "C0003.MP4", "state": "downloading", "mb_done": 4.0, "mb_total": 16.78}),
        4.0,
    );
    assert_eq!(s.projects[0].footage.as_ref().unwrap().files[0].arriving, Some((4.0, 16.78)));
    s.apply_event(&json!({"event": "original", "project": PID, "key": HASH_A, "state": "ready", "file": "C0003.MP4"}), 5.0);
    s.apply_event(&json!({"event": "original", "project": PID, "key": HASH_B, "state": "failed", "error": "no device has it"}), 6.0);
    let f = s.projects[0].footage.as_ref().unwrap();
    assert_eq!(f.files[0].shown_from, "original");
    assert!(f.files[1].failed && f.files[1].arriving.is_none());
    let lines: Vec<String> = s.activity.iter().rev().map(|a| a.said.text()).collect();
    assert_eq!(
        lines,
        [
            "3 footage files on the way; the clips play from proxies first",
            "3 footage files ready to play",
            "The original of C0003.MP4 is here",
            "The original of C0024.MP4 could not come yet; ask again in a moment",
        ]
    );
    plain(&lines.join("\n"));
    // the next poll finds C0024's original here after all: the line says so
    let mut later = media_reply();
    later["items"][1]["shown_from"] = json!("original");
    later["items"][1]["download"] = json!({"state": "failed"});
    s.apply_media(PID, &later);
    assert_eq!(s.activity[0].said.text(), "The original of C0024.MP4 is here");
    assert_eq!(s.footage_for_port(Some(18601)).map(|f| f.files.len()), Some(3));
    assert!(s.footage_for_port(Some(1)).is_none());
}

#[test]
fn review_counts_clips_and_tracks_not_photos() {
    let r = Review::from_value(&json!({"is_owner": true, "approver": true, "owner_name": "Alice",
        "pending": [{"account": "acct-m1", "name": "Member1", "version": "v1", "photos": 3, "summary": "changed 3 photos",
            "changes": [{"photo": "film/5/clip/16777217"}, {"photo": "film/5/clip/16777218"}, {"photo": "film/5/track/9"}],
            "clashes": [{"key": "film/5/clip/16777217#sound", "photo": "film/5/clip/16777217",
                "settings": [{"setting": "gain_db", "main": -3.0, "member": -9.0}]}]}]}));
    let e = &r.pending[0];
    assert_eq!(e.summary(), "2 clips and 1 track");
    assert_eq!(e.clashes[0].item, "film/5/clip/16777217");
    assert_eq!((e.clashes[0].main.as_str(), e.clashes[0].member.as_str()), ("-3", "-9"));
    assert_eq!(changed_text(&["film/5/clip/1".into()], 9), "1 clip");
    assert_eq!(changed_text(&["film/5/track/1".into(), "film/5/track/2".into()], 9), "2 tracks");
    assert_eq!(changed_text(&[], 4), "4 clips");
    assert_eq!(setting_text("gain_db"), "Gain");
    assert_eq!(setting_text("fx.lumetri.params.exposure.value.Float"), "Exposure");
    assert_eq!(setting_text("cut"), "Trim");
    let mine = |state: &str| Review::from_value(&json!({"owner_name": "Alice", "mine": {"state": state, "photos": 2}})).mine_text().unwrap();
    assert_eq!(mine("waiting"), "You changed 2 clips · Waiting for Alice's approval");
    assert!(mine("approved").starts_with("Approved"));
    assert_eq!(mine("declined"), "Declined: Alice kept the project as it was. Your changes stay in your window.");
    // events: new work to review, and what happened to mine
    let mut s = state_with_project();
    s.apply_event(
        &json!({"event": "review", "project": PID, "is_owner": true, "approver": true, "owner_name": "Alice",
        "pending": [{"account": "acct-m1", "name": "Member1", "version": "v1", "photos": 1, "changes": [{"photo": "film/5/clip/16777217"}]}]}),
        1000.0,
    );
    assert_eq!(s.activity[0].said.text(), "Member1 changed 1 clip · waiting for your review");
    let mut m = state_with_project();
    m.apply_event(&json!({"event": "review", "project": PID, "owner_name": "Alice", "mine": {"state": "waiting", "photos": 1}}), 1000.0);
    m.apply_event(&json!({"event": "review", "project": PID, "owner_name": "Alice", "mine": {"state": "approved", "photos": 0}}), 2000.0);
    assert_eq!(m.activity[0].said.text(), "Alice approved your changes");
    plain(&m.activity.iter().chain(s.activity.iter()).map(|a| a.said.text()).collect::<Vec<_>>().join("\n"));
}

#[test]
fn records_say_which_shared_projects_this_window_holds() {
    let home = temp_home();
    let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(read_record(&home, PID), Record::default());
    let saved = Record { share: None, file: Some(r"T:\lab\alice\cableway.fcproj".into()), footage: set(&["C0003.MP4", "C0024.MP4"]) };
    write_record(&home, PID, &saved).unwrap();
    assert_eq!(read_record(&home, PID), saved);
    // the same file (slashes as the OS writes them); never by footage names alone
    let same_file = Record { share: None, file: Some("T:/lab/alice/cableway.fcproj".into()), footage: set(&[]) };
    assert_eq!(holds(&saved, &same_file), cfg!(windows) || same_file.file == saved.file);
    assert!(!holds(&saved, &Record { share: None, file: Some("/other.fcproj".into()), footage: set(&["C0003.MP4", "C0024.MP4"]) }));
    let unsaved = Record { share: None, file: None, footage: set(&["C0003.MP4", "C0024.MP4"]) };
    // camera names such as C0003.MP4 repeat across shoots: an unsaved window never matches
    assert!(!holds(&Record { share: None, file: None, footage: set(&["C0003.MP4", "C0024.MP4"]) }, &unsaved));
    assert!(!holds(&saved, &unsaved));
    // the project's own share id counts, saved or not; another shared project's does not
    let shared = Record { share: Some(PID.into()), file: None, footage: set(&[]) };
    write_record(&home, PID, &shared).unwrap();
    assert_eq!(read_record(&home, PID), shared);
    assert!(holds(&shared, &Record { share: Some(PID.into()), file: None, footage: set(&[]) }));
    assert!(!holds(&shared, &Record { share: Some(NEW_PID.into()), file: None, footage: set(&[]) }));
    assert!(!holds(&Record::default(), &Record::default()));
    assert!(!holds(&Record::default(), &unsaved));
    // a bad id never becomes a path; junk reads as nothing
    assert!(write_record(&home, "../evil", &saved).is_err());
    std::fs::write(record_path(&home, PID).unwrap(), "junk").unwrap();
    assert_eq!(read_record(&home, PID), Record::default());
    assert_eq!(app_port(Some(&json!({"kind": "light-control", "port": 18101}))), None);
    assert_eq!(app_port(Some(&json!({"kind": "film-control", "port": 70000}))), None);
    assert_eq!(app_port(Some(&json!({"kind": "film-control", "port": 18101}))), Some(18101));
}

// ---- a fake kopanod ------------------------------------------------------------------------

#[derive(Default)]
struct FakeData {
    app_port: AtomicUsize,
    signed_out: AtomicBool,
    /// The project is "I approve changes": this side owns it (pending work), or is the member.
    approve_owner: AtomicBool,
    approve_member: AtomicBool,
    live_clash: AtomicBool,
    /// A sign-in waits in the browser (after `sign-in`, until `sign-in-cancel`).
    signing_in: AtomicBool,
    /// The service knows ThabangVision people and invites (else "unknown op", as older ones).
    tv: AtomicBool,
    /// With `tv`: this person is a member, not the owner.
    tv_member: AtomicBool,
}

struct Fake {
    got: Arc<Mutex<Vec<Value>>>,
    data: Arc<FakeData>,
    stop: Arc<AtomicBool>,
    accept: Option<std::thread::JoinHandle<()>>,
}

impl Fake {
    fn start(listener: TcpListener) -> Self {
        listener.set_nonblocking(true).unwrap();
        let got: Arc<Mutex<Vec<Value>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let data: Arc<FakeData> = Arc::default();
        let (g, st, ap) = (got.clone(), stop.clone(), data.clone());
        let accept = std::thread::spawn(move || {
            let mut workers = vec![];
            while !st.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((c, _)) => {
                        let (g, st, ap) = (g.clone(), st.clone(), ap.clone());
                        workers.push(std::thread::spawn(move || serve(c, &g, &st, &ap)));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(10)),
                }
            }
            for w in workers {
                let _ = w.join();
            }
        });
        Fake { got, data, stop, accept: Some(accept) }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(a) = self.accept.take() {
            a.join().unwrap();
        }
    }

    fn requests(&self, op: &str) -> Vec<Value> {
        self.got.lock().unwrap().iter().filter(|r| r["op"] == op).cloned().collect()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.stop();
    }
}

fn app_binding(port: &AtomicUsize) -> Value {
    match port.load(Ordering::SeqCst) {
        0 => json!({"kind": "toy"}),
        p => json!({"kind": "film-control", "port": p}),
    }
}

fn serve(c: TcpStream, got: &Mutex<Vec<Value>>, stop: &AtomicBool, data: &FakeData) {
    let app_port = &data.app_port;
    c.set_nonblocking(false).unwrap();
    c.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let mut w = c.try_clone().unwrap();
    let mut r = BufReader::new(c);
    let mut line = String::new();
    while !stop.load(Ordering::SeqCst) {
        match r.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(_) => continue,
        }
        let req: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
        line.clear();
        if req["token"] != TOKEN {
            let _ = writeln!(w, "{}", json!({"ok": false, "error": "bad token"}));
            return;
        }
        got.lock().unwrap().push(req.clone());
        let result = match req["op"].as_str().unwrap_or_default() {
            "subscribe" => {
                let _ = writeln!(w, "{}", json!({"ok": true, "result": "subscribed"}));
                for ev in [
                    json!({"event": "teammate", "project": PID, "device": M1, "name": "Member1", "online": true}),
                    json!({"event": "synced", "project": PID, "from": "Member1", "how": "fast-forward", "clashes": 0, "photos": 2, "head": "1234"}),
                ] {
                    let _ = writeln!(w, "{ev}");
                }
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return;
            }
            "hello" if data.signed_out.load(Ordering::SeqCst) => json!({"device": ALICE, "name": "", "account": "", "signed_in": false, "projects": [],
                "signing_in": if data.signing_in.load(Ordering::SeqCst) { json!({"code": "BCDF-1234", "link": "http://localhost:3100/link?code=BCDF-1234", "until_ms": 1.8e12}) } else { Value::Null },
                "message": "Sign-in was not approved in the browser."}),
            "hello" => json!({"device": ALICE, "name": "Alice", "account": "acct-alice", "signed_in": !data.signed_out.load(Ordering::SeqCst),
                "signed_in_until_ms": 1.8e12, "projects": [{"id": PID, "name": "Cable way", "app": app_binding(app_port)}]}),
            "sign-in" => {
                data.signing_in.store(true, Ordering::SeqCst);
                json!({"code": "BCDF-1234", "link": "http://localhost:3100/link?code=BCDF-1234", "until_ms": 1.8e12})
            }
            "sign-in-cancel" => {
                data.signing_in.store(false, Ordering::SeqCst);
                json!("not signing in")
            }
            "sign-out" => {
                data.signed_out.store(true, Ordering::SeqCst);
                json!("signed out")
            }
            "people" if data.tv.load(Ordering::SeqCst) => {
                let member = data.tv_member.load(Ordering::SeqCst);
                json!({"shared": "tv-1", "people": [
                    {"user": "u-alice", "name": "Alice", "role": "owner", "status": "active", "me": !member, "online": true, "on_project": true},
                    {"user": "u-m1", "name": "Member1", "role": "editor", "status": "active", "me": member, "online": true, "on_project": true},
                    {"user": "u-bob", "name": "Bob", "role": "editor", "status": "invited", "me": false, "online": false, "on_project": false}]})
            }
            "shared-projects" if data.tv.load(Ordering::SeqCst) => json!({"mine": [{"shared": "tv-1", "name": "Cable way", "project": PID, "role": "owner", "status": "active", "here": true}],
                "shared": [{"shared": "tv-2", "name": "Sound mix", "project": NEW_PID, "role": "editor", "status": "invited", "here": false},
                           {"shared": "tv-3", "name": "Already here", "project": PID, "role": "editor", "status": "active", "here": true}]}),
            "invite" => json!({"message": "Invite sent."}),
            "join-shared" => json!({"project": NEW_PID, "name": "Sound mix"}),
            "leave" => json!({"left": true}),
            "remove-person" => json!({"removed": true}),
            "status" => {
                let mut st = json!({"head": "1234abcd", "name": "Cable way", "app": app_binding(app_port),
                    "teammates": [{"device": M1, "name": "Member1", "account": "acct-m1", "online": true}], "online": {M1: "Member1"},
                    "members": [{"account": "acct-alice", "role": "editor", "owner": true}, {"account": "acct-m1", "role": "editor", "owner": false}]});
                if data.approve_owner.load(Ordering::SeqCst) {
                    st["mode"] = json!("approve");
                    st["review"] = json!({"is_owner": true, "approver": true, "owner_name": "Alice", "declined": [], "mine": null, "viewing": null,
                        "pending": [{"account": "acct-m1", "name": "Member1", "version": "v1", "photos": 2, "summary": "changed 2 photos",
                            "changes": [{"photo": "film/5/clip/16777217", "fields": ["gain"]}, {"photo": "film/5/clip/16777218", "fields": ["trimmed"]}],
                            "clashes": [{"key": "film/5/clip/16777217#sound", "photo": "film/5/clip/16777217", "fields": ["gain"],
                                "settings": [{"setting": "gain_db", "main": -3.0, "member": -9.0}]}]}]});
                } else if data.approve_member.load(Ordering::SeqCst) {
                    st["mode"] = json!("approve");
                    st["review"] = json!({"is_owner": false, "approver": false, "owner_name": "Alice", "declined": [], "pending": [], "viewing": null,
                        "mine": {"state": "waiting", "photos": 1}});
                } else if data.live_clash.load(Ordering::SeqCst) {
                    st["mode"] = json!("live");
                    st["clashes"] = json!([{"clash": "kopano/conflict/k/11-22", "key": "film/5/clip/7#sound", "photo": "film/5/clip/7", "fields": ["gain"],
                        "settings": [{"setting": "gain_db", "a": -6.0, "b": -9.5}]}]);
                }
                st
            }
            "media" => media_reply(),
            "fetch-original" => json!({"started": 2, "keys": [HASH_A, HASH_B]}),
            "approve" => json!({"main": "m2", "clashes_decided": 1}),
            "decline" => json!({"declined": "v1"}),
            "view" => json!({"viewing": {"side": req["show"]}}),
            "resolve" => json!({"settled": req["clash"]}),
            "create-project" => json!({"project": NEW_PID, "head": "99", "items": 15, "photos": 9, "mode": req["mode"]}),
            "attach" => {
                app_port.store(req["app"]["port"].as_u64().unwrap_or(0) as usize, Ordering::SeqCst);
                json!("attached")
            }
            "join" => json!({"project": req["project"]}),
            _ => {
                let _ = writeln!(w, "{}", json!({"ok": false, "error": "unknown op"}));
                continue;
            }
        };
        let _ = writeln!(w, "{}", json!({"ok": true, "result": result}));
    }
}

fn wait(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let t0 = Instant::now();
    while !done() {
        assert!(t0.elapsed() < timeout, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn fake_service() -> (Fake, Config, PathBuf) {
    let home = temp_home();
    std::fs::write(home.join("api.token"), format!("{TOKEN}\n")).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    (Fake::start(listener), Config { port, home: Some(home.clone()) }, home)
}

const T: Duration = Duration::from_secs(20);

#[test]
fn client_follows_a_fake_service_and_finds_it_again_after_a_restart() {
    let (mut fake, config, _home) = fake_service();
    fake.data.app_port.store(18601, Ordering::SeqCst);
    let mut client = Client::new(config.clone());
    client.start(&egui::Context::default());
    wait("hello, status and media", T, || {
        let s = client.state();
        s.running
            && s.me.as_ref().is_some_and(|m| m.name == "Alice")
            && s.projects.first().is_some_and(|p| p.teammates.iter().any(|t| t.online) && p.footage.as_ref().is_some_and(|f| f.files.len() == 3))
    });
    wait("events", T, || client.state().activity.iter().any(|a| a.said.text() == "Member1 changed 2 clips"));
    client
        .request(
            &egui::Context::default(),
            "Sharing…",
            json!({"op": "create-project", "name": "Cut 2", "app": {"kind": "film-control", "port": 18123}, "members": [{"account": "acct-m1", "role": "editor", "lanes": []}]}),
        )
        .unwrap();
    wait("the share", T, || client.state().job.is_some_and(|j| !j.busy));
    let job = client.state().job.unwrap();
    assert!(job.ok, "{job:?}");
    assert_eq!(job.project.as_deref(), Some(NEW_PID));
    assert_eq!(job.text, "Shared this project. Invite sent.");
    plain(&job.text);
    assert_eq!(fake.requests("create-project")[0]["token"], TOKEN);

    fake.stop();
    wait("not running", T, || client.state().problem.as_deref() == Some(NOT_RUNNING));
    assert!(client.state().projects[0].teammates.iter().all(|t| !t.online));
    let fake2 = Fake::start(TcpListener::bind(("127.0.0.1", config.port)).unwrap());
    wait("running again", T, || client.state().running);
    wait("events again", T, || !fake2.requests("subscribe").is_empty());
}

#[test]
fn errors_on_screen_are_plain_words() {
    for raw in [
        r"the collaboration key file is missing (C:\Users\a\AppData\Local\Kopano\api.token)",
        "the collaboration service refused this window (its key file api.token does not match)",
        "the collaboration folder is unknown here (set KOPANO_HOME)",
        "could not start collaboration: The system cannot find the file specified. (os error 2)",
        "no addressing information for c013b977",
        "This window can't be shared: it has no control port (start Ubunye Film with --control PORT)",
        "unknown op",
        "project missing",
        "store: object 1234 not found under /home/a/.local/share/kopano/projects",
    ] {
        let p = for_people(raw);
        plain(&p);
        assert!(!p.contains('/') && !p.contains('\\') && !p.contains("api.token") && !p.contains("--control"), "{raw} -> {p}");
    }
    // the window's own messages are plain already
    for own in ["Type your teammate's email, so they can join", "Shared this project. Invite sent.", NOT_RUNNING] {
        assert_eq!(for_people(own), own);
    }
}

#[test]
fn a_wrong_key_file_shows_no_path_on_screen() {
    let (_fake, config, home) = fake_service();
    std::fs::write(home.join("api.token"), "other").unwrap();
    let mut d = Driver::new(config, None);
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("the refusal", |s| s.contains("could not open collaboration on this computer"));
    let s = d.screen();
    plain(&s);
    assert!(!s.contains("api.token") && !s.contains(&home.to_string_lossy().to_string()), "{s}");
}

#[test]
fn a_wrong_token_is_explained() {
    let (_fake, config, home) = fake_service();
    std::fs::write(home.join("api.token"), "other").unwrap();
    let mut client = Client::new(config);
    client.start(&egui::Context::default());
    wait("the refusal", T, || client.state().problem.is_some());
    assert!(!client.state().running);
    assert!(client.state().problem.unwrap().contains("api.token"));
}

// ---- the window in the real app, headless --------------------------------------------------

struct Driver {
    h: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
    /// The app (demo project) with the panel's client pointed at `config`; `control`: the port
    /// the start-on-demand hook hands out (counted in `started`).
    fn new(config: Config, control: Option<(u16, Arc<AtomicUsize>)>) -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).unwrap();
        let (tx, rx) = channel();
        let mut app = FilmcraftApp::new(session).with_control(rx);
        app.kopano = KopanoPanel::with_config(config);
        if let Some((port, started)) = control {
            app.kopano.start_control = Some(Box::new(move || {
                started.fetch_add(1, Ordering::SeqCst);
                Ok((port, None))
            }));
        }
        let h = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { h, tx };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.h.ctx.clone();
            let mut raw = std::mem::take(self.h.input_mut());
            eframe::App::raw_input_hook(self.h.state_mut(), &ctx, &mut raw);
            *self.h.input_mut() = raw;
            self.h.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let v = self.call(method, params.clone());
        assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
        v["result"].clone()
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.call("engine.execute", json!({"command": command, "params": params}))
    }

    /// A real click on a registered element (synthetic pointer input through egui).
    fn click(&mut self, id: &str) {
        if std::env::var_os("COLLAB_DEBUG").is_some() {
            let els = self.ok("ui.elements", json!({"prefix": "collab."}));
            eprintln!("click {id}: {els}");
        }
        self.ok("ui.click", json!({"id": id}));
        self.frames(2);
    }

    /// Click `id` until `done` (a click can land while the window's layout still moves under
    /// a busy test machine: a person would click again).
    fn click_until(&mut self, id: &str, mut done: impl FnMut() -> bool) {
        let t0 = Instant::now();
        while !done() {
            assert!(t0.elapsed() < T, "clicking {id} did nothing");
            self.click(id);
            for _ in 0..20 {
                if done() {
                    return;
                }
                self.frames(1);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn type_into(&mut self, id: &str, text: &str) {
        self.click(id);
        self.ok("ui.key", json!({"key": "A", "command": true}));
        self.ok("ui.type", json!({"text": text}));
        self.frames(2);
    }

    /// Every text on screen (accessibility labels), one per line.
    fn screen(&self) -> String {
        let out = RefCell::new(Vec::<String>::new());
        let _ = self
            .h
            .query_all_by(|n| {
                // buttons carry their text as the label, plain labels as the value
                for l in [n.label(), n.value()].into_iter().flatten() {
                    out.borrow_mut().push(l);
                }
                false
            })
            .count();
        out.into_inner().join("\n")
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&str) -> bool) {
        let t0 = Instant::now();
        loop {
            self.frames(1);
            let s = self.screen();
            if done(&s) {
                return;
            }
            assert!(t0.elapsed() < T, "timed out waiting for {what}; on screen:\n{s}");
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.h.state_mut()
    }
}

#[test]
fn window_says_when_collaboration_is_not_running_and_opens_from_the_header_and_menu() {
    let home = temp_home();
    let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let mut d = Driver::new(Config { port, home: Some(home) }, None);
    d.click("header.collaborate");
    d.until("not running", |s| s.contains(NOT_RUNNING));
    let s = d.screen();
    assert!(s.contains("Share & Collaborate") && s.contains("Share this project") && s.contains("Start ThabangVision collaboration"), "{s}");
    plain(&s);
    // Join is under Advanced (testing): closed, it isn't on screen
    assert!(!s.contains("Join a shared project"));
    d.click("collab.advanced");
    d.until("advanced", |s| s.contains("Join a shared project"));
    d.frames(30);
    d.click("collab.join");
    d.until("the id check", |s| s.contains("The project id is 64 letters"));
    // Window menu lists it; the command closes it again
    let menus = d.ok("ui.menu.list", json!({}));
    assert!(
        menus.as_array().unwrap().iter().any(|m| m["id"] == "kopano.panel" && m["path"][0] == "Window" && m["label"] == "Share & Collaborate"),
        "{}",
        menus.as_array().unwrap().iter().filter(|m| m["id"] == "kopano.panel").cloned().collect::<Vec<_>>().len()
    );
    assert_eq!(d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}))["open"], false);
    d.frames(2);
    assert!(!d.screen().contains(NOT_RUNNING));
    // the engine's own kopano commands still work through the same dispatcher
    let block = d.exec("kopano.idBlock", json!({}));
    assert_eq!(block["ok"], true, "{block}");
    // sharing without a control port says how to get one
    let r = d.exec("kopano.share", json!({"name": "x", "account": "acct-m1"}));
    assert!(r["error"].as_str().unwrap().contains("--control"), "{r}");
    let r = d.exec("kopano.share", json!({"name": "", "account": "acct-m1"}));
    assert!(r["error"].as_str().unwrap().contains("name"), "{r}");
    let r = d.exec("kopano.share", json!({"name": "x", "email": ""}));
    assert!(r["error"].as_str().unwrap().contains("email"), "{r}");
}

#[test]
fn without_a_sign_in_the_window_offers_one() {
    let (fake, config, _home) = fake_service();
    fake.data.signed_out.store(true, Ordering::SeqCst);
    let mut d = Driver::new(config, None);
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("sign-in", |s| s.contains("Sign in with ThabangVision"));
    // why the last sign-in ended, in the service's own sentence
    d.until("the last message", |s| s.contains("Sign-in was not approved in the browser."));
    d.click_until("collab.signIn", || fake.requests("sign-in").len() == 1);
    d.until("the code", |s| s.contains("Check that the browser shows") && s.contains("BCDF-1234") && s.contains("Open the page again"));
    let s = d.screen();
    plain(&s);
    assert!(!s.contains("Shared projects") && !s.contains("Share this project"), "nothing else before the sign-in: {s}");
    d.click_until("collab.signInCancel", || fake.requests("sign-in-cancel").len() == 1);
    d.until("back to Sign in", |s| s.contains("Sign in with ThabangVision") && !s.contains("BCDF-1234"));
}

#[test]
fn signed_in_the_window_signs_out() {
    let (fake, config, _home) = fake_service();
    let mut d = Driver::new(config, None);
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("signed in", |s| s.contains("Signed in to ThabangVision as Alice"));
    d.click_until("collab.signOut", || fake.requests("sign-out").len() == 1);
    d.until("signed out", |s| s.contains("Sign in with ThabangVision"));
}

#[test]
fn the_owner_sees_people_by_name_invites_by_email_and_takes_someone_off() {
    let (fake, config, _home) = fake_service();
    fake.data.tv.store(true, Ordering::SeqCst);
    let mut d = Driver::new(config, None);
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("the people", |s| s.contains("Member1") && s.contains("can edit") && s.contains("Bob") && s.contains("invited"));
    let s = d.screen();
    plain(&s);
    assert!(!s.contains('@') && !s.contains("u-m1"), "no emails or user ids: {s}");
    let tag = short(PID).to_string();
    d.type_into(&format!("collab.inviteEmail.{tag}"), "bob@example.com");
    d.click_until(&format!("collab.invite.{tag}"), || fake.requests("invite").len() == 1);
    let inv = &fake.requests("invite")[0];
    assert_eq!((inv["project"].as_str(), inv["email"].as_str()), (Some(PID), Some("bob@example.com")));
    d.until("invite sent", |s| s.contains("Invite sent.") && !s.contains("bob@example.com"));
    d.click_until(&format!("collab.remove.{tag}.u-bob"), || fake.requests("remove-person").len() == 1);
    assert_eq!(fake.requests("remove-person")[0]["user"], "u-bob");
    d.until("taken off", |s| s.contains("Taken off the project."));
    // no Remove for themselves, no Leave for the owner
    assert!(d.ok("ui.elements", json!({"prefix": "collab."})).to_string().contains("remove.") && !d.screen().contains("Leave this project"));
}

#[test]
fn a_member_opens_a_project_shared_with_them_and_can_leave() {
    let (fake, config, _home) = fake_service();
    fake.data.tv.store(true, Ordering::SeqCst);
    fake.data.tv_member.store(true, Ordering::SeqCst);
    let mut d = Driver::new(config, Some((18999, Arc::default())));
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("shared with me", |s| s.contains("Shared with me") && s.contains("Sound mix") && s.contains("Open here") && !s.contains("Already here"));
    d.click_until("collab.openShared.tv-2", || fake.requests("join-shared").len() == 1);
    let j = &fake.requests("join-shared")[0];
    assert_eq!(j["shared"], "tv-2");
    assert_eq!(j["app"], json!({"kind": "film-control", "port": 18999}));
    d.until("joined", |s| s.contains("Joined."));
    let tag = short(PID).to_string();
    d.until("leave", |s| s.contains("Leave this project"));
    assert!(!d.ok("ui.elements", json!({"prefix": "collab."})).to_string().contains("collab.remove."), "a member takes no one off");
    d.click_until(&format!("collab.leave.{tag}"), || fake.requests("leave").len() == 1);
    assert_eq!(fake.requests("leave")[0]["project"], PID);
    d.until("left", |s| s.contains("You left the project."));
}

#[test]
fn sharing_with_an_email_sends_an_invite_after_the_project_is_made() {
    let (fake, config, _home) = fake_service();
    fake.data.tv.store(true, Ordering::SeqCst);
    let mut d = Driver::new(config, Some((18999, Arc::default())));
    let r = d.exec("kopano.share", json!({"name": "Cut 3", "email": "m1@example.com, acct-lab"}));
    assert_eq!(r["ok"], true, "{r}");
    wait("the invite", T, || fake.requests("invite").len() == 1);
    let made = &fake.requests("create-project")[0];
    assert_eq!(made["members"], json!([{"account": "acct-lab", "role": "editor", "lanes": []}]));
    let inv = &fake.requests("invite")[0];
    assert_eq!((inv["project"].as_str(), inv["email"].as_str()), (Some(NEW_PID), Some("m1@example.com")));
    wait("done", T, || d.app().kopano.client.state().job.is_some_and(|j| !j.busy));
    assert_eq!(d.app().kopano.client.state().job.unwrap().text, "Shared this project. Invite sent.");
}

#[test]
fn share_this_project_through_the_fields_with_the_control_port_started_on_demand() {
    let (fake, config, _home) = fake_service();
    let started = Arc::new(AtomicUsize::new(0));
    let mut d = Driver::new(config, Some((18999, started.clone())));
    d.click("header.collaborate");
    d.until("signed in and the teammate", |s| {
        s.contains("Signed in to ThabangVision as Alice") && s.contains("Member1") && s.contains("online") && s.contains("Member1 changed 2 clips")
    });
    let s = d.screen();
    plain(&s);
    assert!(!s.contains("c013b977") && !s.contains(&PID[..8]), "no ids while Advanced is closed: {s}");
    // the name starts as the project's name
    let project_name = d.app().session.project.name.clone();
    assert_eq!(d.app().kopano.share_name, project_name);
    d.ok("engine.execute", json!({"command": "kopano.panel", "params": {"open": true, "advanced": true}}));
    d.until("ids", |s| s.contains("c013b977"));
    d.frames(30);
    d.click("collab.copyDevice");
    assert_eq!(d.app().kopano.copied.as_deref(), Some(ALICE));

    assert!(d.screen().contains("Teammate's email"), "{}", d.screen());
    d.type_into("collab.shareName", "Cable way");
    d.type_into("collab.shareAccount", "acct-m1");
    // "I approve changes" is the default way to work
    d.click("collab.share");
    wait("the share", T, || fake.requests("create-project").len() == 1);
    let made = &fake.requests("create-project")[0];
    assert_eq!(made["name"], "Cable way");
    assert_eq!(made["mode"], "approve");
    assert_eq!(made["app"], json!({"kind": "film-control", "port": 18999}));
    assert_eq!(made["members"], json!([{"account": "acct-m1", "role": "editor", "lanes": []}]));
    d.until("shared", |s| s.contains("Shared this project. Invite sent."));
    // live, then Join with the same control port (started once)
    d.click("collab.modeLive");
    d.until("live words", |s| s.contains("Every edit goes to everyone at once."));
    d.click("collab.share");
    wait("the second share", T, || fake.requests("create-project").len() == 2);
    assert_eq!(fake.requests("create-project")[1]["mode"], "live");
    d.frames(5);
    d.type_into("collab.joinProject", PID);
    d.type_into("collab.joinFrom", M1);
    d.click("collab.join");
    wait("the join", T, || fake.requests("join").len() == 1);
    assert_eq!(fake.requests("join")[0]["app"], json!({"kind": "film-control", "port": 18999}));
    assert_eq!(fake.requests("join")[0]["from"], M1);
    assert_eq!(started.load(Ordering::SeqCst), 1);
    let status = d.exec("kopano.status", json!({}));
    assert_eq!(status["result"]["controlPort"], 18999);
    assert_eq!(status["result"]["running"], true);
}

#[test]
fn the_owner_reviews_clips_views_approves_and_declines_with_clicks() {
    let (fake, config, _home) = fake_service();
    fake.data.approve_owner.store(true, Ordering::SeqCst);
    let mut d = Driver::new(config, Some((18999, Arc::default())));
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("the review", |s| s.contains("Changes to review") && s.contains("Member1 changed 2 clips") && s.contains("You approve changes"));
    let s = d.screen();
    plain(&s);
    assert!(s.contains("Gain") && s.contains("Keep yours") && s.contains("Take theirs"), "{s}");
    let tag = short(PID).to_string();
    d.click_until(&format!("collab.view.{tag}.acct-m1"), || fake.requests("view").len() == 1);
    assert_eq!(fake.requests("view")[0]["show"], "member");
    d.until("view done", |s| s.contains("Showing their version of the clips"));
    d.click(&format!("collab.keepMain.{tag}.acct-m1.0"));
    d.click_until(&format!("collab.approve.{tag}.acct-m1"), || fake.requests("approve").len() == 1);
    let a = &fake.requests("approve")[0];
    assert_eq!((a["member"].as_str(), a["version"].as_str()), (Some("acct-m1"), Some("v1")));
    assert_eq!(a["choices"], json!({"film/5/clip/16777217#sound": "main"}));
    d.until("approved", |s| s.contains("Approved. The changes are in your project."));
    d.click_until(&format!("collab.decline.{tag}.acct-m1"), || fake.requests("decline").len() == 1);
    d.until("declined", |s| s.contains("Declined. Your project stays as it was."));
}

#[test]
fn two_members_clashing_on_one_setting_keep_their_own_picks() {
    let (fake, config, _home) = fake_service();
    fake.data.approve_owner.store(true, Ordering::SeqCst);
    let mut d = Driver::new(config, Some((18999, Arc::default())));
    let key = "film/5/clip/16777217#sound";
    assert_eq!(d.exec("kopano.choose", json!({"project": PID, "member": "acct-m1", "key": key, "pick": "main"}))["ok"], true);
    assert_eq!(d.exec("kopano.choose", json!({"project": PID, "member": "acct-m2", "key": key, "pick": "member"}))["ok"], true);
    // a pick says whose changes it is for
    assert!(d.exec("kopano.choose", json!({"project": PID, "key": key, "pick": "main"}))["error"].is_string());
    assert_eq!(d.exec("kopano.approve", json!({"project": PID, "member": "acct-m1", "version": "v1"}))["ok"], true);
    wait("the first approve", T, || fake.requests("approve").len() == 1);
    assert_eq!(fake.requests("approve")[0]["choices"], json!({key: "main"}));
    wait("the first approve done", T, || d.app().kopano.client.state().job.is_some_and(|j| !j.busy));
    // approving Member1 used up only Member1's pick
    assert_eq!(d.exec("kopano.approve", json!({"project": PID, "member": "acct-m2", "version": "v2"}))["ok"], true);
    wait("the second approve", T, || fake.requests("approve").len() == 2);
    assert_eq!(fake.requests("approve")[1]["choices"], json!({key: "member"}));
}

#[test]
fn a_member_sees_their_changes_and_the_footage_and_gets_the_originals() {
    let (fake, config, _home) = fake_service();
    fake.data.approve_member.store(true, Ordering::SeqCst);
    fake.data.app_port.store(18999, Ordering::SeqCst);
    let mut d = Driver::new(config, Some((18999, Arc::default())));
    d.app().kopano.control_port = Some(18999);
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("his line and the footage", |s| s.contains("Waiting for Alice's approval") && s.contains("C0003.MP4 · plays from a proxy"));
    let s = d.screen();
    assert!(s.contains("Alice approves changes") && !s.contains("Changes to review") && s.contains("in this window"), "{s}");
    assert!(s.contains("You changed 1 clip") && s.contains("Footage") && s.contains("1 original on the way"), "{s}");
    // a window that already is a shared project offers no second share
    assert!(s.contains("This window is part of Cable way") && !s.contains("Name for the shared project"), "{s}");
    plain(&s);
    d.click_until(&format!("collab.getOriginals.{}", short(PID)), || fake.requests("fetch-original").len() == 1);
    assert_eq!(fake.requests("fetch-original")[0]["key"], "all");
    assert_eq!(fake.requests("fetch-original")[0]["project"], PID);
    d.until("getting", |s| s.contains("Getting 2 originals."));

    // the Project panel: a missing clip whose footage is coming says so instead of "Media missing"
    let app = d.app();
    let id = *app.session.project.items.iter().find(|(_, it)| it.as_media().is_some()).map(|(id, _)| id).unwrap();
    {
        let p = Arc::make_mut(&mut app.session.project);
        let it = p.items.get_mut(&id).unwrap();
        it.name = "C0003.MP4".into();
        if let Some(m) = it.as_media_mut() {
            m.media = filmcraft_project::MediaRef::File { path: "T:/nowhere/C0003.MP4".into() };
        }
    }
    app.session.offline.missing.push(id);
    let it = app.session.project.item(id).unwrap().clone();
    let b = crate::panels::project::media_badge(app, &it);
    assert!(b.offline && b.coming, "on its way");
    assert_eq!(b.offline_text, "Playing a proxy");
    // without collaboration it is Film's own words
    app.kopano.control_port = None;
    let b = crate::panels::project::media_badge(app, &it);
    assert_eq!((b.offline_text, b.coming), ("Media missing", false));
    // and the Link Media prompt for footage on its way is put away
    app.kopano.control_port = Some(18999);
    app.session.offline.prompt = true;
    let t0 = Instant::now();
    while d.app().session.offline.prompt {
        assert!(t0.elapsed() < T, "the Link Media prompt stayed");
        d.frames(1);
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(d.app().ui.link_media.is_none());
    assert!(d.app().ui.status.contains("The footage is on its way"), "{}", d.app().ui.status);
}

#[test]
fn a_live_clash_offers_both_values() {
    let (fake, config, _home) = fake_service();
    fake.data.live_clash.store(true, Ordering::SeqCst);
    let mut d = Driver::new(config, None);
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("the clash", |s| s.contains("Pick the one to keep"));
    let s = d.screen();
    assert!(s.contains("-9.50") && s.contains("Works live") && s.contains("Gain"), "{s}");
    d.click_until(&format!("collab.pickB.{}.0", short(PID)), || fake.requests("resolve").len() == 1);
    assert_eq!(fake.requests("resolve")[0]["pick"], "b");
    plain(&d.screen());
}

#[test]
fn an_unsaved_window_with_the_same_camera_names_is_linked_only_by_the_person() {
    let (fake, config, home) = fake_service();
    let dead = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    fake.data.app_port.store(usize::from(dead), Ordering::SeqCst);
    let mut d = Driver::new(config, None);
    d.app().kopano.control_port = Some(18777);
    // the record names footage this unsaved window also has (another shoot, same camera names)
    write_record(&home, PID, &Record { share: None, file: None, footage: ["C0003.MP4".to_string()].into() }).unwrap();
    {
        let app = d.app();
        let id = *app.session.project.items.iter().find(|(_, it)| it.as_media().is_some()).map(|(id, _)| id).unwrap();
        let p = Arc::make_mut(&mut app.session.project);
        if let Some(m) = p.items.get_mut(&id).and_then(|it| it.as_media_mut()) {
            m.media = filmcraft_project::MediaRef::File { path: "/other-shoot/C0003.MP4".into() };
        }
    }
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    d.until("the link offer", |s| s.contains("No window has it open") && s.contains("Link this window"));
    for _ in 0..40 {
        d.frames(1);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(fake.requests("attach").is_empty(), "linked by itself: {:?}", fake.requests("attach"));
    let tag = short(PID).to_string();
    d.click_until(&format!("collab.link.{tag}"), || !fake.requests("attach").is_empty());
    assert_eq!(fake.requests("attach")[0]["app"], json!({"kind": "film-control", "port": 18777}));
}

#[test]
fn an_unsaved_window_carrying_the_share_id_is_linked_again_after_a_restart() {
    let (fake, config, home) = fake_service();
    let dead = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    fake.data.app_port.store(usize::from(dead), Ordering::SeqCst);
    let mut d = Driver::new(config, None);
    d.app().kopano.control_port = Some(18777);
    write_record(&home, PID, &Record { share: Some(PID.into()), file: None, footage: Default::default() }).unwrap();
    // crash recovery brought the unsaved project back with its share id
    Arc::make_mut(&mut d.app().session.project).share_id = Some(PID.into());
    let t0 = Instant::now();
    while fake.data.app_port.load(Ordering::SeqCst) != 18777 {
        assert!(t0.elapsed() < T, "attach not sent: {:?}", fake.requests("attach"));
        d.frames(1);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_linked_window_stamps_the_share_id_on_its_project_and_old_files_load_without_it() {
    let (fake, config, home) = fake_service();
    fake.data.app_port.store(18999, Ordering::SeqCst);
    let mut d = Driver::new(config, Some((18999, Arc::default())));
    d.app().kopano.control_port = Some(18999);
    assert_eq!(d.app().session.project.share_id, None);
    let t0 = Instant::now();
    while d.app().session.project.share_id.as_deref() != Some(PID) {
        assert!(t0.elapsed() < T, "no share id on the project");
        d.frames(1);
        std::thread::sleep(Duration::from_millis(20));
    }
    // it is saved with the project, and a file from before loads with none
    let file = home.join("cut.fcproj").to_string_lossy().to_string();
    assert_eq!(d.exec("file.saveAs", json!({"path": file}))["ok"], true);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains(PID), "share id not saved");
    let mut s = Session::default();
    s.execute("file.open", json!({"path": file})).unwrap();
    assert_eq!(s.project.share_id.as_deref(), Some(PID));
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let removed = v.pointer_mut("/project").and_then(Value::as_object_mut).and_then(|p| p.remove("share_id"));
    assert!(removed.is_some(), "share_id is not where expected");
    std::fs::write(&file, v.to_string()).unwrap();
    let mut s = Session::default();
    s.execute("file.open", json!({"path": file})).unwrap();
    assert_eq!(s.project.share_id, None);
}

#[test]
fn opening_another_project_keeps_the_shared_one_with_its_own_file() {
    let (fake, config, home) = fake_service();
    fake.data.app_port.store(18999, Ordering::SeqCst);
    let mut d = Driver::new(config, Some((18999, Arc::default())));
    d.app().kopano.control_port = Some(18999);
    let file = home.join("cableway.fcproj").to_string_lossy().to_string();
    assert_eq!(d.exec("file.saveAs", json!({"path": file}))["ok"], true);
    d.ok("ui.menu.invoke", json!({"command": "kopano.panel"}));
    let recorded = |home: &std::path::Path| read_record(home, PID).file;
    let t0 = Instant::now();
    while recorded(&home).as_deref() != Some(file.as_str()) {
        assert!(t0.elapsed() < T, "no record of the shared project's file");
        d.frames(1);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(d.exec("kopano.items", json!({}))["ok"], true);
    // the person opens another project in this window
    assert_eq!(d.exec("file.newProject", json!({"name": "Holiday"}))["ok"], true);
    // at once, before the panel looks: the service may not read or change this project
    for cmd in ["kopano.items", "kopano.apply", "kopano.idBlock"] {
        let r = d.exec(cmd, json!({}));
        assert_eq!(r["ok"], false, "{cmd}: {r}");
    }
    d.until("the plain line", |s| s.contains("Another project is open in this window. Nothing of it is shared."));
    plain(&d.screen());
    for _ in 0..30 {
        d.frames(1);
    }
    // the shared project's record still names its own file, not the other project
    assert_eq!(recorded(&home).as_deref(), Some(file.as_str()));
    assert!(fake.requests("attach").is_empty());
    // opening the shared project's file again links it again
    assert_eq!(d.exec("file.open", json!({"path": file}))["ok"], true);
    d.until("linked again", |s| s.contains("This window is linked to Cable way again") && !s.contains("Nothing of it is shared"));
    assert_eq!(d.exec("kopano.items", json!({}))["ok"], true);
}

#[test]
fn after_a_restart_the_window_links_its_shared_project_again() {
    let (fake, config, home) = fake_service();
    // the project was bound to a port nobody answers on any more (Film restarted)
    let dead = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    fake.data.app_port.store(usize::from(dead), Ordering::SeqCst);
    let mut d = Driver::new(config, None);
    d.app().kopano.control_port = Some(18777);
    // this window has the shared project's file open
    let file = home.join("cableway.fcproj").to_string_lossy().to_string();
    assert_eq!(d.exec("file.saveAs", json!({"path": file}))["ok"], true);
    write_record(&home, PID, &Record { share: None, file: Some(file), footage: Default::default() }).unwrap();
    let mut attached = false;
    let t0 = Instant::now();
    while t0.elapsed() < T {
        d.frames(1);
        if fake.data.app_port.load(Ordering::SeqCst) == 18777 {
            attached = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(attached, "attach not sent: {:?}", fake.requests("attach"));
    assert_eq!(fake.requests("attach")[0]["app"], json!({"kind": "film-control", "port": 18777}));
    let t0 = Instant::now();
    while !d.app().kopano.client.state().activity.iter().any(|a| a.said.text() == "This window is linked to Cable way again") {
        assert!(t0.elapsed() < T, "no relink line");
        d.frames(1);
    }
    for _ in 0..60 {
        d.frames(1);
    }
    assert_eq!(fake.requests("attach").len(), 1, "attached once");
}
