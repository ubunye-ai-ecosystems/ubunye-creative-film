//! The Share & Collaborate window (Window ▸ Share & Collaborate, and the people icon in the
//! header): ThabangVision collaboration for this film project. Ubunye, Kopano.
//!
//! People see ThabangVision collaboration only: who they are signed in as, the shared projects
//! with their teammates by name (green dot when online), recent activity in plain words, the
//! footage (playing from a proxy, original arriving, "Get originals"), and "Share this project".
//! No protocol names, device ids or project ids outside the collapsed "Advanced (testing)"
//! section, which keeps the id-based Join until ThabangVision email invites reach the teammate
//! by themselves. Invites are by email; the window shows teammates by name, never their emails,
//! and the answer to an invite is always "Invite sent". The code keeps the protocol's name (Kopano) internally.
//!
//! Everything is a command: `kopano.panel` shows or hides the window, `kopano.share`,
//! `kopano.join`, `kopano.getOriginals` are what its buttons run, `kopano.signIn` is the sign-in
//! button, `kopano.status` is what it shows (for agents and tests).
//!
//! Two ways to work, chosen when sharing: "Work together live" (every edit goes to everyone) and
//! "I approve changes" (the default: teammates' edits wait in "Changes to review" until the
//! owner approves them; `kopano.approve`, `kopano.decline`, `kopano.view`, `kopano.choose`,
//! `kopano.setApprover`). Live clashes get a pick (`kopano.resolve`).
//!
//! The service reaches this window through Film's control port. With `--control PORT` that is
//! the port; without it, the first Share or Join starts the control server on demand
//! ([`StartControl`], supplied by the desktop app) and hands its port to the service.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::Receiver;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, vec2};
use serde_json::{Value, json};

use super::kopano::{self, Client, Config, Job, State};
use crate::FilmcraftApp;
use crate::control::ControlRequest;
use crate::theme::Tokens;

/// Start the control server if it isn't running: its port, and its request channel when this
/// call started it (the app then serves it like one started with `--control`).
pub type StartControl = Box<dyn FnMut() -> Result<(u16, Option<Receiver<ControlRequest>>), String>>;

/// The window's title, menu item and header tooltip.
pub const TITLE: &str = "Share & Collaborate";
/// Online teammates' dot.
const GREEN: Color32 = Color32::from_rgb(0x4c, 0xc3, 0x6b);
/// Something needs a look (a declined change, a clash, lost touch).
const CAUTION: Color32 = Color32::from_rgb(0xe8, 0xb3, 0x4a);
/// Activity lines shown (the rest are in `kopano.status`).
const SHOWN: usize = 6;
/// The window's width.
const W: f32 = 360.0;

pub struct KopanoPanel {
    pub open: bool,
    /// The "Advanced (testing)" section is open.
    pub advanced: bool,
    pub client: Client,
    /// From the desktop app; none on the web and in tests that don't give one.
    pub start_control: Option<StartControl>,
    /// The control port the service was given (once known).
    pub control_port: Option<u16>,
    pub share_name: String,
    pub share_account: String,
    /// The "Invite by email" field under a project this person owns.
    pub invite_email: String,
    /// "approve" (default) or "live": how the next Share works.
    pub share_mode: String,
    /// The owner's pick per clash before approving: (project, member account, item key) ->
    /// "member" | "main". Two members can clash on the same setting; each has their own pick.
    pub choices: BTreeMap<(String, String, String), String>,
    pub join_project: String,
    pub join_from: String,
    /// Shared projects an `attach` was sent for (once each).
    pub attached: BTreeSet<String>,
    /// The project this window had (`Session::document`) when each shared project was linked
    /// to it. Opening another project does not move a shared project along with it.
    pub bound: BTreeMap<String, u64>,
    /// Shared projects linked to this window while another project is open in it.
    pub away: BTreeSet<String>,
    /// The last thing copied (tests and agents read it; the clipboard itself is the OS's).
    pub copied: Option<String>,
    /// When the background work last ran (wall ms).
    last_tick: f64,
    /// What was last written to each linked project's record.
    recorded: BTreeMap<String, kopano::Record>,
    /// A Link Media prompt was put away because the footage is on its way (once per project).
    link_prompt_closed: BTreeSet<String>,
}

impl Default for KopanoPanel {
    fn default() -> Self {
        KopanoPanel::with_config(Config::from_env())
    }
}

impl KopanoPanel {
    pub fn with_config(config: Config) -> Self {
        KopanoPanel {
            open: false,
            advanced: false,
            client: Client::new(config),
            start_control: None,
            control_port: None,
            share_name: String::new(),
            share_account: String::new(),
            invite_email: String::new(),
            share_mode: "approve".to_string(),
            choices: BTreeMap::new(),
            join_project: String::new(),
            join_from: String::new(),
            attached: BTreeSet::new(),
            bound: BTreeMap::new(),
            away: BTreeSet::new(),
            copied: None,
            last_tick: f64::NEG_INFINITY,
            recorded: BTreeMap::new(),
            link_prompt_closed: BTreeSet::new(),
        }
    }
}

/// The port the service reaches this window on, starting the control server if needed.
fn control_port(app: &mut FilmcraftApp) -> Result<u16, String> {
    if let Some(p) = app.kopano.control_port {
        return Ok(p);
    }
    let Some(start) = app.kopano.start_control.as_mut() else {
        return Err("This window can't be shared: it has no control port (start Ubunye Film with --control PORT)".to_string());
    };
    let (port, rx) = start()?;
    if let Some(rx) = rx {
        app.control_rx = Some(rx);
    }
    app.kopano.control_port = Some(port);
    Ok(port)
}

fn text_param(p: &Value, key: &str, fallback: &str) -> String {
    p.get(key).and_then(Value::as_str).unwrap_or(fallback).trim().to_string()
}

