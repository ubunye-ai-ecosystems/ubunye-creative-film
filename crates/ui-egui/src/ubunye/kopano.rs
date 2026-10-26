//! Client for `kopanod`, the collaboration service on this computer (Ubunye, Kopano).
//!
//! One service per computer holds the device identity and does all the syncing; Film never syncs
//! itself. Film only talks to the service's local API: TCP on 127.0.0.1, one JSON request per
//! line, each carrying the token from `<home>/api.token`. The service drives Film back through
//! Film's own control port (`docs/control-protocol.md`), binding `{"kind": "film-control", port}`.
//!
//! Everything here runs off the UI thread: one thread polls `hello`, `status` and `media` every
//! few seconds, one holds a `subscribe` stream of events, and Share / Join / Approve / Get
//! originals each run on a thread of their own. The UI reads a [`State`] snapshot behind a mutex
//! and never waits for the service. When the service stops, both threads keep retrying, so a
//! restarted service is found again by itself.
//!
//! Footage: a teammate's clips first play from small proxies the owner's computer makes; the
//! originals come when someone asks (`fetch-original`). [`Footage`] is what the service says
//! about each file, so the panel and the Project panel can say "on its way" instead of
//! "Media missing".
//!
//! Defaults shared with `kopanod`: API port `KOPANO_API_PORT`, else 47100; home `KOPANO_HOME`,
//! else `%LOCALAPPDATA%\Kopano` (Windows), `~/Library/Application Support/Kopano` (macOS),
//! `~/.local/share/kopano` (Linux, or `$XDG_DATA_HOME/kopano`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::{Value, json};

/// The service's API port when `KOPANO_API_PORT` doesn't say otherwise.
pub const DEFAULT_API_PORT: u16 = 47100;
/// How often the poll thread asks the service for `hello`, `status` and `media`.
pub const POLL_MS: u64 = 3000;
/// Recent activity lines kept (newest first).
pub const ACTIVITY_KEEP: usize = 40;
/// After an `attach`, the service's errors about the old port are stale for this long (ms).
pub const RELINK_QUIET_MS: f64 = 15000.0;
/// What the panel says when the service doesn't answer.
pub const NOT_RUNNING: &str = "Collaboration is not running on this computer";
/// The binding kind of a Film window.
pub const BINDING: &str = "film-control";
/// Longest line read from the service (its replies and events are small JSON objects).
const MAX_LINE: usize = 1 << 20;

/// Wall-clock milliseconds.
pub fn now_ms() -> f64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// Where the service lives: its API port and its home folder (which holds `api.token`).
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub port: u16,
    pub home: Option<PathBuf>,
}

impl Config {
    /// From this process's environment (`KOPANO_API_PORT`, `KOPANO_HOME`, the OS folders).
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok();
        Config { port: api_port_with(get), home: home_with(get, std::env::consts::OS) }
    }
}

/// The API port: `KOPANO_API_PORT` when it is a port number, else [`DEFAULT_API_PORT`].
pub fn api_port_with(get: impl Fn(&str) -> Option<String>) -> u16 {
    match get("KOPANO_API_PORT").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        Some(v) => match v.parse::<u16>() {
            Ok(p) if p != 0 => p,
            _ => {
                log::warn!("KOPANO_API_PORT {v:?} is not a port number; using {DEFAULT_API_PORT}");
                DEFAULT_API_PORT
            }
        },
        None => DEFAULT_API_PORT,
    }
}

/// The service's home folder for `os` (`std::env::consts::OS`), with `get` reading the
/// environment. `None` when the variables it needs are not set.
pub fn home_with(get: impl Fn(&str) -> Option<String>, os: &str) -> Option<PathBuf> {
    let var = |k: &str| get(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(h) = var("KOPANO_HOME") {
        return Some(h);
    }
    match os {
        "windows" => var("LOCALAPPDATA").map(|d| d.join("Kopano")),
        "macos" => var("HOME").map(|h| h.join("Library").join("Application Support").join("Kopano")),
        _ => var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share"))).map(|d| d.join("kopano")),
    }
}

/// What a person reads for an error from the service, the connection or the plumbing of this
/// window: plain words, with no file paths, protocol names or raw service text (those stay in
/// the log and in `kopano.status`). The window's own messages are plain already and pass through.
pub fn for_people(e: &str) -> String {
    let has = |words: &[&str]| words.iter().any(|w| e.contains(w));
    let plain = if has(&["works live"]) {
        "This project works live: there is nothing to approve."
    } else if has(&["another project is open"]) {
        "Another project is open in this window."
    } else if has(&["api.token", "key file", "bad token"]) {
        "This window could not open collaboration on this computer. Restart Ubunye Film; if it keeps happening, install it again."
    } else if has(&["KOPANO_HOME", "folder is unknown"]) {
        "Collaboration is not set up on this computer. Install Ubunye Film again to add it."
    } else if has(&["--control", "control port"]) {
        "This window can't be shared yet. Restart Ubunye Film and try again."
    } else if has(&["addressing", "not reachable", "timed out", "could not reach", "unreachable"]) {
        "Your teammate's computer could not be reached. Check that they are online, then try again."
    } else if has(&["hung up", "stopped answering", "unreadable", "refused", "os error", "could not start"]) {
        "Collaboration stopped answering for a moment. Try again."
    } else if has(&["kopano", "Kopano", "KOPANO", "/", "\\", "{", "unknown op", "missing", ": "]) {
        "Something went wrong. Try again in a moment."
    } else {
        return e.to_string();
    };
    plain.to_string()
}

/// The API token the service wrote into its home folder.
pub fn read_token(home: &Path) -> Result<String, String> {
    let f = home.join("api.token");
    match std::fs::read_to_string(&f) {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        Ok(_) => Err(format!("the collaboration key file is empty ({})", f.display())),
        Err(_) => Err(format!("the collaboration key file is missing ({})", f.display())),
    }
}

/// The first 8 characters of a device or project id (ids are hex, so this is a char boundary).
pub fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// A 64-character hex id (devices; projects).
pub fn is_hex_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Who this computer is signed in as.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Me {
    pub device: String,
    pub name: String,
    pub account: String,
}

/// A "Sign in with ThabangVision" waiting for the person in the browser (`hello.signing_in`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SignIn {
    pub code: String,
    pub link: String,
    pub until_ms: Option<f64>,
}

/// Someone on a project shared on ThabangVision (`people`): a name, never an email.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Person {
    pub user: String,
    pub name: String,
    /// owner, admin, approver, editor or viewer.
    pub role: String,
    /// Invited and not joined yet.
    pub invited: bool,
    pub me: bool,
    pub online: bool,
}

impl Person {
    /// "owner", "can edit", "approves", "can view", "invited".
    pub fn role_text(&self) -> &'static str {
        if self.invited {
            return "invited";
        }
        match self.role.as_str() {
            "owner" => "owner",
            "admin" => "can share",
            "approver" => "approves",
            "viewer" => "can view",
            _ => "can edit",
        }
    }
}

