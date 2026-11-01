//! Ubunye: no update check reaches upstream (an upstream FilmCraft "update" would replace Ubunye
//! Film), and Help > Check for Updates opens our product page instead of craft-update.

use std::sync::{Arc, Mutex};

use filmcraft_engine::Session;
use serde_json::json;

use crate::FilmcraftApp;

#[test]
fn the_updater_never_names_an_upstream_repository() {
    let app = &crate::UPDATE_APP;
    assert!(!app.repo.starts_with("storytold/"), "update checks must not target {}", app.repo);
    assert_eq!(app.repo, "ubunye-ai-ecosystems/ubunye-creative-film");
    assert_eq!(app.name, crate::brand::NAME);
}

#[test]
fn check_for_updates_opens_the_product_page_and_starts_no_check() {
    let mut app = FilmcraftApp::new(Session::default());
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&opened);
    app.hooks.open_url = Some(Box::new(move |url| {
        seen.lock().unwrap().push(url.to_string());
        Ok(())
    }));
    let ctx = egui::Context::default();
    crate::menus::invoke_command(&mut app, &ctx, "app.checkForUpdates", json!({})).unwrap();
    assert_eq!(*opened.lock().unwrap(), [crate::brand::PRODUCT_URL.to_string()]);
    assert!(!app.updates_rt.manual_window_open, "no craft-update window");
}