fn set_job(app: &FilmcraftApp, ok: bool, text: impl Into<String>) {
    kopano::lock(&app.kopano.client.shared).job = Some(Job { busy: false, ok, text: text.into(), project: None });
}

/// The collaboration commands; `None` for any other id (the engine's `kopano.*` commands, such
/// as `kopano.items`, fall through to the engine).
pub fn command(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, p: &Value) -> Option<Result<Value, String>> {
    // A shared project stays with the project it was shared from: while another project is
    // open in this window the service may neither read it (nothing of it is sent) nor change it.
    if matches!(id, "kopano.items" | "kopano.apply" | "kopano.idBlock") && another_project_open(app) {
        return Some(Err("another project is open in this window".to_string()));
    }
    Some(match id {
        "kopano.panel" => {
            app.kopano.open = p.get("open").and_then(Value::as_bool).unwrap_or(!app.kopano.open);
            if let Some(a) = p.get("advanced").and_then(Value::as_bool) {
                app.kopano.advanced = a;
            }
            if app.kopano.open {
                app.kopano.client.start(ctx);
                if app.kopano.share_name.is_empty() {
                    app.kopano.share_name = app.session.project.name.clone();
                }
            }
            Ok(json!({"open": app.kopano.open, "advanced": app.kopano.advanced}))
        }
        "kopano.signIn" => app.kopano.client.request(ctx, "Opening ThabangVision in your browser…", json!({"op": "sign-in"})).map(|()| json!({"started": true})),
        "kopano.signInCancel" => app.kopano.client.request(ctx, "Stopping…", json!({"op": "sign-in-cancel"})).map(|()| json!({"started": true})),
        "kopano.signOut" => app.kopano.client.request(ctx, "Signing out…", json!({"op": "sign-out"})).map(|()| json!({"started": true})),
        "kopano.signInPage" => match app.kopano.client.state().signing_in.map(|s| s.link).filter(|l| l.starts_with("http")) {
            Some(link) => {
                ctx.open_url(egui::OpenUrl::new_tab(&link));
                Ok(json!({"opened": link}))
            }
            None => Err("No sign-in is waiting. Press Sign in with ThabangVision.".to_string()),
        },
        "kopano.invite" => invite(app, ctx, p),
        "kopano.joinShared" => join_shared(app, ctx, p),
        "kopano.leave" => {
            let req = json!({"op": "leave", "project": text_param(p, "project", "")});
            app.kopano.client.request(ctx, "Leaving…", req).map(|()| json!({"started": true}))
        }
        "kopano.removePerson" => {
            let req = json!({"op": "remove-person", "project": text_param(p, "project", ""), "user": text_param(p, "user", "")});
            app.kopano.client.request(ctx, "Taking them off…", req).map(|()| json!({"started": true}))
        }
        "kopano.share" => share(app, ctx, p),
        "kopano.join" => join(app, ctx, p),
        "kopano.getOriginals" => {
            let project = match text_param(p, "project", "") {
                s if s.is_empty() => match linked_project(app) {
                    Some(id) => id,
                    None => return Some(Err("This window is not part of a shared project yet".to_string())),
                },
                s => s,
            };
            let key = text_param(p, "key", "all");
            let req = json!({"op": "fetch-original", "project": project, "key": key});
            app.kopano.client.request(ctx, "Asking for the originals…", req).map(|()| json!({"started": true}))
        }
        "kopano.mode" => {
            let m = text_param(p, "mode", "approve");
            if m != "live" && m != "approve" {
                return Some(Err("The way to work is live or approve".to_string()));
            }
            app.kopano.share_mode = m;
            Ok(json!({"mode": app.kopano.share_mode}))
        }
        "kopano.link" => link(app, ctx, p),
        "kopano.approve" => review_op(app, ctx, "approve", p),
        "kopano.decline" => review_op(app, ctx, "decline", p),
        "kopano.view" => review_op(app, ctx, "view", p),
        "kopano.choose" => {
            let (project, member, key, pick) =
                (text_param(p, "project", ""), text_param(p, "member", ""), text_param(p, "key", ""), text_param(p, "pick", "member"));
            if project.is_empty() || member.is_empty() || key.is_empty() || (pick != "member" && pick != "main") {
                return Some(Err("Say which project, whose changes, which clip setting, and member or main".to_string()));
            }
            app.kopano.choices.insert((project, member, key), pick.clone());
            Ok(json!({"pick": pick}))
        }
        "kopano.setApprover" => {
            let req = json!({"op": "set-approver", "project": text_param(p, "project", ""), "account": text_param(p, "account", ""),
                "approver": p.get("approver").and_then(Value::as_bool).unwrap_or(true)});
            app.kopano.client.request(ctx, "Saving…", req).map(|()| json!({"started": true}))
        }
        "kopano.resolve" => {
            let req =
                json!({"op": "resolve", "project": text_param(p, "project", ""), "clash": text_param(p, "clash", ""), "pick": text_param(p, "pick", "a")});
            app.kopano.client.request(ctx, "Settling…", req).map(|()| json!({"started": true}))
        }
        "kopano.status" => {
            let state = app.kopano.client.state();
            let mut v = serde_json::to_value(&state).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                o.insert("shareMode".into(), json!(app.kopano.share_mode));
                o.insert("open".into(), json!(app.kopano.open));
                o.insert("advanced".into(), json!(app.kopano.advanced));
                o.insert("controlPort".into(), json!(app.kopano.control_port));
                o.insert("apiPort".into(), json!(app.kopano.client.config.port));
                o.insert("linkedProject".into(), json!(linked_project(app)));
            }
            Ok(v)
        }
        _ => return None,
    })
}

/// The shared project this window is linked to (its binding names this window's port).
fn linked_project(app: &FilmcraftApp) -> Option<String> {
    let port = app.kopano.control_port?;
    kopano::lock(&app.kopano.client.shared).projects.iter().find(|p| p.app_port == Some(port)).map(|p| p.id.clone())
}