/// A project someone shared with this person on ThabangVision (`shared-projects`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SharedRow {
    /// ThabangVision's id for it (what `join-shared` takes).
    pub shared: String,
    pub name: String,
    pub project: String,
    /// Already on this computer.
    pub here: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Teammate {
    pub device: String,
    pub account: Option<String>,
    /// Known once they were online (the service names offline teammates by device id only).
    pub name: Option<String>,
    pub online: bool,
}

impl Teammate {
    /// The name, else the short device id.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| short(&self.device).to_string())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub head: Option<String>,
    pub teammates: Vec<Teammate>,
    /// The Film control port the service drives this project through (`film-control` only).
    pub app_port: Option<u16>,
    /// Something answers on `app_port` (checked by the poll thread; `None` = not checked).
    pub app_alive: Option<bool>,
    pub members: Vec<Member>,
    /// The people on it, when it is shared on ThabangVision (`None`: not shared there, or a
    /// service without people).
    pub people: Option<Vec<Person>>,
    /// "live" (every edit goes to everyone) or "approve" (the owner approves changes); `None`
    /// from an older service, which only knows live.
    pub mode: Option<String>,
    /// What waits for approval, and where this person's own work is (approve mode).
    pub review: Option<Review>,
    /// Settings both sides changed at once in a live project, waiting for a pick.
    pub clashes: Vec<Clash>,
    /// The project's footage on this computer (`media`), once the service said.
    pub footage: Option<Footage>,
}

/// One footage file of a shared project as this computer has it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FootageFile {
    /// The service's key for it (the original's content hash; what `fetch-original` takes).
    pub key: String,
    /// The file name (`C0003.MP4`), when the service knows it.
    pub name: Option<String>,
    /// "original", "proxy", "missing" (known, nothing to play yet) or "waiting" (not here yet).
    pub shown_from: String,
    pub original_mb: f64,
    /// The original is on its way: (MB done, MB in all).
    pub arriving: Option<(f64, f64)>,
    /// Getting the original failed (the service's words, kept for the log).
    pub failed: bool,
}

impl FootageFile {
    /// The footage is not all here yet but is coming (a proxy plays, or the file is on its way).
    pub fn on_its_way(&self) -> bool {
        self.shown_from != "original"
    }

    /// What the Project panel's badge says instead of "Media missing".
    pub fn badge_text(&self) -> &'static str {
        if self.arriving.is_some() {
            "Original arriving"
        } else if self.shown_from == "proxy" {
            "Playing a proxy"
        } else {
            "On its way"
        }
    }

    /// One line for the panel: "C0003.MP4 · plays from a proxy (16.78 MB original)".
    pub fn line(&self) -> String {
        let name = self.name.clone().unwrap_or_else(|| short(&self.key).to_string());
        let what = match (&self.arriving, self.shown_from.as_str()) {
            (Some((done, all)), _) => format!("original arriving, {} of {}", mb_text(*done), mb_text(*all)),
            (None, "original") => "original here".to_string(),
            (None, "proxy") if self.failed => "plays from a proxy; the original could not come yet".to_string(),
            (None, "proxy") => format!("plays from a proxy ({} original)", mb_text(self.original_mb)),
            (None, _) => "on its way".to_string(),
        };
        format!("{name} · {what}")
    }
}

/// "16.8 MB", "1.2 GB".
pub fn mb_text(mb: f64) -> String {
    let mb = if mb.is_finite() { mb.max(0.0) } else { 0.0 };
    if mb >= 1000.0 { format!("{:.1} GB", mb / 1000.0) } else { format!("{mb:.1} MB") }
}

/// A shared project's footage on this computer (`media`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Footage {
    pub files: Vec<FootageFile>,
    pub from_original: u64,
    pub from_proxy: u64,
    pub waiting: u64,
}

impl Footage {
    /// From the service's `media` reply. `None` when it is not a Film project's reply.
    pub fn from_value(v: &Value) -> Option<Footage> {
        let n = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        // Film projects say `from_proxy` (photo projects `from_preview`)
        v.get("from_proxy")?;
        let files = v
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|f| {
                let download = f.get("download").filter(|d| d.is_object());
                let state = download.and_then(|d| d.get("state")).and_then(Value::as_str);
                let num = |d: &Value, k: &str| d.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                Some(FootageFile {
                    key: f.get("key")?.as_str()?.to_string(),
                    name: f.get("name").and_then(Value::as_str).map(str::to_string),
                    shown_from: f.get("shown_from").and_then(Value::as_str).unwrap_or("waiting").to_string(),
                    original_mb: f.get("original_mb").and_then(Value::as_f64).unwrap_or(0.0),
                    arriving: download.filter(|_| state == Some("downloading")).map(|d| (num(d, "mb_done"), num(d, "mb_total"))),
                    failed: state == Some("failed"),
                })
            })
            .collect();
        Some(Footage { files, from_original: n("from_original"), from_proxy: n("from_proxy"), waiting: n("waiting") })
    }

    /// Every file is the original.
    pub fn all_here(&self) -> bool {
        self.files.iter().all(|f| !f.on_its_way())
    }

    /// Something can still be asked for: a file that is not the original and not already coming.
    pub fn can_get_originals(&self) -> bool {
        self.files.iter().any(|f| f.on_its_way() && f.arriving.is_none())
    }

    /// The summary line: "3 clips play from proxies. The originals come when you ask."
    pub fn summary(&self) -> String {
        let arriving = self.files.iter().filter(|f| f.arriving.is_some()).count();
        if self.files.is_empty() {
            "No footage in this project yet.".to_string()
        } else if self.all_here() {
            "All footage is here.".to_string()
        } else if arriving > 0 {
            format!("{} on the way. Exports use the originals once they are here.", files_text(arriving, "original", "originals"))
        } else if self.from_proxy > 0 {
            let n = usize::try_from(self.from_proxy).unwrap_or(usize::MAX);
            format!("{} from proxies. The originals come when you ask.", if n == 1 { "1 file plays".to_string() } else { format!("{n} files play") })
        } else {
            "Footage is on its way from your teammate.".to_string()
        }
    }

    /// The file named `name` (the Project panel's media item name or file name), if any.
    pub fn file_named(&self, name: &str) -> Option<&FootageFile> {
        self.files.iter().find(|f| f.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }
}

/// "1 original" / "3 originals".
fn files_text(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// An "I approve changes" project as the service sees it (`status.review`, `review` events).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub is_owner: bool,
    /// This person may approve (the owner, or someone the owner chose).
    pub approver: bool,
    pub owner_name: String,
    pub pending: Vec<Pending>,
    /// Members whose latest work was declined (their branch keeps it).
    pub declined: Vec<String>,
    /// This person's own work (a member): none, waiting, approved or declined.
    pub mine: Option<Mine>,
    /// The owner's window shows a member's version now.
    pub viewing: Option<Viewing>,
}

