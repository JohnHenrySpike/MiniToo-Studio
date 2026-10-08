//! Статистика Claude (`claudestats`). Placeholder until the real implementation lands.

use super::{LiveMode, ModeCx};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;

pub struct ClaudeStats;

impl ClaudeStats {
    pub fn new() -> Self {
        ClaudeStats
    }
}

impl LiveMode for ClaudeStats {
    fn id(&self) -> &'static str {
        "claudestats"
    }
    fn title(&self) -> &'static str {
        "Статистика Claude"
    }
    fn subtitle(&self) -> &'static str {
        "сессии и токены за сегодня"
    }
    fn icon(&self) -> &'static str {
        "sparkle"
    }
    fn render(&mut self, cx: &mut ModeCx) {
        let mut c = Canvas::device();
        c.text(r(0.0, 0.0, 160.0, 128.0), Align::CENTER, "Статистика Claude", FontSpec::bold(12.0), Color::WHITE);
        cx.publish(c.to_frame());
    }
}