/// `kopano.share {name, email, mode}`: share this film project; `email` names the teammates who
/// may join (comma separated; `account` is the same). Without params, the window's fields.
fn share(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let name = text_param(p, "name", &app.kopano.share_name);
    let accounts: Vec<String> =
        text_param(p, "email", &text_param(p, "account", &app.kopano.share_account)).split([',', ' ', ';']).map(str::trim).filter(|a| !a.is_empty()).map(str::to_string).collect();
    if name.is_empty() {
        return Err("Give the shared project a name".to_string());
    }
    if accounts.is_empty() {
        return Err("Type your teammate's email, so they can join".to_string());
    }
    let mode = text_param(p, "mode", &app.kopano.share_mode);
    if mode != "live" && mode != "approve" {
        return Err("The way to work is live or approve".to_string());
    }
    let port = control_port(app)?;
    // an email gets a ThabangVision invite; a bare account id (lab tests) is a member at once
    let (emails, ids): (Vec<String>, Vec<String>) = accounts.into_iter().partition(|a| a.contains('@'));
    let members: Vec<Value> = ids.iter().map(|a| json!({"account": a, "role": "editor", "lanes": []})).collect();
    let mut reqs = vec![json!({"op": "create-project", "name": name, "app": {"kind": kopano::BINDING, "port": port}, "members": members, "mode": mode})];
    reqs.extend(emails.iter().map(|e| json!({"op": "invite", "project": "$project", "email": e})));
    app.kopano.client.request_chain(ctx, "Sharing…", reqs)?;
    Ok(json!({"started": true, "controlPort": port}))
}

/// `kopano.invite {project, email}`: invite someone by email to a project this person owns.
/// The answer is always "Invite sent." (whether the address has an account is not told).
fn invite(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let project = match text_param(p, "project", "") {
        s if s.is_empty() => linked_project(app).ok_or("This window is not part of a shared project yet")?,
        s => s,
    };
    let email = text_param(p, "email", &app.kopano.invite_email);
    if !email.contains('@') {
        return Err("Type your teammate's email address".to_string());
    }
    app.kopano.client.request(ctx, "Inviting…", json!({"op": "invite", "project": project, "email": email}))?;
    app.kopano.invite_email.clear();
    Ok(json!({"started": true}))
}

/// `kopano.joinShared {shared}`: open a project from "Shared with me" in this window, nothing
/// typed: the service finds the owner's computer and joins from it.
fn join_shared(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let shared = text_param(p, "shared", "");
    if shared.is_empty() {
        return Err("Say which shared project".to_string());
    }
    let port = control_port(app)?;
    let req = json!({"op": "join-shared", "shared": shared, "app": {"kind": kopano::BINDING, "port": port}});
    app.kopano.client.request(ctx, "Opening the shared project…", req)?;
    Ok(json!({"started": true, "controlPort": port}))
}

/// A shared project is linked to this window but the person opened another project in it.
fn another_project_open(app: &FilmcraftApp) -> bool {
    app.kopano.bound.values().any(|d| *d != app.session.document)
}

/// `kopano.link {project}`: link this window to a shared project whose window is gone ("Link
/// this window"). Only the person does this for an unsaved window: nothing guesses it.
fn link(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let project = text_param(p, "project", "");
    let state = app.kopano.client.state();
    let Some(shared) = state.projects.iter().find(|x| x.id == project) else {
        return Err("That shared project is not on this computer".to_string());
    };
    let name = shared.name.clone();
    let port = control_port(app)?;
    app.kopano.attached.insert(project.clone());
    app.kopano.bound.insert(project.clone(), app.session.document);
    app.kopano.away.remove(&project);
    app.kopano.client.attach(ctx, &project, &name, port);
    Ok(json!({"started": true}))
}

/// `kopano.approve|decline|view {project, member, version, show}`: what the review buttons run.
/// Approve sends the owner's picks for the clashes of that member's changes.
fn review_op(app: &mut FilmcraftApp, ctx: &egui::Context, op: &str, p: &Value) -> Result<Value, String> {
    let project = text_param(p, "project", "");
    let member = text_param(p, "member", "");
    let show = text_param(p, "show", "member");
    if project.is_empty() || (member.is_empty() && show != "off") {
        return Err("Say which project and whose changes".to_string());
    }
    let mut req = json!({"op": op, "project": project, "member": member});
    if let Some(v) = p.get("version").and_then(Value::as_str) {
        req["version"] = json!(v);
    }
    let busy = match op {
        "approve" => {
            let mut choices = serde_json::Map::new();
            for ((pid, who, key), pick) in &app.kopano.choices {
                if *pid == project && *who == member {
                    choices.insert(key.clone(), json!(pick));
                }
            }
            if let Some(c) = p.get("choices").and_then(Value::as_object) {
                choices.extend(c.clone());
            }
            req["choices"] = Value::Object(choices);
            "Approving…"
        }
        "decline" => "Declining…",
        _ => {
            req["show"] = json!(show);
            "Showing…"
        }
    };
    app.kopano.client.request(ctx, busy, req)?;
    if op == "approve" || op == "decline" {
        // only this member's picks are used up; another member's stay for their review
        app.kopano.choices.retain(|(pid, who, _), _| *pid != project || *who != member);
    }
    Ok(json!({"started": true}))
}

/// `kopano.join {project, from}`: join a teammate's shared project into this window.
fn join(app: &mut FilmcraftApp, ctx: &egui::Context, p: &Value) -> Result<Value, String> {
    let project = text_param(p, "project", &app.kopano.join_project).to_lowercase();
    let from = text_param(p, "from", &app.kopano.join_from).to_lowercase();
    if !kopano::is_hex_id(&project) {
        return Err("The project id is 64 letters and digits (your teammate copies it under Advanced)".to_string());
    }
    if !kopano::is_hex_id(&from) {
        return Err("The teammate's device id is 64 letters and digits (they copy it under Advanced)".to_string());
    }
    let port = control_port(app)?;
    let req = json!({"op": "join", "project": project, "from": from, "app": {"kind": kopano::BINDING, "port": port}});
    app.kopano.client.request(ctx, "Joining…", req)?;
    Ok(json!({"started": true, "controlPort": port}))
}