/// One teammate's changes waiting for review.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Pending {
    pub account: String,
    pub name: String,
    pub version: String,
    /// Clips and tracks changed, as the service counts them (it calls every item a "photo").
    pub items: u64,
    /// What changed: `film/<sequence>/clip/<id>` or `film/<sequence>/track/<id>` keys.
    pub changed: Vec<String>,
    pub clashes: Vec<PendingClash>,
}

impl Pending {
    /// "changed 2 clips", "changed 1 clip and 1 track": counted from the keys (the service's own
    /// summary says "photos" for every kind of item).
    pub fn summary(&self) -> String {
        changed_text(&self.changed, self.items)
    }
}

/// What a list of changed item keys is, in words: "2 clips", "1 clip and 1 track". `fallback` is
/// the service's count when it gave no keys (said as clips).
pub fn changed_text(keys: &[String], fallback: u64) -> String {
    let clips = keys.iter().filter(|k| k.contains("/clip/")).count();
    let tracks = keys.iter().filter(|k| k.contains("/track/")).count();
    let other = keys.len().saturating_sub(clips + tracks);
    let clips = clips + other;
    match (clips, tracks) {
        (0, 0) => clips_text(Some(fallback)),
        (c, 0) => clips_text(Some(u64::try_from(c).unwrap_or(u64::MAX))),
        (0, t) => files_text(t, "track", "tracks"),
        (c, t) => format!("{} and {}", files_text(c, "clip", "clips"), files_text(t, "track", "tracks")),
    }
}

/// A setting the member and main both changed since the member last caught up.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PendingClash {
    pub key: String,
    /// The clip or track key (`film/S/clip/C`).
    pub item: String,
    pub setting: String,
    pub main: String,
    pub member: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Mine {
    pub state: String,
    pub items: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Viewing {
    pub account: String,
    pub name: String,
    /// "member" (their version shows) or "main" (before).
    pub side: String,
}

/// A live project's clash: one setting of one clip, two values; a person picks one.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Clash {
    /// The service's id for it (what `resolve` takes).
    pub id: String,
    pub item: String,
    pub setting: String,
    pub a: String,
    pub b: String,
}

/// A setting value in plain words: numbers with two decimals, text as it is.
pub fn value_text(v: &Value) -> String {
    match v {
        Value::Number(n) => n.as_f64().map(|f| if f.fract() == 0.0 { format!("{f:.0}") } else { format!("{f:.2}") }).unwrap_or_else(|| n.to_string()),
        Value::String(s) => s.clone(),
        Value::Null => "none".to_string(),
        Value::Bool(b) => if *b { "on" } else { "off" }.to_string(),
        other => other.to_string(),
    }
}

/// A Film setting path in words: "sound.gain_db" -> "Gain", "fx.lumetri.params.exposure.value.Float"
/// -> "Exposure", "cut" -> "Trim".
pub fn setting_text(path: &str) -> String {
    if path == "cut" || path.starts_with("cut.") {
        return "Trim".to_string();
    }
    let parts: Vec<&str> = path.split('.').filter(|p| !matches!(*p, "value" | "Float" | "Int" | "Bool" | "params" | "fx")).collect();
    let last = parts.last().copied().unwrap_or(path);
    let last = last.strip_suffix("_db").unwrap_or(last).replace('_', " ");
    let mut c = last.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => last,
    }
}

impl Review {
    /// From `status.review` or a `review` event.
    pub fn from_value(v: &Value) -> Review {
        let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let pending = v
            .get("pending")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|e| Pending {
                account: s(e, "account"),
                name: s(e, "name"),
                version: s(e, "version"),
                items: e.get("photos").and_then(Value::as_u64).unwrap_or(0),
                changed: e.get("changes").and_then(Value::as_array).into_iter().flatten().map(|c| s(c, "photo")).collect(),
                clashes: e
                    .get("clashes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .flat_map(|c| {
                        let (key, item) = (s(c, "key"), s(c, "photo"));
                        c.get("settings").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().map(move |x| PendingClash {
                            key: key.clone(),
                            item: item.clone(),
                            setting: s(&x, "setting"),
                            main: value_text(x.get("main").unwrap_or(&Value::Null)),
                            member: value_text(x.get("member").unwrap_or(&Value::Null)),
                        })
                    })
                    .collect(),
            })
            .collect();
        Review {
            is_owner: v.get("is_owner").and_then(Value::as_bool).unwrap_or(false),
            approver: v.get("approver").and_then(Value::as_bool).unwrap_or(false),
            owner_name: s(v, "owner_name"),
            pending,
            declined: v.get("declined").and_then(Value::as_array).into_iter().flatten().map(|d| s(d, "name")).collect(),
            mine: v.get("mine").filter(|m| m.is_object()).map(|m| Mine { state: s(m, "state"), items: m.get("photos").and_then(Value::as_u64).unwrap_or(0) }),
            viewing: v.get("viewing").filter(|m| m.is_object()).map(|m| Viewing { account: s(m, "account"), name: s(m, "name"), side: s(m, "side") }),
        }
    }

    /// The member's line: "Waiting for Alice's approval", "Approved", "Declined".
    pub fn mine_text(&self) -> Option<String> {
        let m = self.mine.as_ref()?;
        let owner = if self.owner_name.is_empty() { "the owner".to_string() } else { self.owner_name.clone() };
        Some(match m.state.as_str() {
            "waiting" => format!("You changed {} · Waiting for {owner}'s approval", clips_text(Some(m.items))),
            "approved" => format!("Approved: {owner} took your changes"),
            "declined" => format!("Declined: {owner} kept the project as it was. Your changes stay in your window."),
            _ => format!("Your changes go to {owner} for approval"),
        })
    }
}

/// From `status.clashes` (live projects).
pub fn clashes_from(v: &Value) -> Vec<Clash> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    v.as_array()
        .into_iter()
        .flatten()
        .flat_map(|c| {
            let (id, item) = (s(c, "clash"), s(c, "photo"));
            c.get("settings").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().take(1).map(move |x| Clash {
                id: id.clone(),
                item: item.clone(),
                setting: s(&x, "setting"),
                a: value_text(x.get("a").unwrap_or(&Value::Null)),
                b: value_text(x.get("b").unwrap_or(&Value::Null)),
            })
        })
        .collect()
}

/// Who may join a project (from `status`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Member {
    pub account: String,
    pub role: String,
    pub owner: bool,
}

