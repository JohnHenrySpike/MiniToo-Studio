//! The 160×128 notification card (§10). PLACEHOLDER.

use crate::frame::Frame;

/// Card for `app` / `summary` / `body` with the icon looked up by `icon` (theme name or path),
/// then `app.to_lowercase()`, then `preferences-desktop-notification`. `time` is «HH:mm».
pub fn render(app: &str, summary: &str, body: &str, icon: &str, time: &str) -> Frame {
    let _ = (app, summary, body, icon, time);
    Frame::solid(0x10, 0x12, 0x1a)
}

/// Strips HTML tags, decodes `&amp; &lt; &gt;`, collapses whitespace.
pub fn clean_body(body: &str) -> String {
    body.to_string()
}