/// Two people, drawn in code (our own original drawing): the header's Share & Collaborate icon.
pub fn paint_icon(p: &egui::Painter, r: Rect, color: Color32) {
    let k = r.width().min(r.height()) / 18.0;
    let at = |x: f32, y: f32| Pos2::new(r.min.x + x * k, r.min.y + y * k);
    let s = Stroke::new(1.4 * k.max(0.7), color);
    // a shoulder line: a half ellipse under a head
    let shoulders = |cx: f32, top: f32, w: f32| -> Vec<Pos2> {
        (0..=12)
            .map(|i| {
                let a = std::f32::consts::PI * (i as f32 / 12.0);
                at(cx - w * a.cos(), top + 5.0 - 5.0 * a.sin())
            })
            .collect()
    };
    // the one behind, then the one in front
    p.circle_stroke(at(12.0, 5.2), 2.4 * k, s);
    p.add(egui::Shape::line(shoulders(12.5, 10.5, 4.2), s));
    p.circle_stroke(at(6.8, 6.6), 2.8 * k, s);
    p.add(egui::Shape::line(shoulders(6.8, 12.6, 5.0), s));
}

/// The header button (right cluster): opens or closes the window. Returns its width used.
pub fn header_button(app: &mut FilmcraftApp, ui: &mut egui::Ui, r: Rect) {
    let t = app.tokens;
    let resp = ui.interact(r, egui::Id::new(("hdr", "collaborate")), Sense::click()).on_hover_text(crate::i18n::t(TITLE));
    app.auto.add("header.collaborate", r, TITLE);
    if resp.hovered() || app.kopano.open {
        ui.painter().rect_filled(r, 4.0, t.hover);
    }
    paint_icon(ui.painter(), r.shrink(5.0), if resp.hovered() || app.kopano.open { t.text } else { t.text_dim });
    if resp.clicked() {
        let ctx = ui.ctx().clone();
        if let Err(e) = crate::menus::invoke(app, &ctx, "kopano.panel", json!({})) {
            app.ui.status = e;
        }
    }
}

/// What the Project panel's badge says for a media item that is missing here because its footage
/// is on its way from a teammate (`None`: not shared footage; Film's own words stay).
pub fn footage_badge(app: &FilmcraftApp, item: &filmcraft_project::ProjectItem) -> Option<&'static str> {
    let port = app.kopano.control_port?;
    let state = kopano::lock(&app.kopano.client.shared);
    let footage = state.footage_for_port(Some(port))?;
    let file_name = item.as_media().and_then(|m| match &m.media {
        filmcraft_project::MediaRef::File { path } => std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()),
        filmcraft_project::MediaRef::Generator(_) => None,
    });
    let f = footage.file_named(&item.name).or_else(|| file_name.as_deref().and_then(|n| footage.file_named(n)))?;
    f.on_its_way().then(|| f.badge_text())
}

fn dot(ui: &mut egui::Ui, on: bool, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 14.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, if on { GREEN } else { t.text_faint });
}

fn heading(ui: &mut egui::Ui, text: &str, t: &Tokens) {
    ui.add_space(6.0);
    ui.label(RichText::new(crate::i18n::t(text)).font(Tokens::semibold(12.5)).color(t.text));
}

fn dim(ui: &mut egui::Ui, text: impl Into<String>, t: &Tokens) {
    ui.add(egui::Label::new(RichText::new(text.into()).color(t.text_dim).size(11.5)).wrap());
}

fn line(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    ui.add(egui::Label::new(RichText::new(text.into()).color(color).size(11.5)).wrap());
}

/// A button registered for agents and tests as `collab.<id>`; `selected` draws it as the chosen one.
fn button(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, label: &str, selected: bool) -> egui::Response {
    let t = app.tokens;
    let text = RichText::new(crate::i18n::t(label)).color(if selected { Color32::WHITE } else { t.text });
    let mut b = egui::Button::new(text).min_size(vec2(0.0, 22.0));
    if selected {
        b = b.fill(t.accent);
    }
    let r = ui.add(b);
    app.auto.add(&format!("collab.{id}"), r.rect, label);
    r
}

/// A text field registered as `collab.<id>`.
fn field(app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, hint: &str, which: Field) {
    let text = match which {
        Field::ShareName => &mut app.kopano.share_name,
        Field::ShareAccount => &mut app.kopano.share_account,
        Field::InviteEmail => &mut app.kopano.invite_email,
        Field::JoinProject => &mut app.kopano.join_project,
        Field::JoinFrom => &mut app.kopano.join_from,
    };
    let r = ui.add(egui::TextEdit::singleline(text).id(egui::Id::new(("collab-field", id))).hint_text(crate::i18n::t(hint)).desired_width(W - 12.0));
    app.auto.add(&format!("collab.{id}"), r.rect, hint);
}

#[derive(Clone, Copy)]
enum Field {
    ShareName,
    ShareAccount,
    InviteEmail,
    JoinProject,
    JoinFrom,
}

/// Copy `text` to the clipboard (and say so).
fn copy(app: &mut FilmcraftApp, ctx: &egui::Context, text: &str) {
    ctx.copy_text(text.to_string());
    app.kopano.copied = Some(text.to_string());
    app.toast = Some((crate::i18n::t("Copied").to_string(), ctx.input(|i| i.time)));
}

/// Run a button's command; a refusal shows on the window's last line.
fn press(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: Value) {
    if let Err(e) = crate::menus::invoke(app, ctx, id, params) {
        set_job(app, false, e);
    }
}

