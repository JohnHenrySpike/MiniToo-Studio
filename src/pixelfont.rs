//! 5×7 small-caps pixel font (§17.1): drawing, measuring, eliding.

use crate::canvas::Canvas;
use crate::color::Color;

#[path = "pixelfont_data.rs"]
mod data;
pub use data::{HEIGHT, TOP};

pub fn glyph(c: char) -> Option<&'static [&'static str]> {
    data::glyph(c)
}

/// All characters (and at least one) are in the font.
pub fn covers(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| glyph(c).is_some())
}

fn advance(c: char, spacing: i32) -> i32 {
    glyph(c).map(|g| g[0].chars().count() as i32).unwrap_or(0) + 1 + spacing
}

/// Width in font pixels (zoom 1).
pub fn width(text: &str, spacing: i32) -> i32 {
    if text.is_empty() {
        return 0;
    }
    text.chars().map(|c| advance(c, spacing)).sum::<i32>() - 1 - spacing
}

pub fn elided(text: &str, spacing: i32, max_width: i32) -> String {
    if width(text, spacing) <= max_width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s = chars.iter().collect::<String>().trim().to_string() + "…";
        if width(&s, spacing) <= max_width {
            return s;
        }
    }
    "…".to_string()
}

/// Draws `text` with the top of its 10-px line at (x, y). Returns the width drawn (zoomed).
pub fn draw(c: &mut Canvas, x: i32, y: i32, text: &str, color: Color, spacing: i32, zoom: i32) -> i32 {
    let z = zoom.max(1);
    let mut pen = 0;
    for ch in text.chars() {
        if let Some(rows) = glyph(ch) {
            for (ry, row) in rows.iter().enumerate() {
                for (rx, b) in row.chars().enumerate() {
                    if b != '#' {
                        continue;
                    }
                    for dy in 0..z {
                        for dx in 0..z {
                            c.blend_pixel(x + (pen + rx as i32) * z + dx, y + (TOP + ry as i32) * z + dy, color);
                        }
                    }
                }
            }
        }
        pen += advance(ch, spacing);
    }
    (pen - 1 - spacing).max(0) * z
}

/// Renders into a new transparent canvas, `width × HEIGHT` (zoom 1).
pub fn render(text: &str, color: Color, spacing: i32) -> Canvas {
    let mut c = Canvas::new(width(text, spacing).max(1) as u32, HEIGHT as u32);
    draw(&mut c, 0, 0, text, color, spacing, 1);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics() {
        assert!(covers("ПРОВЕРКА"));
        assert!(covers("РАЗРЕШИТЬ BASH?"));
        assert!(!covers("abc"));
        assert_eq!(width("I", 0), 3);
        assert_eq!(width("AB", 0), 11);
        assert_eq!(glyph('Д').unwrap().len(), 8);
        assert_eq!(glyph('А'), glyph('A'));
        assert!(elided("ОЧЕНЬ ДЛИННАЯ ПОДПИСЬ ПРО ПРОЕКТ", 0, 60).ends_with('…'));
    }
}