/// What happened, kept as data so the UI words it at display time.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Said {
    Online {
        who: String,
    },
    Offline {
        who: String,
    },
    /// An edit made in this window was saved and sent.
    YouChanged {
        clips: Option<u64>,
    },
    /// A teammate's edit arrived and was applied here.
    Changed {
        who: String,
        clips: Option<u64>,
    },
    /// Both sides edited at once; their edits were merged (clashes: same setting on both sides).
    Merged {
        who: String,
        clashes: usize,
    },
    /// This window was linked to a shared project again (Film had restarted on a new port).
    Reattached {
        name: String,
    },
    /// Both sides ended up with the same cut.
    UpToDate {
        who: String,
    },
    /// The person opened another project in the window a shared project is linked to.
    Away {
        name: String,
    },
    /// The service could not read or write this window.
    AppProblem {
        error: String,
    },
    /// A teammate's changes wait for this person's approval.
    ToReview {
        who: String,
        what: String,
    },
    /// The owner (or an approver) approved this person's changes.
    Approved {
        who: String,
    },
    /// ... or declined them.
    Declined {
        who: String,
    },
    /// Footage files are on their way to this computer (proxies first).
    FootageComing {
        files: u64,
    },
    /// Footage arrived and the clips play (proxies or originals).
    FootageReady {
        files: u64,
    },
    /// An original file is here; the clip now plays and exports from it.
    OriginalHere {
        file: String,
    },
    /// An original could not be fetched (it is tried again when asked).
    OriginalFailed {
        file: String,
    },
}

impl Said {
    /// In plain words (English; the activity sentences are not in the catalogs yet).
    pub fn text(&self) -> String {
        match self {
            Said::Online { who } => format!("{who} is online"),
            Said::Offline { who } => format!("{who} went offline"),
            Said::YouChanged { clips: None } => "Your edit was saved and sent".to_string(),
            Said::YouChanged { clips } => format!("You changed {}", clips_text(*clips)),
            Said::Changed { who, clips: None } => format!("{who}'s edit arrived"),
            Said::Changed { who, clips } => format!("{who} changed {}", clips_text(*clips)),
            Said::Merged { who, clashes: 0 } => format!("Your changes and {who}'s were put together"),
            Said::Merged { who, clashes: 1 } => format!("Your changes and {who}'s were put together · 1 clash"),
            Said::Merged { who, clashes } => format!("Your changes and {who}'s were put together · {clashes} clashes"),
            Said::UpToDate { who } => format!("Up to date with {who}"),
            Said::Reattached { name } => format!("This window is linked to {name} again"),
            Said::Away { name } => format!("Another project is open in this window, so {name} is not shared from it"),
            // the service's own words stay in `kopano.status` and the log, not on screen
            Said::AppProblem { .. } => "Lost touch with this window for a moment; it reconnects by itself".to_string(),
            Said::ToReview { who, what } => format!("{who} changed {what} · waiting for your review"),
            Said::Approved { who } => format!("{who} approved your changes"),
            Said::Declined { who } => format!("{who} declined your changes"),
            Said::FootageComing { files } => format!("{} on the way; the clips play from proxies first", footage_files(*files)),
            Said::FootageReady { files } => format!("{} ready to play", footage_files(*files)),
            Said::OriginalHere { file } => format!("The original of {file} is here"),
            Said::OriginalFailed { file } => format!("The original of {file} could not come yet; ask again in a moment"),
        }
    }
}

/// "1 footage file", "3 footage files".
fn footage_files(n: u64) -> String {
    if n == 1 { "1 footage file".to_string() } else { format!("{n} footage files") }
}

/// "2 clips", "1 clip", or "clips" when the count is unknown.
pub fn clips_text(n: Option<u64>) -> String {
    match n {
        Some(1) => "1 clip".to_string(),
        Some(n) => format!("{n} clips"),
        None => "clips".to_string(),
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Activity {
    /// Wall-clock ms of the latest time it happened.
    pub at_ms: f64,
    pub project: Option<String>,
    pub said: Said,
    /// The same thing several times in a row (a slider drag is many edits).
    pub times: u32,
}

/// The last Share, Join, Approve or Get originals.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Job {
    pub busy: bool,
    pub ok: bool,
    pub text: String,
    /// The project a Share made (to copy and send to the teammate).
    pub project: Option<String>,
}

/// Everything the panel shows. The client threads write it; the UI reads a copy each frame.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// The client threads were started.
    pub started: bool,
    /// The service answered the last poll.
    pub running: bool,
    /// This computer is signed in to ThabangVision (the service's `hello`).
    pub signed_in: bool,
    /// Until when (wall-clock ms), when the service says.
    pub signed_in_until_ms: Option<f64>,
    /// A sign-in waiting in the browser.
    pub signing_in: Option<SignIn>,
    /// Why the last sign-in ended or this computer was signed out (the service's sentence).
    pub sign_in_message: Option<String>,
    /// Projects shared with this person that are not on this computer yet.
    pub shared_with_me: Vec<SharedRow>,
    /// Why it isn't running, or what went wrong talking to it (plain words).
    pub problem: Option<String>,
    pub me: Option<Me>,
    pub projects: Vec<Project>,
    /// Newest first.
    pub activity: Vec<Activity>,
    pub job: Option<Job>,
    /// Device id → name, learned from `status` and events (offline teammates keep their names).
    #[serde(skip)]
    pub names: BTreeMap<String, String>,
}

impl State {
    fn name_of(&self, device: &str) -> String {
        self.names.get(device).cloned().unwrap_or_else(|| short(device).to_string())
    }

    /// Add a line; the same thing again right after the last one only bumps its time and count.
    pub fn note(&mut self, project: Option<String>, said: Said, now_ms: f64) {
        if let Some(last) = self.activity.first_mut()
            && last.said == said
            && last.project == project
        {
            last.at_ms = now_ms;
            last.times = last.times.saturating_add(1);
            return;
        }
        self.activity.insert(0, Activity { at_ms: now_ms, project, said, times: 1 });
        self.activity.truncate(ACTIVITY_KEEP);
    }