/// A clip or track of this window's project in plain words: its name, else "a clip" (never
/// the service's key).
fn item_name(app: &FilmcraftApp, key: &str) -> String {
    let mut parts = key.split('/');
    let (Some("film"), Some(_seq), Some(kind), Some(id)) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return crate::i18n::t("a clip").to_string();
    };
    let Ok(id) = id.parse::<u64>() else { return crate::i18n::t("a clip").to_string() };
    for s in app.session.project.sequences() {
        let Some(seq) = s.as_sequence() else { continue };
        for tr in seq.video_tracks.iter().chain(seq.audio_tracks.iter()) {
            if kind == "track" && tr.id.0 == id {
                return tr.name.clone();
            }
            if kind == "clip"
                && let Some(c) = tr.items.iter().find(|c| c.id.0 == id)
            {
                return c.name.clone();
            }
        }
    }
    format!("{kind} {}", kopano::short(&id.to_string()))
}

/// The "I approve changes" part of one project: who approves, what waits, where my work is.
fn review_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &kopano::Project, r: &kopano::Review) {
    let t = app.tokens;
    let tag = kopano::short(&p.id).to_string();
    let who = if r.is_owner { crate::i18n::t("You approve changes").to_string() } else { format!("{} {}", r.owner_name, crate::i18n::t("approves changes")) };
    dim(ui, who, &t);
    if let Some(text) = r.mine_text() {
        let declined = r.mine.as_ref().is_some_and(|m| m.state == "declined");
        line(ui, text, if declined { CAUTION } else { t.text });
    }
    if !r.approver {
        return;
    }
    heading(ui, "Changes to review", &t);
    if r.pending.is_empty() {
        dim(ui, crate::i18n::t("Nothing waiting."), &t);
    }
    for e in &r.pending {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{} changed {}", e.name, e.summary())).color(t.text));
        });
        let viewing = r.viewing.as_ref().filter(|v| v.account == e.account);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let base = json!({"project": p.id, "member": e.account, "version": e.version});
            if viewing.is_none() {
                if button(app, ui, &format!("view.{tag}.{}", e.account), "View", false).clicked() {
                    let mut q = base.clone();
                    q["show"] = json!("member");
                    press(app, ctx, "kopano.view", q);
                }
            } else {
                let side = viewing.map(|v| v.side.as_str()).unwrap_or("member");
                if button(app, ui, &format!("before.{tag}"), "Before", side == "main").clicked() {
                    let mut q = base.clone();
                    q["show"] = json!("main");
                    press(app, ctx, "kopano.view", q);
                }
                if button(app, ui, &format!("after.{tag}"), "After", side == "member").clicked() {
                    let mut q = base.clone();
                    q["show"] = json!("member");
                    press(app, ctx, "kopano.view", q);
                }
                if button(app, ui, &format!("viewDone.{tag}"), "Done", false).clicked() {
                    press(app, ctx, "kopano.view", json!({"project": p.id, "show": "off"}));
                }
            }
            if button(app, ui, &format!("approve.{tag}.{}", e.account), "Approve", false).clicked() {
                press(app, ctx, "kopano.approve", base.clone());
            }
            if button(app, ui, &format!("decline.{tag}.{}", e.account), "Decline", false).clicked() {
                press(app, ctx, "kopano.decline", base.clone());
            }
        });
        if let Some(v) = viewing {
            let side = if v.side == "main" { crate::i18n::t("your version (before)").to_string() } else { format!("{}'s version (after)", e.name) };
            dim(ui, format!("{} {side}. {}", crate::i18n::t("This window shows"), crate::i18n::t("Editing waits until you press Done.")), &t);
        }
        for (i, c) in e.clashes.iter().enumerate() {
            let pick = app.kopano.choices.get(&(p.id.clone(), e.account.clone(), c.key.clone())).cloned().unwrap_or_else(|| "member".to_string());
            dim(
                ui,
                format!("{} · {}: {} {} · {} {}", item_name(app, &c.item), kopano::setting_text(&c.setting), crate::i18n::t("yours"), c.main, e.name, c.member),
                &t,
            );
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                if button(app, ui, &format!("keepMain.{tag}.{}.{i}", e.account), "Keep yours", pick == "main").clicked() {
                    press(app, ctx, "kopano.choose", json!({"project": p.id, "member": e.account, "key": c.key, "pick": "main"}));
                }
                if button(app, ui, &format!("takeMember.{tag}.{}.{i}", e.account), "Take theirs", pick == "member").clicked() {
                    press(app, ctx, "kopano.choose", json!({"project": p.id, "member": e.account, "key": c.key, "pick": "member"}));
                }
            });
        }
    }
    if !r.declined.is_empty() {
        dim(ui, format!("{} {}", crate::i18n::t("Declined:"), r.declined.join(", ")), &t);
    }
}

/// A live project's clashes: both values of the setting, pick one.
fn clash_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &kopano::Project) {
    let t = app.tokens;
    heading(ui, "Clashes", &t);
    dim(ui, crate::i18n::t("You and a teammate changed the same setting at once. Pick the one to keep."), &t);
    let tag = kopano::short(&p.id).to_string();
    for (i, c) in p.clashes.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{} · {}", item_name(app, &c.item), kopano::setting_text(&c.setting))).color(t.text));
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if button(app, ui, &format!("pickA.{tag}.{i}"), &c.a, false).clicked() {
                press(app, ctx, "kopano.resolve", json!({"project": p.id, "clash": c.id, "pick": "a"}));
            }
            if button(app, ui, &format!("pickB.{tag}.{i}"), &c.b, false).clicked() {
                press(app, ctx, "kopano.resolve", json!({"project": p.id, "clash": c.id, "pick": "b"}));
            }
        });
    }
}

