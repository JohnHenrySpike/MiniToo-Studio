//! GitHub Actions (`github`). Placeholder until the real implementation lands.

use super::{LiveMode, ModeCx};
use crate::canvas::{r, Align, Canvas};
use crate::color::Color;
use crate::fonts::FontSpec;

pub struct Github;

impl Github {
    pub fn new() -> Self {
        Github
    }
}

impl LiveMode for Github {
    fn id(&self) -> &'static str {
        "github"
    }
    fn title(&self) -> &'static str {
        "GitHub Actions"
    }
    fn subtitle(&self) -> &'static str {
        "последние запуски CI по репозиториям"
    }
    fn icon(&self) -> &'static str {
        "branch"
    }
    fn render(&mut self, cx: &mut ModeCx) {
        let mut c = Canvas::device();
        c.text(r(0.0, 0.0, 160.0, 128.0), Align::CENTER, "GitHub Actions", FontSpec::bold(12.0), Color::WHITE);
        cx.publish(c.to_frame());
    }
}