    /// Take the reply to `hello` (who we are, which projects).
    pub fn apply_hello(&mut self, v: &Value) {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        self.me = Some(Me { device: s("device"), name: s("name"), account: s("account") });
        self.signed_in = v.get("signed_in").and_then(Value::as_bool).unwrap_or(false);
        self.signed_in_until_ms = v.get("signed_in_until_ms").and_then(Value::as_f64);
        self.signing_in = v.get("signing_in").filter(|x| x.is_object()).map(|x| SignIn {
            code: x.get("code").and_then(Value::as_str).unwrap_or_default().to_string(),
            link: x.get("link").and_then(Value::as_str).unwrap_or_default().to_string(),
            until_ms: x.get("until_ms").and_then(Value::as_f64),
        });
        self.sign_in_message = v.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()).map(str::to_string);
        if !self.signed_in {
            self.shared_with_me.clear();
        }
        let mut projects = Vec::new();
        for p in v.get("projects").and_then(Value::as_array).into_iter().flatten() {
            let Some(id) = p.get("id").and_then(Value::as_str) else { continue };
            let name = p.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
            // keep what the last status said until the next one
            let old = self.projects.iter().find(|o| o.id == id).cloned().unwrap_or_default();
            let app_port = app_port(p.get("app"));
            projects.push(Project { id: id.to_string(), name, app_alive: if app_port == old.app_port { old.app_alive } else { None }, app_port, ..old });
        }
        projects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        self.projects = projects;
    }

    /// Take the reply to `people {project}`.
    pub fn apply_people(&mut self, project: &str, v: &Value) {
        let Some(p) = self.projects.iter_mut().find(|p| p.id == project) else { return };
        if !v.get("shared").is_some_and(Value::is_string) {
            p.people = None;
            return;
        }
        let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        let b = |x: &Value, k: &str| x.get(k).and_then(Value::as_bool).unwrap_or(false);
        p.people = Some(
            v.get("people")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|x| Person { user: s(x, "user"), name: s(x, "name"), role: s(x, "role"), invited: s(x, "status") == "invited", me: b(x, "me"), online: b(x, "online") })
                .collect(),
        );
    }

    /// Take the reply to `shared-projects`: what is shared with this person and not here yet.
    pub fn apply_shared_projects(&mut self, v: &Value) {
        let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        self.shared_with_me = v
            .get("shared")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|x| !x.get("here").and_then(Value::as_bool).unwrap_or(false))
            .map(|x| SharedRow { shared: s(x, "shared"), name: s(x, "name"), project: s(x, "project"), here: false })
            .collect();
    }

    /// Take the reply to `status {project}`.
    pub fn apply_status(&mut self, project: &str, v: &Value) {
        let mut online: BTreeMap<String, String> = v
            .get("online")
            .and_then(Value::as_object)
            .map(|o| o.iter().map(|(d, n)| (d.clone(), n.as_str().unwrap_or_default().to_string())).collect())
            .unwrap_or_default();
        for (d, n) in &online {
            if !n.is_empty() {
                self.names.insert(d.clone(), n.clone());
            }
        }
        let mine = self.me.as_ref().map(|m| m.device.clone()).unwrap_or_default();
        // teammates: device ids (older services) or {device, name, account, online}
        let mut accounts: BTreeMap<String, String> = BTreeMap::new();
        let mut devices: Vec<String> = Vec::new();
        for t in v.get("teammates").and_then(Value::as_array).into_iter().flatten() {
            let Some(d) = t.as_str().or_else(|| t.get("device").and_then(Value::as_str)) else { continue };
            if let Some(n) = t.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) {
                self.names.insert(d.to_string(), n.to_string());
            }
            if let Some(a) = t.get("account").and_then(Value::as_str) {
                accounts.insert(d.to_string(), a.to_string());
            }
            if t.get("online").and_then(Value::as_bool) == Some(true) && !online.contains_key(d) {
                online.insert(d.to_string(), self.names.get(d).cloned().unwrap_or_default());
            }
            devices.push(d.to_string());
        }
        for d in online.keys() {
            if !devices.contains(d) {
                devices.push(d.clone());
            }
        }
        devices.retain(|d| *d != mine);
        let names = &self.names;
        let teammates = devices
            .iter()
            .map(|d| Teammate { device: d.clone(), account: accounts.get(d).cloned(), name: names.get(d).cloned(), online: online.contains_key(d) })
            .collect::<Vec<_>>();
        let head = v.get("head").and_then(Value::as_str).map(str::to_string);
        let members: Vec<Member> = v
            .get("members")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                Some(Member {
                    account: m.get("account")?.as_str()?.to_string(),
                    role: m.get("role").and_then(Value::as_str).unwrap_or("editor").to_string(),
                    owner: m.get("owner").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect();
        let port = v.get("app").map(|a| app_port(Some(a)));
        let mode = v.get("mode").and_then(Value::as_str).map(str::to_string);
        let review = v.get("review").map(Review::from_value);
        let clashes = v.get("clashes").map(clashes_from).unwrap_or_default();
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == project) {
            p.teammates = teammates;
            p.head = head;
            p.members = members;
            p.mode = mode;
            p.review = review;
            p.clashes = clashes;
            if let Some(port) = port
                && port != p.app_port
            {
                p.app_port = port;
                p.app_alive = None;
            }
        }
    }

    /// Take the reply to `media {project}` (Film projects only; others are left without footage).
    pub fn apply_media(&mut self, project: &str, v: &Value) {
        let Some(p) = self.projects.iter_mut().find(|p| p.id == project) else { return };
        p.footage = Footage::from_value(v);
        // an original reported as failed that is here after all (the service can fail saving its
        // own notes after the file itself arrived): say it is here
        let here: Vec<String> = p.footage.iter().flat_map(|f| &f.files).filter(|f| !f.on_its_way()).filter_map(|f| f.name.clone()).collect();
        for a in &mut self.activity {
            if a.project.as_deref() == Some(project)
                && let Said::OriginalFailed { file } = &a.said
                && here.contains(file)
            {
                a.said = Said::OriginalHere { file: file.clone() };
            }
        }
    }

    /// Take one event line from `subscribe`.
    pub fn apply_event(&mut self, ev: &Value, now_ms: f64) {
        let s = |k: &str| ev.get(k).and_then(Value::as_str);
        let project = s("project").map(str::to_string);
        match s("event") {
            Some("teammate") => {
                let Some(device) = s("device") else { return };
                if let Some(n) = s("name").filter(|n| !n.is_empty()) {
                    self.names.insert(device.to_string(), n.to_string());
                }
                let online = ev.get("online").and_then(Value::as_bool).unwrap_or(false);
                let name = self.names.get(device).cloned();
                if let Some(p) = project.as_ref().and_then(|id| self.projects.iter_mut().find(|p| p.id == *id)) {
                    match p.teammates.iter_mut().find(|t| t.device == device) {
                        Some(t) => {
                            t.online = online;
                            t.name = name.or(t.name.take());
                        }
                        None => p.teammates.push(Teammate { device: device.to_string(), account: None, name, online }),
                    }
                }
                let who = self.name_of(device);
                self.note(project, if online { Said::Online { who } } else { Said::Offline { who } }, now_ms);
            }
            Some("saved") => {
                self.set_head(project.as_deref(), s("head"));
                self.note(project, Said::YouChanged { clips: item_count(ev) }, now_ms);
            }
            Some("synced") => {
                self.set_head(project.as_deref(), s("head"));
                let who = s("from").unwrap_or("A teammate").to_string();
                let clashes = ev.get("clashes").and_then(Value::as_u64).map(|c| usize::try_from(c).unwrap_or(usize::MAX)).unwrap_or(0);
                let said = match s("how") {
                    Some("merged") => Said::Merged { who, clashes },
                    Some(h) if h.starts_with("same content") => Said::UpToDate { who },
                    _ => Said::Changed { who, clips: item_count(ev) },
                };
                self.note(project, said, now_ms);
            }
            Some("review") => {
                let new = Review::from_value(ev);
                let Some(p) = project.as_ref().and_then(|id| self.projects.iter_mut().find(|p| p.id == *id)) else { return };
                let old = p.review.replace(new.clone()).unwrap_or_default();
                p.mode = Some("approve".to_string());
                // say what changed: new work to review, or what happened to mine
                let mut said = vec![];
                for e in &new.pending {
                    if !old.pending.iter().any(|o| o.account == e.account && o.version == e.version) {
                        said.push(Said::ToReview { who: e.name.clone(), what: e.summary() });
                    }
                }
                let (was, now) = (old.mine.map(|m| m.state), new.mine.as_ref().map(|m| m.state.clone()));
                if was.is_some() && was != now {
                    match now.as_deref() {
                        Some("approved") => said.push(Said::Approved { who: new.owner_name.clone() }),
                        Some("declined") => said.push(Said::Declined { who: new.owner_name.clone() }),
                        _ => {}
                    }
                }
                for x in said {
                    self.note(project.clone(), x, now_ms);
                }
            }
            Some("media") => {
                let n = |k: &str| ev.get(k).and_then(Value::as_u64);
                if let Some(e) = s("error") {
                    log::info!("collaboration: footage: {e}");
                    return;
                }
                // the first event names how many files are coming; one per proxy follows (`file`)
                match (n("files_waiting"), n("files_ready"), s("file").is_some()) {
                    (Some(w), _, false) if w > 0 => self.note(project, Said::FootageComing { files: w }, now_ms),
                    (Some(0), Some(r), true) if r > 0 => self.note(project, Said::FootageReady { files: r }, now_ms),
                    _ => {}
                }
            }
            Some("original") => {
                let key = s("key").unwrap_or_default().to_string();
                let file = s("file").or(s("photo")).map(str::to_string).unwrap_or_else(|| self.footage_name(project.as_deref(), &key));
                let state = s("state").unwrap_or_default();
                // what the panel shows until the next `media` poll
                if let Some(f) = project
                    .as_ref()
                    .and_then(|id| self.projects.iter_mut().find(|p| p.id == *id))
                    .and_then(|p| p.footage.as_mut())
                    .and_then(|m| m.files.iter_mut().find(|f| f.key == key))
                {
                    let num = |k: &str| ev.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                    match state {
                        "downloading" => (f.arriving, f.failed) = (Some((num("mb_done"), num("mb_total"))), false),
                        "ready" => (f.arriving, f.failed, f.shown_from) = (None, false, "original".to_string()),
                        "failed" => (f.arriving, f.failed) = (None, true),
                        _ => {}
                    }
                }
                match state {
                    "ready" => self.note(project, Said::OriginalHere { file }, now_ms),
                    "failed" => {
                        log::info!("collaboration: original of {file}: {}", s("error").unwrap_or_default());
                        self.note(project, Said::OriginalFailed { file }, now_ms);
                    }
                    _ => {}
                }
            }
            Some("app") => {
                let error = s("error").unwrap_or("unknown problem").to_string();
                log::info!("collaboration: the service could not reach this window: {error}");
                // the service's last try at the old port, just after this window linked it again
                let just_linked =
                    self.activity.iter().any(|a| a.project == project && matches!(a.said, Said::Reattached { .. }) && now_ms - a.at_ms < RELINK_QUIET_MS);
                if just_linked {
                    return;
                }
                self.note(project, Said::AppProblem { error }, now_ms);
            }
            _ => {}
        }
    }

    /// The file name the service gave for a footage key, else the start of the key.
    fn footage_name(&self, project: Option<&str>, key: &str) -> String {
        project
            .and_then(|id| self.projects.iter().find(|p| p.id == id))
            .and_then(|p| p.footage.as_ref())
            .and_then(|m| m.files.iter().find(|f| f.key == key))
            .and_then(|f| f.name.clone())
            .unwrap_or_else(|| short(key).to_string())
    }

    fn set_head(&mut self, project: Option<&str>, head: Option<&str>) {
        // events carry the short head; status the full one: both only say "it moved"
        if let (Some(id), Some(h)) = (project, head)
            && let Some(p) = self.projects.iter_mut().find(|p| p.id == id)
        {
            p.head = Some(h.to_string());
        }
    }

    /// The footage of the project this window is linked to (`port`: this window's control port),
    /// for the Project panel's badges.
    pub fn footage_for_port(&self, port: Option<u16>) -> Option<&Footage> {
        let port = port?;
        self.projects.iter().filter(|p| p.app_port == Some(port)).find_map(|p| p.footage.as_ref())
    }
}