/// The footage of one shared project on this computer: what plays from where, Get originals.
fn footage_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &kopano::Project, f: &kopano::Footage) {
    let t = app.tokens;
    heading(ui, "Footage", &t);
    dim(ui, f.summary(), &t);
    if f.all_here() {
        return;
    }
    for file in &f.files {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let color = if file.failed { CAUTION } else { t.text };
            ui.add(egui::Label::new(RichText::new(file.line()).color(color).size(11.5)).wrap());
        });
    }
    let busy = kopano::lock(&app.kopano.client.shared).job.as_ref().is_some_and(|j| j.busy);
    ui.horizontal(|ui| {
        let r = ui
            .add_enabled_ui(f.can_get_originals() && !busy, |ui| button(app, ui, &format!("getOriginals.{}", kopano::short(&p.id)), "Get originals", false))
            .inner;
        if r.clicked() {
            press(app, ctx, "kopano.getOriginals", json!({"project": p.id}));
        }
    });
}

/// How often the background work runs.
const TICK_MS: f64 = 2000.0;

/// This window's project as a [`kopano::Record`]: its shared project, file and footage names.
fn our_record(app: &FilmcraftApp) -> kopano::Record {
    let footage = app
        .session
        .project
        .items
        .values()
        .filter_map(|it| match &it.as_media()?.media {
            filmcraft_project::MediaRef::File { path } => std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()),
            filmcraft_project::MediaRef::Generator(_) => None,
        })
        .collect();
    kopano::Record { share: app.session.project.share_id.clone(), file: app.session.path.clone(), footage }
}

/// Background work each frame, open or not (only where the window can be reached: the desktop
/// app): follow the service, remember which project file each project linked to this window is,
/// after a restart on a new port link the projects this window holds back to it (`attach`), and
/// put away Film's Link Media prompt when the footage is on its way from a teammate.
pub fn tick(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if app.kopano.start_control.is_none() && app.kopano.control_port.is_none() {
        return;
    }
    let now = kopano::now_ms();
    if now - app.kopano.last_tick < TICK_MS {
        return;
    }
    app.kopano.last_tick = now;
    app.kopano.client.start(ctx);
    let state = app.kopano.client.state();
    let Some(home) = app.kopano.client.config.home.clone().filter(|_| state.running) else { return };
    let mine = app.kopano.control_port;
    for p in &state.projects {
        let Some(port) = p.app_port else { continue };
        if Some(port) == mine {
            let ours = our_record(app);
            let doc = app.session.document;
            if *app.kopano.bound.entry(p.id.clone()).or_insert(doc) != doc {
                // the person opened another project here: the shared one stays linked to its own
                // file and is not written over; opening that file again links it again
                let recorded = app.kopano.recorded.get(&p.id).cloned().unwrap_or_else(|| kopano::read_record(&home, &p.id));
                if kopano::holds(&recorded, &ours) {
                    app.kopano.bound.insert(p.id.clone(), doc);
                    app.kopano.away.remove(&p.id);
                    kopano::lock(&app.kopano.client.shared).note(Some(p.id.clone()), kopano::Said::Reattached { name: p.name.clone() }, now);
                } else {
                    if app.kopano.away.insert(p.id.clone()) {
                        let said = kopano::Said::Away { name: p.name.clone() };
                        app.ui.status = said.text();
                        kopano::lock(&app.kopano.client.shared).note(Some(p.id.clone()), said, now);
                    }
                    continue;
                }
            }
            if app.session.project.share_id.as_deref() != Some(p.id.as_str()) {
                // the project remembers which shared project it is (saved with it, kept by
                // crash recovery), so this window finds it again even before the first save
                std::sync::Arc::make_mut(&mut app.session.project).share_id = Some(p.id.clone());
                app.session.revision += 1;
            }
            let ours = our_record(app);
            if app.kopano.recorded.get(&p.id) != Some(&ours) {
                match kopano::write_record(&home, &p.id, &ours) {
                    Ok(()) => {
                        app.kopano.recorded.insert(p.id.clone(), ours);
                    }
                    Err(e) => log::warn!("collaboration: could not remember the project of {}: {e}", p.name),
                }
            }
            // the footage is coming: Film's "where is the media?" prompt is not the person's job
            if p.footage.as_ref().is_some_and(|f| !f.all_here()) && app.session.offline.prompt && !app.kopano.link_prompt_closed.contains(&p.id) {
                app.kopano.link_prompt_closed.insert(p.id.clone());
                app.session.offline.prompt = false;
                app.ui.link_media = None;
                app.ui.status = crate::i18n::t("The footage is on its way from your teammate; the clips play from proxies first.").to_string();
            }
        } else if p.app_alive != Some(false) {
            // linked to another window now
            app.kopano.bound.remove(&p.id);
            app.kopano.away.remove(&p.id);
        } else if !app.kopano.attached.contains(&p.id) {
            if !kopano::holds(&kopano::read_record(&home, &p.id), &our_record(app)) {
                continue;
            }
            app.kopano.attached.insert(p.id.clone());
            match control_port(app) {
                Ok(port) => app.kopano.client.attach(ctx, &p.id, &p.name, port),
                Err(error) => kopano::lock(&app.kopano.client.shared).note(Some(p.id.clone()), kopano::Said::AppProblem { error }, now),
            }
        }
    }
}

