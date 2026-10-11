//! Ubunye Film brand layer: the product name, ids and links, our mark (the app icon) and
//! the run-time renaming of upstream's product name in interface text.
//!
//! Modified by Ubunye AI Ecosystems, 2026. Upstream's version of this file compiled in a
//! trademarked wordmark that a modified version may not ship, so this file replaces it. It is
//! restored by `python -I ubunye/brand/apply.py` after an upstream merge.
//! Generated: the values come from ubunye/brand/apply.py (the one place the product name is set)
//! and mirror `ubunye/brand.toml`. Edit the template in ubunye/brand/overlay/, not this file.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

/// The product name shown everywhere in the interface.
pub const NAME: &str = "Ubunye Film";
/// Short form, where upstream uses one.
pub const SHORT_NAME: &str = "Film";
/// The suite this app belongs to.
pub const SUITE: &str = "Ubunye Creative Studio";
/// Who distributes it.
pub const DISTRIBUTOR: &str = "ThabangVision";
/// The About line naming the product and its suite.
pub const PART_OF: &str = "Ubunye Film, part of Ubunye Creative Studio.";
/// Vendor / manufacturer.
pub const VENDOR: &str = "Ubunye AI Ecosystems";
/// Reverse-DNS app id: macOS bundle id, Linux desktop / icon / Wayland id, Flatpak id, data folder.
pub const APP_ID: &str = "org.ubunye.creative.film";
/// The licence of this distribution.
pub const LICENSE: &str = "MIT OR Apache-2.0";

/// Upstream's product name, replaced by [`NAME`] in interface text ([`localize`]).
pub const UPSTREAM_NAME: &str = "FilmCraft";
/// The plain-text upstream credit (About dialog). Never passed through [`localize`].
pub const BASED_ON: &str = "Based on FilmCraft by the ArtCraft team";
/// Upstream's repository, linked from the credit.
pub const UPSTREAM_URL: &str = "https://github.com/storytold/filmcraft";

/// The product page.
pub const PRODUCT_URL: &str = "https://www.thabangvision.com/tools/film";
/// The ThabangVision website (home of Ubunye Creative Studio).
pub const WEBSITE_URL: &str = "https://www.thabangvision.com";
/// The Ubunye community (help.discord, the header button, the About and Home links).
pub const COMMUNITY_URL: &str = "https://github.com/ubunye-ai-ecosystems";
/// The source repository.
pub const SOURCE_URL: &str = "https://github.com/ubunye-ai-ecosystems/ubunye-creative-film";
/// New issue on the repository.
pub const ISSUES_URL: &str = "https://github.com/ubunye-ai-ecosystems/ubunye-creative-film/issues";
/// The documentation (Help ▸ Help…, F1).
pub const DOCS_URL: &str = "https://github.com/ubunye-ai-ecosystems/ubunye-creative-film/tree/main/docs";

/// Our mark: the app icon (ubunye/brand/assets/icon.svg rendered at 256 px).
const MARK: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/org.ubunye.creative.film.png");

/// At most this many distinct renamed strings are kept (each is leaked once, for the life of the
/// process). Interface text is a bounded set; past the cap text is shown unrenamed.
const MAX_RENAMED: usize = 8192;

/// `text` with upstream's product name replaced by ours. Every translated interface string goes
/// through this (`i18n::Language::tr` and `i18n::tr_ctx`), so upstream strings and catalogs stay
/// untouched and new upstream strings are renamed without edits. Text without the name is
/// returned as is (no allocation).
pub fn localize(text: &str) -> &str {
    if !text.contains(UPSTREAM_NAME) {
        return text;
    }
    static RENAMED: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut map = RENAMED.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(s) = map.get(text) {
        return *s;
    }
    if map.len() >= MAX_RENAMED {
        return text;
    }
    let renamed: &'static str = Box::leak(text.replace(UPSTREAM_NAME, NAME).into_boxed_str());
    map.insert(text.to_string(), renamed);
    renamed
}

/// [`localize`] for owned text that is not a fixed interface string (status messages, errors).
pub fn localize_owned(text: String) -> String {
    if text.contains(UPSTREAM_NAME) { text.replace(UPSTREAM_NAME, NAME) } else { text }
}

/// Our mark, decoded once and kept as a texture.
pub fn mark(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let id = egui::Id::new("ubunye-mark");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let img = image::load_from_memory_with_format(MARK, image::ImageFormat::Png).ok()?.to_rgba8();
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    let tex = ctx.load_texture("ubunye-mark", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    Some(tex)
}

/// Draw the mark `size` points square with its left edge at `left_center` (vertically centred).
/// Returns the drawn rect.
pub fn paint_mark(ui: &egui::Ui, left_center: egui::Pos2, size: f32) -> Option<egui::Rect> {
    let tex = mark(ui.ctx())?;
    let r = egui::Rect::from_min_size(egui::pos2(left_center.x, left_center.y - size / 2.0), egui::vec2(size, size));
    ui.painter().image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
    Some(r)
}

/// Upstream's call sites draw a wordmark here (Home screen): our mark followed by the product
/// name, `height` points tall, left edge at `left_center`. `dark` picks the text colour. Returns
/// the drawn rect (mark and name).
pub fn paint_wordmark(ui: &egui::Ui, left_center: egui::Pos2, height: f32, dark: bool) -> Option<egui::Rect> {
    let size = height * 1.6;
    let m = paint_mark(ui, left_center, size)?;
    let ink = if dark { egui::Color32::from_gray(235) } else { egui::Color32::from_gray(20) };
    let text = ui.painter().text(egui::pos2(m.max.x + 8.0, left_center.y), egui::Align2::LEFT_CENTER, NAME, egui::FontId::proportional(height), ink);
    Some(m.union(text))
}
