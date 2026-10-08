//! The six pages (§16.7–16.12).

mod claude;
mod device;
mod image;
mod modes;
mod screen;
mod settings;

use super::Cx;
use egui::{Align, Layout, Rect, Ui, UiBuilder};
use std::path::PathBuf;

#[derive(Default)]
pub struct Pages {
    image: image::State,
    screen: screen::State,
    modes: modes::State,
    claude: claude::State,
    device: device::State,
    settings: settings::State,
}

impl Pages {
    pub fn show(&mut self, ui: &mut Ui, cx: &mut Cx, page: usize) {
        match page {
            0 => image::show(ui, cx, &mut self.image),
            1 => screen::show(ui, cx, &mut self.screen),
            2 => modes::show(ui, cx, &mut self.modes),
            3 => claude::show(ui, cx, &mut self.claude),
            4 => device::show(ui, cx, &mut self.device),
            _ => settings::show(ui, cx, &mut self.settings),
        }
    }
}

/// A child Ui in `rect`, top-down.
pub fn area<R>(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)));
    add(&mut child)
}

/// A vertically scrolling page with 20 px margins; `max_width` limits the content.
pub fn scroll_page(ui: &mut Ui, id: &str, max_width: Option<f32>, add: impl FnOnce(&mut Ui)) {
    let full = ui.max_rect();
    let mut child = ui.new_child(UiBuilder::new().max_rect(full).layout(Layout::top_down(Align::Min)));
    egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(&mut child, |ui| {
        let w = ui.available_width();
        let inner_w = max_width.map(|m| m.min(w)).unwrap_or(w) - 40.0;
        ui.add_space(20.0);
        ui.horizontal(|ui| {
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.set_width(inner_w);
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 16.0);
                add(ui);
            });
        });
        ui.add_space(20.0);
    });
}

/// Opens a file dialog on another thread; `then` gets the chosen paths.
pub fn pick_files(title: &'static str, filters: &'static [(&'static str, &'static [&'static str])], multiple: bool, then: impl FnOnce(Vec<PathBuf>) + Send + 'static) {
    std::thread::spawn(move || {
        let mut d = rfd::FileDialog::new().set_title(title);
        for (name, ext) in filters {
            d = d.add_filter(*name, ext);
        }
        let files = if multiple { d.pick_files().unwrap_or_default() } else { d.pick_file().into_iter().collect() };
        if !files.is_empty() {
            then(files);
        }
    });
}

pub fn pick_folder(title: &'static str, then: impl FnOnce(PathBuf) + Send + 'static) {
    std::thread::spawn(move || {
        if let Some(dir) = rfd::FileDialog::new().set_title(title).pick_folder() {
            then(dir);
        }
    });
}

pub const IMAGE_FILTERS: &[(&str, &[&str])] = &[
    ("Изображения", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "jxl", "heic", "tif", "tiff"]),
    ("Все файлы", &["*"]),
];