pub fn show(app: &mut FilmcraftApp, ctx: &egui::Context) {
    tick(app, ctx);
    if !app.kopano.open {
        return;
    }
    app.kopano.client.start(ctx);
    let state = app.kopano.client.state();
    let mut open = true;
    let screen = ctx.content_rect();
    let x = (screen.right() - W - 40.0).max(screen.left() + 8.0);
    let shown = egui::Window::new(crate::i18n::t(TITLE))
        .id(egui::Id::new("collab-panel"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(egui::pos2(x, screen.top() + 48.0))
        .default_width(W)
        .show(ctx, |ui| {
            ui.set_width(W);
            ui.spacing_mut().item_spacing.y = 5.0;
            body(app, ui, ctx, &state);
        });
    if let Some(r) = shown {
        app.auto.add("collab.window", r.response.rect, TITLE);
    }
    if !open {
        app.kopano.open = false;
    }
    // "3 s ago" keeps counting
    ctx.request_repaint_after(std::time::Duration::from_secs(1));
}

fn body(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, state: &State) {
    let t = app.tokens;
    match state.me.as_ref().filter(|_| state.running) {
        Some(me) if state.signed_in => {
            ui.horizontal(|ui| {
                dot(ui, true, &t);
                ui.label(RichText::new(format!("{} {}", crate::i18n::t("Signed in to ThabangVision as"), me.name)).font(Tokens::semibold(12.5)).color(t.text));
                if button(app, ui, "signOut", "Sign out", false).clicked() {
                    press(app, ctx, "kopano.signOut", json!({}));
                }
            });
        }
        // the service runs, but this computer is not signed in to ThabangVision
        Some(_) => {
            match &state.signing_in {
                Some(si) => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(crate::i18n::t("Check that the browser shows")).color(t.text));
                        ui.label(RichText::new(&si.code).font(Tokens::semibold(15.0)).color(t.text));
                    });
                    dim(ui, crate::i18n::t("Approve this computer there. This window carries on by itself."), &t);
                    ui.horizontal(|ui| {
                        if button(app, ui, "signInPage", "Open the page again", false).clicked() {
                            press(app, ctx, "kopano.signInPage", json!({}));
                        }
                        if button(app, ui, "signInCancel", "Cancel", false).clicked() {
                            press(app, ctx, "kopano.signInCancel", json!({}));
                        }
                    });
                }
                None => {
                    if let Some(m) = &state.sign_in_message {
                        line(ui, kopano::for_people(m), CAUTION);
                    }
                    dim(ui, crate::i18n::t("Work on this film together with your team."), &t);
                    if button(app, ui, "signIn", "Sign in with ThabangVision", false).clicked() {
                        press(app, ctx, "kopano.signIn", json!({}));
                    }
                }
            }
            // nothing else works before the sign-in
            job_line(app, ui, state);
            return;
        }
        None => {
            ui.horizontal(|ui| {
                dot(ui, false, &t);
                ui.label(RichText::new(crate::i18n::t(kopano::NOT_RUNNING)).font(Tokens::semibold(12.5)).color(t.text));
            });
            dim(ui, crate::i18n::t("Start ThabangVision collaboration on this computer. This window finds it by itself."), &t);
            if let Some(p) = &state.problem
                && p != kopano::NOT_RUNNING
            {
                dim(ui, kopano::for_people(p), &t);
            }
        }
    }

    heading(ui, "Shared projects", &t);
    if state.projects.is_empty() {
        dim(ui, crate::i18n::t("Nothing shared yet."), &t);
    }
    let mine = app.kopano.control_port;
    for p in &state.projects {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&p.name).font(Tokens::semibold(12.0)).color(t.text));
            if mine.is_some() && p.app_port == mine {
                ui.label(RichText::new(crate::i18n::t("in this window")).color(t.text_dim).size(11.0));
            }
        });
        if app.kopano.away.contains(&p.id) {
            line(
                ui,
                format!("{} {}.", crate::i18n::t("Another project is open in this window. Nothing of it is shared. To keep working together, open"), p.name),
                CAUTION,
            );
        } else if p.app_alive == Some(false) && !app.kopano.attached.contains(&p.id) && !state.projects.iter().any(|x| mine.is_some() && x.app_port == mine) {
            // its window is gone (Film restarted): the person says which window holds it
            dim(ui, crate::i18n::t("No window has it open. Open its project file, or link this window if it holds this project."), &t);
            let tag = kopano::short(&p.id).to_string();
            if button(app, ui, &format!("link.{tag}"), "Link this window", false).clicked() {
                press(app, ctx, "kopano.link", json!({"project": p.id}));
            }
        }
        if let Some(people) = &p.people {
            people_section(app, ui, ctx, p, people, state.running);
        } else {
            if p.teammates.is_empty() {
                dim(ui, crate::i18n::t("No teammate has joined yet."), &t);
            }
            for m in &p.teammates {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let on = m.online && state.running;
                    dot(ui, on, &t);
                    ui.label(RichText::new(m.label()).color(t.text));
                    ui.label(RichText::new(crate::i18n::t(if on { "online" } else { "offline" })).color(t.text_dim).size(11.5));
                });
            }
        }
        match &p.review {
            Some(r) => review_section(app, ui, ctx, p, r),
            None if p.mode.as_deref() == Some("live") => dim(ui, crate::i18n::t("Works live: every edit goes to everyone."), &t),
            None => {}
        }
        if !p.clashes.is_empty() {
            clash_section(app, ui, ctx, p);
        }
        if let Some(f) = &p.footage {
            footage_section(app, ui, ctx, p, f);
        }
    }

    if !state.shared_with_me.is_empty() {
        heading(ui, "Shared with me", &t);
        let linked = mine.is_some() && state.projects.iter().any(|x| x.app_port == mine);
        for r in &state.shared_with_me {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&r.name).font(Tokens::semibold(12.0)).color(t.text));
                if !linked && button(app, ui, &format!("openShared.{}", kopano::short(&r.shared)), "Open here", false).clicked() {
                    press(app, ctx, "kopano.joinShared", json!({"shared": r.shared}));
                }
            });
        }
        if linked {
            dim(ui, crate::i18n::t("This window already holds a shared project. Open a new window to open one of these."), &t);
        }
    }

    heading(ui, "Recent activity", &t);
    if state.activity.is_empty() {
        dim(ui, crate::i18n::t("Nothing yet."), &t);
    }
    let now = kopano::now_ms();
    // while another project is open here, "lost touch" lines about the shared one say nothing new
    let quiet = |a: &&kopano::Activity| !(matches!(a.said, kopano::Said::AppProblem { .. }) && a.project.as_ref().is_some_and(|id| app.kopano.away.contains(id)));
    for a in state.activity.iter().filter(quiet).take(SHOWN) {
        let times = if a.times > 1 { format!(" · {} times", a.times) } else { String::new() };
        let text = format!("{}{times} · {}", a.said.text(), kopano::ago((now - a.at_ms) / 1000.0));
        let warn = matches!(a.said, kopano::Said::AppProblem { .. } | kopano::Said::OriginalFailed { .. } | kopano::Said::Merged { clashes: 1.., .. });
        line(ui, text, if warn { CAUTION } else { t.text });
    }

    ui.add_space(4.0);
    ui.separator();
    let linked = mine.and_then(|port| state.projects.iter().find(|p| p.app_port == Some(port)));
    if let Some(p) = linked {
        // this window already is a shared project: a second share would drive it twice
        heading(ui, "Share this project", &t);
        dim(ui, format!("{} {}. {}", crate::i18n::t("This window is part of"), p.name, crate::i18n::t("To share another film, open it in a new window.")), &t);
    } else {
        heading(ui, "Share this project", &t);
        field(app, ui, "shareName", "Name for the shared project", Field::ShareName);
        field(app, ui, "shareAccount", "Teammate's email", Field::ShareAccount);
        ui.horizontal(|ui| {
            let approve = app.kopano.share_mode != "live";
            if button(app, ui, "modeApprove", "I approve changes", approve).clicked() {
                app.kopano.share_mode = "approve".to_string();
            }
            if button(app, ui, "modeLive", "Work together live", !approve).clicked() {
                app.kopano.share_mode = "live".to_string();
            }
        });
        dim(
            ui,
            crate::i18n::t(if app.kopano.share_mode == "live" {
                "Every edit goes to everyone at once."
            } else {
                "Your teammates see their own edits at once. Your project changes only when you approve."
            }),
            &t,
        );
        dim(ui, crate::i18n::t("Your teammate gets small proxies of the footage first; the originals come when they ask."), &t);
        if button(app, ui, "share", "Share", false).clicked() {
            press(app, ctx, "kopano.share", json!({}));
        }
    }

    ui.add_space(4.0);
    let header = egui::CollapsingHeader::new(RichText::new(crate::i18n::t("Advanced (testing)")).color(t.text_dim).size(11.5))
        .id_salt("collab-advanced")
        .open(Some(app.kopano.advanced))
        .show(ui, |ui| advanced(app, ui, state, ctx));
    app.auto.add("collab.advanced", header.header_response.rect, "Advanced (testing)");
    if header.header_response.clicked() {
        app.kopano.advanced = !app.kopano.advanced;
    }

    job_line(app, ui, state);
}

