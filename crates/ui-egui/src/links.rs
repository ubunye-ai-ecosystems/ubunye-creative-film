//! Community and project links (Help menu, About dialog, header Discord button, Home screen).
//!
//! Modified by Ubunye AI Ecosystems, 2026: the links are ours (crate::brand); the command ids
//! are upstream's, so `help.discord` opens the Ubunye community.

/// The app's short name, used in the website and repository URLs.
pub const APP: &str = "filmcraft";

/// The Ubunye community.
pub const DISCORD: &str = crate::brand::COMMUNITY_URL;
/// The ThabangVision website.
pub const WEBSITE: &str = crate::brand::WEBSITE_URL;
/// The product page.
pub const APP_PAGE: &str = crate::brand::PRODUCT_URL;
/// The source repository.
pub const GITHUB: &str = crate::brand::SOURCE_URL;
/// New issue on the repository.
pub const ISSUES: &str = crate::brand::ISSUES_URL;

/// (command id, label, url) for every link, in menu order.
pub const ALL: [(&str, &str, &str); 5] = [
    ("help.discord", "Join the Ubunye Community…", DISCORD),
    ("help.website", "ThabangVision Website", WEBSITE),
    ("help.appPage", "FilmCraft on thabangvision.com", APP_PAGE),
    ("help.github", "FilmCraft on GitHub", GITHUB),
    ("help.reportIssue", "Report an Issue…", ISSUES),
];

/// The URL a `help.*` link command opens.
pub fn url_for(command: &str) -> Option<&'static str> {
    ALL.iter().find(|(id, _, _)| *id == command).map(|(_, _, u)| *u)
}

/// Open `url` in the system browser (a new tab on the web).
pub fn open(ctx: &egui::Context, url: &str) {
    ctx.open_url(egui::OpenUrl::new_tab(url));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_ubunye_scheme() {
        assert_eq!(APP_PAGE, format!("{WEBSITE}/tools/film"));
        assert_eq!(GITHUB, "https://github.com/ubunye-ai-ecosystems/ubunye-creative-film");
        assert_eq!(APP, "filmcraft");
        assert!(ALL.iter().all(|(id, _, u)| id.starts_with("help.") && u.starts_with("https://")));
        assert_eq!(url_for("help.discord"), Some(DISCORD));
        assert_eq!(url_for("help.nope"), None);
    }
}
