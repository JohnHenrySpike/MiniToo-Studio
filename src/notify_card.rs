//! The 160×128 notification card (§10), also used for the Pomodoro phase-end overlay.

use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::Frame;
use crate::platform::notifications::{DesktopNotification, NotifyImage};

/// Where the card icon may come from: what a desktop notification carries (§10). An empty
/// source still finds the icon of the application by its name.
#[derive(Clone, Copy, Debug, Default)]
pub struct IconSource<'a> {
    /// `app_icon`: theme name, path or `file://` URL
    pub app_icon: &'a str,
    /// `desktop-entry` hint
    pub desktop_entry: &'a str,
    /// `image-path` hint
    pub image_path: &'a str,
    /// `image-data` hint
    pub image: Option<&'a NotifyImage>,
}

impl<'a> IconSource<'a> {
    pub fn named(app_icon: &'a str) -> Self {
        IconSource { app_icon, ..Default::default() }
    }

    pub fn of(n: &'a DesktopNotification) -> Self {
        IconSource { app_icon: &n.icon, desktop_entry: &n.desktop_entry, image_path: &n.image_path, image: n.image.as_ref() }
    }
}

/// Card for `app` / `summary` / `body` with the icon of the sender (see [`card_icon`]).
/// `time` is the time already formatted for display («20:48»).
pub fn render(app: &str, summary: &str, body: &str, icon: IconSource, time: &str) -> Frame {
    let mut c = Canvas::device();
    c.fill(Color::hex(0x10121a));
    c.fill_round_rect(5.0, 6.0, 150.0, 116.0, 10.0, Color::hex(0x202432));

    if let Some(img) = card_icon(app, icon) {
        let src = Canvas::from_image(&img);
        c.draw_canvas_scaled(&src, r(13.0, 14.0, 28.0, 28.0), true, 1.0);
    }

    let title_font = FontSpec::bold(12.0);
    let title = if summary.is_empty() { app } else { summary };
    c.text(r(48.0, 12.0, 102.0, 18.0), Align::LEFT, &Canvas::elide(title, title_font, 102.0), title_font, Color::WHITE);
    let meta_font = FontSpec::sans(9.0);
    let meta = Canvas::elide(&tr!("card.meta", app = app, time = time), meta_font, 102.0);
    c.text(r(48.0, 29.0, 102.0, 14.0), Align::LEFT, &meta, meta_font, Color::hex(0x96a0b9));

    let body_font = FontSpec::sans(11.0);
    let text = clean_body(body);
    for (i, line) in body_lines(&text, body_font, 136.0, 4).iter().enumerate() {
        c.text(r(13.0, 50.0 + i as f32 * 16.0, 136.0, 16.0), Align::LEFT, line, body_font, Color::hex(0xe1e4ee));
    }
    c.to_frame()
}

/// The icon of the sender, rendered at 56 px and drawn into 28×28. Search order:
/// 1. `app_icon` (theme name or file);
/// 2. `Icon=` of the desktop file named by `desktop-entry`, then that id as a theme name;
/// 3. `Icon=` of the installed application called `app` (desktop file id, `StartupWMClass`,
///    `Name`, program), then `app` itself as a theme name («Google Chrome» → `google-chrome`);
/// 4. the notification's own picture: `image-path`, `image-data`;
/// 5. `preferences-desktop-notification`.
pub fn card_icon(app: &str, src: IconSource) -> Option<image::RgbaImage> {
    use crate::platform::desktop_entry;
    use crate::platform::icon_theme::{find_icon, fit_square};
    const SIZE: u32 = 56;
    let theme = |name: &str| -> Option<image::RgbaImage> { if name.trim().is_empty() { None } else { find_icon(name, SIZE) } };
    let lower = app.trim().to_lowercase();
    theme(src.app_icon)
        .or_else(|| desktop_entry::icon_for_id(src.desktop_entry).and_then(|i| theme(&i)))
        .or_else(|| theme(src.desktop_entry))
        .or_else(|| desktop_entry::icon_for_app(app).and_then(|i| theme(&i)))
        .or_else(|| theme(&lower.replace(' ', "-")))
        .or_else(|| theme(src.image_path))
        .or_else(|| {
            let img = src.image?;
            image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()).map(|i| fit_square(&i, SIZE))
        })
        .or_else(|| theme("preferences-desktop-notification"))
}

/// Strips HTML tags, decodes `&amp; &lt; &gt;`, collapses whitespace.
pub fn clean_body(body: &str) -> String {
    let mut stripped = String::with_capacity(body.len());
    let mut rest = body;
    // like the regex `<[^>]*>`: a `<` without a closing `>` stays as is
    while let Some(start) = rest.find('<') {
        match rest[start..].find('>') {
            Some(end) => {
                stripped.push_str(&rest[..start]);
                rest = &rest[start + end + 1..];
            }
            None => break,
        }
    }
    stripped.push_str(rest);
    let decoded = stripped.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Word wrap like QTextLayout; when more than `max` lines are needed the last one gets "…"
/// appended and is elided to the width (as the Qt version does).
fn body_lines(text: &str, font: FontSpec, width: f32, max: usize) -> Vec<String> {
    let all = Canvas::wrap(text, font, width, usize::MAX);
    if all.len() <= max {
        return all;
    }
    let mut lines: Vec<String> = all[..max].to_vec();
    // a line that ended at a space keeps that space in Qt, so the ellipsis follows a gap
    let mut pos = 0usize;
    let mut at_space = false;
    for line in &lines {
        if text[pos..].starts_with(line.as_str()) {
            pos += line.len();
        }
        at_space = text[pos..].starts_with(' ');
        if at_space {
            pos += 1;
        }
    }
    let last = &mut lines[max - 1];
    let candidate = if at_space { format!("{last} …") } else { format!("{last}…") };
    *last = Canvas::elide(&candidate, font, width);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_markup() {
        assert_eq!(clean_body("<b>0 ошибок</b> &amp; 3\n\n  предупреждения &lt;x&gt;"), "0 ошибок & 3 предупреждения <x>");
        assert_eq!(clean_body("a < b"), "a < b");
        assert_eq!(clean_body("  <a href='x'>link</a>  "), "link");
    }

    #[test]
    fn wraps_to_four_lines() {
        let f = FontSpec::sans(11.0);
        let text = "0 ошибок & 3 предупреждения, сборка заняла 2 минуты 14 секунд, артефакты загружены в хранилище";
        let lines = body_lines(text, f, 136.0, 4);
        assert_eq!(lines.len(), 4);
        assert!(lines[3].ends_with('…'), "{lines:?}");
        assert_eq!(body_lines("коротко", f, 136.0, 4), vec!["коротко"]);
    }

    #[test]
    fn renders_card() {
        let f = render("Telegram", "Анна", "Созвон переносим на 20:30, ок?", IconSource::default(), "12:00");
        assert_eq!(f.pixel(0, 0), [0x10, 0x12, 0x1a]);
        assert_eq!(f.pixel(80, 120), [0x20, 0x24, 0x32]);
    }

    #[test]
    fn falls_back_to_the_notification_image() {
        let red = NotifyImage { width: 2, height: 2, rgba: [255, 0, 0, 255].repeat(4) };
        let src = IconSource { image: Some(&red), ..Default::default() };
        let img = card_icon("no such application 7f3a", src).unwrap();
        assert_eq!((img.width(), img.height()), (56, 56));
        assert_eq!(img.get_pixel(28, 28).0, [255, 0, 0, 255]);
    }
}