/// The control port of a `film-control` app binding (`{"kind": "film-control", "port": N}`).
pub fn app_port(app: Option<&Value>) -> Option<u16> {
    let a = app?;
    if a.get("kind").and_then(Value::as_str) != Some(BINDING) {
        return None;
    }
    a.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0)
}

/// Where this app remembers which project file and footage a shared project is, so that after a
/// restart on a new control port it knows which shared projects are its own
/// (`<home>/film/<project>.json`).
pub fn record_path(home: &Path, project: &str) -> Option<PathBuf> {
    is_hex_id(project).then(|| home.join("film").join(format!("{project}.json")))
}

/// What a window's project is, for [`holds`]: the shared project it belongs to (the project's
/// `share_id`), its file (if saved) and its footage file names (for the log only).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    pub share: Option<String>,
    pub file: Option<String>,
    pub footage: BTreeSet<String>,
}

pub fn write_record(home: &Path, project: &str, r: &Record) -> Result<(), String> {
    let f = record_path(home, project).ok_or("not a project id")?;
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = f.with_extension("json.tmp");
    std::fs::write(&tmp, json!({"share": r.share, "file": r.file, "footage": r.footage}).to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &f).map_err(|e| e.to_string())
}

/// The record for `project`; empty when there is no readable one.
pub fn read_record(home: &Path, project: &str) -> Record {
    let Some(f) = record_path(home, project) else { return Record::default() };
    let Ok(text) = std::fs::read_to_string(f) else { return Record::default() };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Record {
        share: v.get("share").and_then(Value::as_str).map(str::to_string),
        file: v.get("file").and_then(Value::as_str).map(str::to_string),
        footage: v.get("footage").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
    }
}

/// This window holds a shared project when its project carries that shared project's id, or it
/// has the same project file open. Nothing weaker counts: footage names such as C0001.MP4 repeat
/// across shoots, so any other window is linked only by the person ("Link this window").
pub fn holds(recorded: &Record, ours: &Record) -> bool {
    if recorded.share.is_some() && recorded.share == ours.share {
        return true;
    }
    matches!((&recorded.file, &ours.file), (Some(a), Some(b)) if same_path(a, b))
}