/// The last Share, Join, Invite… and how it went.
fn job_line(app: &FilmcraftApp, ui: &mut egui::Ui, state: &State) {
    let t = app.tokens;
    if let Some(job) = &state.job {
        ui.add_space(4.0);
        let color = if job.busy {
            t.text_dim
        } else if job.ok {
            t.text
        } else {
            CAUTION
        };
        line(ui, if job.ok { job.text.clone() } else { kopano::for_people(&job.text) }, color);
    }
}

/// The people on a project shared on ThabangVision: names (never emails), what they may do,
/// online or not. The owner invites by email and can take someone off; others can leave.
fn people_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &kopano::Project, people: &[kopano::Person], running: bool) {
    let t = app.tokens;
    let tag = kopano::short(&p.id).to_string();
    let owner = people.iter().any(|x| x.me && x.role == "owner");
    if people.iter().all(|x| x.me) {
        dim(ui, crate::i18n::t("No one else is on it yet."), &t);
    }
    for x in people.iter().filter(|x| !x.me) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let on = x.online && running && !x.invited;
            dot(ui, on, &t);
            ui.label(RichText::new(&x.name).color(t.text));
            ui.label(RichText::new(crate::i18n::t(x.role_text())).color(t.text_dim).size(11.5));
            if owner && button(app, ui, &format!("remove.{tag}.{}", x.user), "Remove", false).clicked() {
                press(app, ctx, "kopano.removePerson", json!({"project": p.id, "user": x.user}));
            }
        });
    }
    if owner {
        field(app, ui, &format!("inviteEmail.{tag}"), "Teammate's email", Field::InviteEmail);
        if button(app, ui, &format!("invite.{tag}"), "Invite", false).clicked() {
            press(app, ctx, "kopano.invite", json!({"project": p.id}));
        }
    } else if button(app, ui, &format!("leave.{tag}"), "Leave this project", false).clicked() {
        press(app, ctx, "kopano.leave", json!({"project": p.id}));
    }
}

/// Ids and the id-based Join, until email invites reach the teammate by themselves.
fn advanced(app: &mut FilmcraftApp, ui: &mut egui::Ui, state: &State, ctx: &egui::Context) {
    let t = app.tokens;
    dim(ui, crate::i18n::t("For testing: until email invites arrive by themselves, your teammate joins with these ids."), &t);
    if let Some(me) = &state.me {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} {}", crate::i18n::t("Your device id"), kopano::short(&me.device))).color(t.text_dim).monospace());
            if button(app, ui, "copyDevice", "Copy", false).clicked() {
                copy(app, ctx, &me.device);
            }
        });
    }
    for p in &state.projects {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} {}", p.name, kopano::short(&p.id))).color(t.text_dim).monospace());
            if button(app, ui, &format!("copyProject.{}", kopano::short(&p.id)), "Copy", false).clicked() {
                copy(app, ctx, &p.id);
            }
        });
    }
    heading(ui, "Join a shared project", &t);
    field(app, ui, "joinProject", "Project id", Field::JoinProject);
    field(app, ui, "joinFrom", "Teammate's device id", Field::JoinFrom);
    if button(app, ui, "join", "Join", false).clicked() {
        press(app, ctx, "kopano.join", json!({}));
    }
}