fn same_path(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let s = s.replace('\\', "/");
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// How many clips an event says changed (`photos`, the service's word for items), if it says.
fn item_count(ev: &Value) -> Option<u64> {
    ev.get("photos").and_then(Value::as_u64).filter(|n| *n > 0)
}

/// "3 s ago", "2 min ago" (`secs` since it happened).
pub fn ago(secs: f64) -> String {
    let s = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
    if s < 2.0 {
        "just now".to_string()
    } else if s < 60.0 {
        format!("{} s ago", s.floor())
    } else if s < 3600.0 {
        format!("{} min ago", (s / 60.0).floor())
    } else if s < 86400.0 {
        format!("{} h ago", (s / 3600.0).floor())
    } else {
        format!("{} d ago", (s / 86400.0).floor())
    }
}

/// Shared between the client threads and the UI.
pub type Shared = Arc<Mutex<State>>;

pub fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The client: its state and how to wake the poll thread.
pub struct Client {
    pub config: Config,
    pub shared: Shared,
    #[cfg(not(target_arch = "wasm32"))]
    wake: Option<std::sync::mpsc::Sender<()>>,
}

impl Client {
    pub fn new(config: Config) -> Self {
        Client {
            config,
            shared: Arc::default(),
            #[cfg(not(target_arch = "wasm32"))]
            wake: None,
        }
    }

    /// A copy of the state for this frame.
    pub fn state(&self) -> State {
        lock(&self.shared).clone()
    }

    pub fn started(&self) -> bool {
        lock(&self.shared).started
    }
}

#[cfg(target_arch = "wasm32")]
impl Client {
    pub fn start(&mut self, _ctx: &egui::Context) {
        let mut s = lock(&self.shared);
        s.started = true;
        s.problem = Some("Collaboration works in the desktop app only.".to_string());
    }

    pub fn request(&mut self, _ctx: &egui::Context, _busy: &str, _req: Value) -> Result<(), String> {
        Err("Collaboration works in the desktop app only.".to_string())
    }

    pub fn request_chain(&mut self, _ctx: &egui::Context, _busy: &str, _reqs: Vec<Value>) -> Result<(), String> {
        Err("Collaboration works in the desktop app only.".to_string())
    }

    pub fn attach(&mut self, _ctx: &egui::Context, _project: &str, _name: &str, _port: u16) {}

    pub fn wake(&self) {}
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{call, connect};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    use super::*;

    /// One connection to the service's API.
    pub struct Conn {
        read: BufReader<TcpStream>,
        write: TcpStream,
        token: String,
    }

    /// Connect and read the token. The error is in plain words for the panel.
    pub fn connect(config: &Config, timeout: Duration) -> Result<Conn, String> {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], config.port));
        let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).map_err(|_| NOT_RUNNING.to_string())?;
        let Some(home) = &config.home else { return Err("the collaboration folder is unknown here (set KOPANO_HOME)".to_string()) };
        let token = read_token(home)?;
        let _ = stream.set_read_timeout(Some(timeout));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_nodelay(true);
        let write = stream.try_clone().map_err(|e| e.to_string())?;
        Ok(Conn { read: BufReader::new(stream), write, token })
    }

    impl Conn {
        /// Send one request (the token is added) without reading the reply.
        fn send(&mut self, mut req: Value) -> Result<(), String> {
            if let Some(o) = req.as_object_mut() {
                o.insert("token".into(), json!(self.token));
            }
            writeln!(self.write, "{req}").and_then(|()| self.write.flush()).map_err(|_| "the collaboration service stopped answering".to_string())
        }

        /// One line from the service; `Ok(None)` when it hung up.
        pub fn line(&mut self) -> Result<Option<Value>, String> {
            let mut buf = Vec::new();
            let limit = u64::try_from(MAX_LINE).unwrap_or(u64::MAX);
            let n = (&mut self.read).take(limit).read_until(b'\n', &mut buf).map_err(|_| "the collaboration service stopped answering".to_string())?;
            if n == 0 {
                return Ok(None);
            }
            serde_json::from_slice(&buf).map(Some).map_err(|_| "the collaboration service sent something unreadable".to_string())
        }
    }

    /// One request, one reply: the `result`, or the service's error.
    pub fn call(conn: &mut Conn, req: Value) -> Result<Value, String> {
        conn.send(req)?;
        let reply = conn.line()?.ok_or_else(|| "the collaboration service hung up".to_string())?;
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
        let e = reply.get("error").and_then(Value::as_str).unwrap_or("the collaboration service refused").to_string();
        Err(if e == "bad token" { "the collaboration service refused this window (its key file api.token does not match)".to_string() } else { e })
    }

    /// Ask `hello` and every project's `status` and `media` once.
    pub fn poll_once(config: &Config, shared: &Shared) {
        let r = (|| -> Result<(), String> {
            let mut c = connect(config, Duration::from_secs(5))?;
            let hello = call(&mut c, json!({"op": "hello"}))?;
            let ids: Vec<String> = {
                let mut s = lock(shared);
                s.apply_hello(&hello);
                s.projects.iter().map(|p| p.id.clone()).collect()
            };
            // ThabangVision: what is shared with this person, and who is on each project (a
            // service without these answers "unknown op": nothing changes)
            if lock(shared).signed_in
                && let Ok(v) = call(&mut c, json!({"op": "shared-projects"}))
            {
                lock(shared).apply_shared_projects(&v);
            }
            for id in &ids {
                if let Ok(v) = call(&mut c, json!({"op": "people", "project": id})) {
                    lock(shared).apply_people(id, &v);
                }
            }
            for id in ids {
                // one project's trouble doesn't hide the others
                if let Ok(st) = call(&mut c, json!({"op": "status", "project": id})) {
                    lock(shared).apply_status(&id, &st);
                }
                let film = lock(shared).projects.iter().any(|p| p.id == id && p.app_port.is_some());
                if film && let Ok(m) = call(&mut c, json!({"op": "media", "project": id})) {
                    lock(shared).apply_media(&id, &m);
                }
            }
            // is a Film still answering where each project points? (a dead port after a restart)
            let ports: Vec<(String, u16)> = lock(shared).projects.iter().filter_map(|p| Some((p.id.clone(), p.app_port?))).collect();
            for (id, port) in ports {
                let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
                let alive = TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok();
                if let Some(p) = lock(shared).projects.iter_mut().find(|p| p.id == id && p.app_port == Some(port)) {
                    p.app_alive = Some(alive);
                }
            }
            Ok(())
        })();
        let mut s = lock(shared);
        match r {
            Ok(()) => {
                s.running = true;
                s.problem = None;
            }
            Err(e) => {
                s.running = false;
                for p in &mut s.projects {
                    for t in &mut p.teammates {
                        t.online = false;
                    }
                }
                s.problem = Some(e);
            }
        }
    }

    impl Client {
        /// Start the poll and event threads (once). `ctx` is repainted when something changes.
        pub fn start(&mut self, ctx: &egui::Context) {
            if self.started() {
                return;
            }
            lock(&self.shared).started = true;
            let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
            self.wake = Some(wake_tx);
            let (config, shared, repaint) = (self.config.clone(), self.shared.clone(), ctx.clone());
            let poll = std::thread::Builder::new().name("collab-poll".into()).spawn(move || {
                loop {
                    let before = format!("{:?}", lock(&shared).projects);
                    let was = lock(&shared).running;
                    poll_once(&config, &shared);
                    let s = lock(&shared);
                    if s.running != was || format!("{:?}", s.projects) != before {
                        repaint.request_repaint();
                    }
                    drop(s);
                    match wake_rx.recv_timeout(Duration::from_millis(POLL_MS)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        // the app is gone
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
            });
            let (config, shared, repaint) = (self.config.clone(), self.shared.clone(), ctx.clone());
            let events = std::thread::Builder::new().name("collab-events".into()).spawn(move || {
                loop {
                    let _ = follow_events(&config, &shared, &repaint);
                    std::thread::sleep(Duration::from_secs(2));
                }
            });
            if let Err(e) = poll.and(events) {
                lock(&self.shared).problem = Some(format!("could not start collaboration: {e}"));
            }
        }

        /// Ask the poll thread to look now (after a Share or Join).
        pub fn wake(&self) {
            if let Some(w) = &self.wake {
                let _ = w.send(());
            }
        }

        /// Point `project` at this window's control `port` again (`attach`), quietly: the outcome
        /// is an activity line, not the Share / Join line.
        pub fn attach(&mut self, ctx: &egui::Context, project: &str, name: &str, port: u16) {
            self.start(ctx);
            let (config, shared, repaint, wake) = (self.config.clone(), self.shared.clone(), ctx.clone(), self.wake.clone());
            let req = json!({"op": "attach", "project": project, "app": {"kind": BINDING, "port": port}});
            let (project, name) = (project.to_string(), name.to_string());
            let _ = std::thread::Builder::new().name("collab-attach".into()).spawn(move || {
                let r = connect(&config, Duration::from_secs(20)).and_then(|mut c| call(&mut c, req));
                let said = match r {
                    Ok(_) => Said::Reattached { name },
                    Err(error) => Said::AppProblem { error },
                };
                let mut s = lock(&shared);
                if matches!(said, Said::Reattached { .. }) {
                    // the trouble before (the old port) is over
                    s.activity.retain(|a| !(a.project.as_deref() == Some(project.as_str()) && matches!(a.said, Said::AppProblem { .. })));
                }
                s.note(Some(project), said, now_ms());
                drop(s);
                if let Some(w) = wake {
                    let _ = w.send(());
                }
                repaint.request_repaint();
            });
        }

        /// Run one request (`create-project`, `join`, `approve`, `fetch-original`…) on its own
        /// thread; the outcome lands in [`State::job`]. `busy` is what the panel says meanwhile.
        pub fn request(&mut self, ctx: &egui::Context, busy: &str, req: Value) -> Result<(), String> {
            self.request_chain(ctx, busy, vec![req])
        }

        /// Several requests one after the other (Share, then an invite per email). A later
        /// request whose `project` is "$project" gets the project the first one made. The job
        /// says what the first one did, or the first error.
        pub fn request_chain(&mut self, ctx: &egui::Context, busy: &str, reqs: Vec<Value>) -> Result<(), String> {
            let Some(req) = reqs.first().cloned() else { return Ok(()) };
            let rest: Vec<Value> = reqs[1..].to_vec();
            self.start(ctx);
            {
                let mut s = lock(&self.shared);
                if s.job.as_ref().is_some_and(|j| j.busy) {
                    return Err("still busy with the last request".to_string());
                }
                s.job = Some(Job { busy: true, ok: false, text: busy.to_string(), project: None });
            }
            let (config, shared, repaint, wake) = (self.config.clone(), self.shared.clone(), ctx.clone(), self.wake.clone());
            let op = req.get("op").and_then(Value::as_str).unwrap_or_default().to_string();
            let spawned = std::thread::Builder::new().name("collab-job".into()).spawn(move || {
                // a join fetches the project from the teammate: up to 30 s to reach them
                let r = connect(&config, Duration::from_secs(90)).and_then(|mut c| {
                    let first = call(&mut c, req)?;
                    let made = first.get("project").and_then(Value::as_str).unwrap_or_default().to_string();
                    for mut next in rest {
                        if next.get("project").and_then(Value::as_str) == Some("$project") {
                            next["project"] = json!(made);
                        }
                        call(&mut c, next)?;
                    }
                    Ok(first)
                });
                let job = match r {
                    Ok(v) => job_done(&op, &v),
                    Err(e) => {
                        log::info!("collaboration: {op} failed: {e}");
                        Job { busy: false, ok: false, text: e, project: None }
                    }
                };
                lock(&shared).job = Some(job);
                if let Some(w) = wake {
                    let _ = w.send(());
                }
                repaint.request_repaint();
            });
            if let Err(e) = spawned {
                let text = format!("could not start the request: {e}");
                lock(&self.shared).job = Some(Job { busy: false, ok: false, text: text.clone(), project: None });
                return Err(text);
            }
            Ok(())
        }
    }

    /// Hold a `subscribe` stream until the service hangs up.
    fn follow_events(config: &Config, shared: &Shared, repaint: &egui::Context) -> Result<(), String> {
        // no read timeout: events come when they come
        let mut c = connect(config, Duration::from_secs(3600 * 24 * 365))?;
        call(&mut c, json!({"op": "subscribe"}))?;
        while let Some(ev) = c.line()? {
            lock(shared).apply_event(&ev, now_ms());
            repaint.request_repaint();
        }
        Ok(())
    }
}

/// What the panel says once a request is done.
pub fn job_done(op: &str, v: &Value) -> Job {
    let done = |text: &str| Job { busy: false, ok: true, text: text.to_string(), project: None };
    match op {
        "create-project" => Job {
            busy: false,
            ok: true,
            text: "Shared this project. Invite sent.".to_string(),
            project: v.get("project").and_then(Value::as_str).map(str::to_string),
        },
        "join" => Job {
            busy: false,
            ok: true,
            text: "Joined. Your teammate's cut arrives in a few seconds; the clips play from proxies until you get the originals.".to_string(),
            project: v.get("project").and_then(Value::as_str).map(str::to_string),
        },
        "join-shared" => Job {
            busy: false,
            ok: true,
            text: "Joined. Your teammate's cut arrives in a few seconds; the clips play from proxies until you get the originals.".to_string(),
            project: v.get("project").and_then(Value::as_str).map(str::to_string),
        },
        "sign-in" => match v.get("code").and_then(Value::as_str) {
            Some(code) => Job { busy: false, ok: true, text: format!("Check that the browser shows {code}."), project: None },
            None => done("Finish signing in in your browser."),
        },
        "sign-in-cancel" => done("Sign-in stopped."),
        "sign-out" => done("Signed out of ThabangVision on this computer."),
        "invite" => done("Invite sent."),
        "leave" => done("You left the project. It stays on this computer but no longer changes."),
        "remove-person" => done("Taken off the project."),
        "approve" => done("Approved. The changes are in your project."),
        "decline" => done("Declined. Your project stays as it was."),
        "view" => done(match v.pointer("/viewing/side").and_then(Value::as_str) {
            Some("member") => "Showing their version of the clips they changed. Editing waits until you are done.",
            Some("main") => "Showing your version (before).",
            _ => "Back to your project.",
        }),
        "resolve" => done("Clash settled. Everyone gets your pick."),
        "fetch-original" => match v.get("started").and_then(Value::as_u64) {
            Some(0) => done("The originals are here or already on their way."),
            Some(1) => done("Getting 1 original. Its clips switch to it when it arrives."),
            Some(n) => Job { busy: false, ok: true, text: format!("Getting {n} originals. The clips switch to them as each one arrives."), project: None },
            None => done("Getting the originals. The clips switch to them as each one arrives."),
        },
        _ => done("Done."),
    }
}
