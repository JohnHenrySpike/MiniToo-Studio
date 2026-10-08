//! «Изображение» (§16.7): editor canvas with the CropBox, the gallery and the result.

use super::{IMAGE_FILTERS, area, pick_files, pick_folder};
use crate::api::{Command, Fit, GalleryFilter, SourceImage};
use crate::frame::Frame;
use crate::ui::Cx;
use crate::ui::crop_box::{CropEvent, crop_box};
use crate::ui::pixel::{self, PixStyle};
use crate::ui::textures;
use crate::ui::theme::{WHITE, hex, pal};
use crate::ui::widgets::{self as w, Key};
use egui::{Align, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::time::Duration;

#[derive(Default)]
pub struct State {
    expanded: bool,
    strip_x: f32,
}

const GAP: f32 = 16.0;

fn header_subtitle(src: Option<&SourceImage>, frames: usize) -> String {
    match src {
        Some(s) => {
            let mut t = format!("{}  ·  {}×{}", s.name, s.width, s.height);
            if frames > 1 {
                t.push_str(&format!("  ·  {frames} кадров"));
            }
            t
        }
        None => "картинка или GIF на весь экран колонки".into(),
    }
}

fn open_dialog(cx: &Cx) {
    let core = cx.core.clone();
    pick_files("Выберите изображение", IMAGE_FILTERS, true, move |files| core.send(Command::OpenFiles(files)));
}

/// Frame index of a source animation at the current time (per-frame delays).
fn source_index(ui: &Ui, delays: &[u32], count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    let d = |i: usize| delays.get(i).copied().unwrap_or(100).max(20) as u64;
    let total: u64 = (0..count).map(d).sum();
    let ms = (ui.input(|i| i.time) * 1000.0) as u64 % total.max(1);
    let mut acc = 0;
    for i in 0..count {
        acc += d(i);
        if ms < acc {
            ui.ctx().request_repaint_after(Duration::from_millis(acc - ms));
            return i;
        }
    }
    0
}

fn fit_rect(outer: Rect, w: f32, h: f32) -> Rect {
    if w <= 0.0 || h <= 0.0 {
        return outer;
    }
    let s = (outer.width() / w).min(outer.height() / h);
    Rect::from_center_size(outer.center(), vec2(w * s, h * s))
}

fn editor(ui: &mut Ui, cx: &mut Cx, rect: Rect) {
    let p = pal();
    let img = &cx.snap.image;
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 12, p.canvas);
    let clip = painter.with_clip_rect(rect);
    if let Some(src) = &img.source
        && !src.frames.is_empty()
    {
        let i = source_index(ui, &src.delays, src.frames.len());
        let frame = &src.frames[i.min(src.frames.len() - 1)];
        // small animations keep a texture per frame; long ones re-upload the current frame
        let key = if src.frames.len() <= 24 { format!("src/{i}") } else { "src".to_string() };
        let tex = cx.tex.image(ui.ctx(), &key, frame, !img.pixel_art);
        let shown = fit_rect(rect.shrink(22.0), src.width.max(1) as f32, src.height.max(1) as f32);
        w::paint_texture(&clip, shown, tex, WHITE);
        if img.fit == Fit::Crop {
            match crop_box(ui, ui.id().with("crop"), shown, img.crop) {
                Some(CropEvent::Edited(r)) => cx.send(Command::SetCrop(r)),
                Some(CropEvent::Reset) => cx.send(Command::ResetCrop),
                None => {}
            }
        }
    } else {
        let big = PixStyle::new(12).zoom(2);
        let line = "Перетащите сюда картинку или GIF";
        let ts = big.measure(line);
        let key = Key::new("Выбрать файл…").icon("open").accent(true);
        let ks = key.size(ui);
        let h = 60.0 + 14.0 + ts.y + 14.0 + ks.y;
        let top = rect.center().y - h / 2.0;
        pixel::paint_icon(&painter, pos2(rect.center().x - 30.0, top), "image", 5, hex(0x8f877c), Some(p.accent));
        pixel::paint_text(&painter, pos2(rect.center().x - ts.x / 2.0, top + 74.0), line, &big, hex(0xd8cfbf));
        let kr = Rect::from_min_size(pos2(rect.center().x - ks.x / 2.0, top + 74.0 + ts.y + 14.0), ks);
        if key.show_at(ui, kr).clicked() {
            open_dialog(cx);
        }
    }
    if cx.drag_hover {
        painter.rect_stroke(rect, 12, Stroke::new(3.0, p.accent), StrokeKind::Inside);
    }
}

enum Thumb<'a> {
    Item(&'a crate::api::GalleryItemView),
    File(&'a crate::api::FolderItemView),
}

impl Thumb<'_> {
    fn key(&self) -> String {
        match self {
            Thumb::Item(i) => format!("g/{}", i.id),
            Thumb::File(f) => format!("f/{}", f.path.display()),
        }
    }
    fn thumb(&self) -> Option<&Frame> {
        match self {
            Thumb::Item(i) => i.thumb.as_ref(),
            Thumb::File(f) => f.thumb.as_ref(),
        }
    }
    fn name(&self) -> &str {
        match self {
            Thumb::Item(i) => &i.name,
            Thumb::File(f) => &f.name,
        }
    }
    fn open(&self) -> Command {
        match self {
            Thumb::Item(i) => Command::GalleryOpen(i.id.clone()),
            Thumb::File(f) => Command::FolderOpen(f.path.clone()),
        }
    }
    fn send(&self) -> Command {
        match self {
            Thumb::Item(i) => Command::GallerySend(i.id.clone()),
            Thumb::File(f) => Command::FolderSend(f.path.clone()),
        }
    }
    fn tooltip(&self) -> String {
        let mut t = self.name().to_string();
        if let Thumb::Item(i) = self {
            if i.width > 0 {
                t.push_str(&format!("  ·  {}×{}", i.width, i.height));
            }
            if i.frames > 1 {
                t.push_str(&format!("  ·  {} кадров", i.frames));
            }
            if i.sent > 0 {
                t.push_str(&format!("\nотправлено: {}", i.sent));
            }
        }
        t.push_str("\nщелчок — открыть, двойной — отправить");
        t
    }
}

fn folder_name(p: &std::path::Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

fn choose_folder(cx: &Cx) {
    let core = cx.core.clone();
    pick_folder("Папка с картинками", move |dir| {
        core.send(Command::SetFolder(Some(dir)));
        core.send(Command::GalleryFilter(GalleryFilter::Folder));
    });
}

/// Height of the gallery panel in strip mode.
fn strip_height() -> f32 {
    let (_, s) = w::screen_size(0.5);
    10.0 + 30.0 + 8.0 + (s.y + 12.0) + 14.0
}

fn gallery(ui: &mut Ui, cx: &mut Cx, st: &mut State, rect: Rect) {
    let p = pal();
    let img = &cx.snap.image;
    w::paint_panel(ui.painter(), rect);
    if cx.drag_hover && st.expanded {
        ui.painter().rect_stroke(Rect::from_min_max(rect.min, rect.max - vec2(0.0, 3.0)), 12, Stroke::new(3.0, p.accent), StrokeKind::Inside);
    }
    let inner = Rect::from_min_max(rect.min + vec2(12.0, 10.0), rect.max - vec2(12.0, 14.0));
    let folder_view = img.filter == GalleryFilter::Folder;
    let favorites = img.gallery.iter().filter(|g| g.favorite).count();
    area(ui, inner, |ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 8.0);
        w::row(ui, 30.0, 2.0, |ui| {
            w::pixel_text(ui, "Галерея", PixStyle::new(12), p.text);
            ui.add_space(10.0);
            let all = format!("Все  {}", img.gallery.len());
            if Key::new(&all).flat().checked(img.filter == GalleryFilter::All).tip("Всё, что вы открывали и отправляли").show(ui).clicked() {
                cx.send(Command::GalleryFilter(GalleryFilter::All));
            }
            let fav = if favorites > 0 { format!("Избранное  {favorites}") } else { "Избранное".into() };
            if Key::new(&fav).flat().checked(img.filter == GalleryFilter::Favorites).show(ui).clicked() {
                cx.send(Command::GalleryFilter(GalleryFilter::Favorites));
            }
            let folder_label = img.folder.as_deref().map(folder_name).unwrap_or_else(|| "Папка…".into());
            let tip_text = img.folder.as_ref().map(|f| f.display().to_string()).unwrap_or_else(|| "Показать картинки из своей папки".into());
            let fk = Key::new(&folder_label).flat().icon("open").checked(folder_view).tip(&tip_text);
            let fk = if fk.size(ui).x > 180.0 { fk.width(180.0) } else { fk };
            if fk.show(ui).clicked() {
                if img.folder.is_none() {
                    choose_folder(cx);
                } else {
                    cx.send(Command::GalleryFilter(GalleryFilter::Folder));
                }
            }
            w::right(ui, |ui| {
                let (icon, tip_t) = if st.expanded { ("down", "Свернуть в полосу") } else { ("up", "Развернуть сеткой") };
                if Key::icon_only(icon).flat().tip(tip_t).show(ui).clicked() {
                    st.expanded = !st.expanded;
                }
                if folder_view && img.folder.is_some() {
                    if Key::icon_only("close").flat().tip("Убрать папку").show(ui).clicked() {
                        cx.send(Command::SetFolder(None));
                        cx.send(Command::GalleryFilter(GalleryFilter::All));
                    }
                    if Key::icon_only("open").flat().tip("Выбрать другую папку…").show(ui).clicked() {
                        choose_folder(cx);
                    }
                    if Key::icon_only("refresh").flat().tip("Перечитать папку").show(ui).clicked() {
                        cx.send(Command::RescanFolder);
                    }
                }
            });
        });

        let thumbs: Vec<Thumb> = match img.filter {
            GalleryFilter::All => img.gallery.iter().map(Thumb::Item).collect(),
            GalleryFilter::Favorites => img.gallery.iter().filter(|g| g.favorite).map(Thumb::Item).collect(),
            GalleryFilter::Folder => img.folder_items.iter().map(Thumb::File).collect(),
        };
        let list_rect = ui.available_rect_before_wrap();
        if thumbs.is_empty() {
            let text = match img.filter {
                GalleryFilter::Favorites => "Отметьте картинки звёздочкой — они соберутся здесь.",
                GalleryFilter::Folder if img.folder.is_none() => "Папка не выбрана.",
                GalleryFilter::Folder => "В папке нет картинок.",
                GalleryFilter::All => {
                    "Здесь появятся картинки, которые вы открываете и отправляете. Можно перетащить сюда файлы или целую папку."
                }
            };
            let width = (list_rect.width() - 24.0).min(460.0);
            let g = w::galley_wrapped(ui, text, w::font(13.0), p.text_dim, width);
            let key = Key::new("Выбрать папку…").icon("open");
            let ks = key.size(ui);
            let h = g.size().y + if folder_view { 8.0 + ks.y } else { 0.0 };
            let top = list_rect.center().y - h / 2.0;
            let g = ui.fonts_mut(|f| f.layout_job(w::job(text, w::font(13.0), p.text_dim, width, Align::Center)));
            ui.painter().galley(pos2(list_rect.center().x, top), g, p.text_dim);
            if folder_view {
                let kr = Rect::from_min_size(pos2(list_rect.center().x - ks.x / 2.0, top + h - ks.y), ks);
                if key.show_at(ui, kr).clicked() {
                    choose_folder(cx);
                }
            }
            ui.allocate_rect(list_rect, Sense::hover());
            return;
        }
        let current_id = img.source.as_ref().map(|s| s.id.clone());
        let current_path = img.source.as_ref().map(|s| s.path.clone());
        let is_current = |t: &Thumb| match t {
            Thumb::Item(i) => current_id.as_deref() == Some(i.id.as_str()),
            Thumb::File(f) => current_path.as_ref() == Some(&f.path),
        };
        if st.expanded {
            let factor = 0.75;
            let (_, ts) = w::screen_size(factor);
            let cols = ((list_rect.width() / (ts.x + 16.0)).floor() as usize).max(1);
            let cell_w = (list_rect.width() / cols as f32).floor();
            let cell_h = ts.y + 30.0;
            let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
            egui::ScrollArea::vertical().id_salt("gallery-grid").auto_shrink([false, false]).show(&mut lu, |ui| {
                let rows = thumbs.len().div_ceil(cols);
                let (all, _) = ui.allocate_exact_size(vec2(list_rect.width(), rows as f32 * cell_h), Sense::hover());
                for (n, t) in thumbs.iter().enumerate() {
                    let cell = Rect::from_min_size(all.min + vec2((n % cols) as f32 * cell_w, (n / cols) as f32 * cell_h), vec2(cell_w, cell_h));
                    thumb_cell(ui, cx, st, t, cell, factor, true, is_current(t));
                }
            });
        } else {
            let factor = 0.5;
            let (_, ts) = w::screen_size(factor);
            let cell_w = ts.x + 10.0;
            let cell_h = ts.y + 12.0;
            let strip = Rect::from_min_size(list_rect.min, vec2(list_rect.width(), cell_h));
            let content_w = thumbs.len() as f32 * cell_w;
            let max_x = (content_w - strip.width()).max(0.0);
            let hovered = ui.rect_contains_pointer(strip);
            if hovered {
                let wd = w::wheel_delta(ui);
                let d = if wd.y != 0.0 { wd.y } else { wd.x };
                st.strip_x -= d;
            }
            st.strip_x = st.strip_x.clamp(0.0, max_x);
            let mut lu = ui.new_child(egui::UiBuilder::new().max_rect(strip));
            lu.set_clip_rect(strip.intersect(ui.clip_rect()));
            let first = (st.strip_x / cell_w).floor() as usize;
            for (n, t) in thumbs.iter().enumerate().skip(first) {
                let x = strip.left() + n as f32 * cell_w - st.strip_x;
                if x > strip.right() {
                    break;
                }
                let cell = Rect::from_min_size(pos2(x, strip.top()), vec2(cell_w, cell_h));
                thumb_cell(&mut lu, cx, st, t, cell, factor, false, is_current(t));
            }
            if max_x > 0.0 && hovered {
                let bar_w = strip.width() * strip.width() / content_w;
                let bx = strip.left() + (strip.width() - bar_w) * st.strip_x / max_x;
                ui.painter().rect_filled(Rect::from_min_size(pos2(bx, strip.bottom() - 4.0), vec2(bar_w, 3.0)), 2, p.text_dim.gamma_multiply(0.5));
            }
            ui.allocate_rect(strip, Sense::hover());
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn thumb_cell(ui: &mut Ui, cx: &mut Cx, st: &mut State, t: &Thumb, cell: Rect, factor: f32, grid: bool, current: bool) {
    let p = pal();
    let (bezel, size) = w::screen_size(factor);
    let frame_min = pos2((cell.center().x - size.x / 2.0).round(), cell.top() + 4.0);
    let frame_rect = Rect::from_min_size(frame_min, size);
    let hit = Rect::from_min_size(frame_min, vec2(size.x, size.y + if grid { 24.0 } else { 0.0 }));
    let id = ui.id().with(("thumb", t.key()));
    let resp = ui.interact(hit, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hovered = ui.rect_contains_pointer(hit);
    let painter = ui.painter().clone();
    let glass = w::paint_screen_frame(&painter, frame_min, factor, current, false);
    if let Some(f) = t.thumb() {
        let tex = cx.tex.frame(ui.ctx(), &t.key(), f, true);
        w::paint_texture(&painter, glass, tex, WHITE);
    }
    let shade = ui.ctx().animate_bool_with_time(id.with("shade"), hovered, 0.12);
    if shade > 0.0 {
        painter.rect_filled(glass, 0, Color32::from_black_alpha((0.28 * 255.0 * shade) as u8));
    }
    if let Thumb::Item(i) = t
        && i.frames > 1
    {
        let ps = w::plate_size("gif");
        w::paint_plate(&painter, pos2(glass.left() + 3.0, glass.bottom() - 3.0 - ps.y), "gif", p.plate, p.plate_text);
    }
    if grid {
        let color = if current { p.accent_text } else { p.text_dim };
        let name = w::elide_middle(ui, t.name(), &w::font(11.0), frame_rect.width());
        let g = w::galley(ui, &name, w::font(11.0), color);
        painter.galley(pos2(frame_rect.center().x - g.size().x / 2.0, frame_rect.bottom() + 5.0), g, color);
    }
    let mut clicked_key = false;
    if let Thumb::Item(i) = t {
        if i.favorite || hovered {
            let r = Rect::from_min_size(frame_min + vec2(bezel + 3.0, bezel + 3.0), vec2(22.0, 22.0));
            let tip = if i.favorite { "Убрать из избранного" } else { "В избранное" };
            if w::mini_key(ui, r, "star", i.favorite, tip).clicked() {
                cx.send(Command::GalleryFavorite(i.id.clone(), !i.favorite));
                clicked_key = true;
            }
        }
        if hovered {
            let r = Rect::from_min_size(pos2(frame_rect.right() - bezel - 25.0, frame_rect.top() + bezel + 3.0), vec2(22.0, 22.0));
            if w::mini_key(ui, r, "trash", false, "Убрать из галереи").clicked() {
                cx.send(Command::GalleryRemove(i.id.clone()));
                clicked_key = true;
            }
        }
    }
    if hovered {
        let y = frame_rect.bottom() - bezel - 25.0;
        let mut x = frame_rect.right() - bezel - 25.0;
        let r = Rect::from_min_size(pos2(x, y), vec2(22.0, 22.0));
        if w::mini_key(ui, r, "send", true, "Отправить на колонку").clicked() {
            cx.send(t.send());
            clicked_key = true;
        }
        if grid {
            x -= 26.0;
            let r = Rect::from_min_size(pos2(x, y), vec2(22.0, 22.0));
            if w::mini_key(ui, r, "crop", false, "Открыть в редакторе").clicked() {
                cx.send(t.open());
                st.expanded = false;
                clicked_key = true;
            }
        }
    }
    if !clicked_key {
        if resp.double_clicked() {
            cx.send(t.send());
        } else if resp.clicked() {
            cx.send(t.open());
        }
    }
    if !grid {
        let tt = t.tooltip();
        let _ = resp.on_hover_ui(|ui| {
            ui.set_max_width(340.0);
            ui.label(w::rich(tt, 12.0, pal().plate_text));
        });
    }
}

fn bottom_height(ui: &Ui, cx: &Cx, width: f32) -> f32 {
    let img = &cx.snap.image;
    let factor = if width - 32.0 < 640.0 { 1.0 } else { 1.25 };
    let (_, fs) = w::screen_size(factor);
    let col_w = width - 32.0 - fs.x - 20.0;
    let hint = w::galley_wrapped(ui, fit_hint(img.fit), w::font(12.0), Color32::WHITE, col_w).size().y;
    let mut col = 14.0 + 10.0 + 33.0 + 10.0 + 28.0 + 10.0 + hint + 10.0 + 46.0;
    if img.source_frames > 92 {
        col += 10.0 + w::galley_wrapped(ui, WARN_92, w::font(13.0), Color32::WHITE, col_w).size().y;
    }
    if let Some(e) = &img.error {
        col += 10.0 + w::galley_wrapped(ui, e, w::font(13.0), Color32::WHITE, col_w).size().y;
    }
    16.0 + col.max(fs.y) + 19.0
}

const WARN_92: &str = "Колонка показывает до 92 кадров — лишние будут равномерно прорежены, длительность сохранится.";

fn fit_hint(fit: Fit) -> &'static str {
    match fit {
        Fit::Crop => "Тяните рамку и её углы, колесо мыши — масштаб, двойной щелчок — сброс. Пропорции экрана 5:4.",
        Fit::Fit => "Картинка целиком, поля заполняются чёрным.",
        Fit::Stretch => "Картинка растягивается на весь экран без сохранения пропорций.",
    }
}

fn bottom(ui: &mut Ui, cx: &mut Cx, rect: Rect) {
    let p = pal();
    let img = &cx.snap.image;
    let has = img.source.is_some();
    w::paint_panel(ui.painter(), rect);
    let inner = Rect::from_min_max(rect.min + vec2(16.0, 16.0), rect.max - vec2(16.0, 19.0));
    let factor = if inner.width() < 640.0 { 1.0 } else { 1.25 };
    let (_, fs) = w::screen_size(factor);
    let glass = w::paint_screen_frame(ui.painter(), pos2(inner.left(), inner.center().y - fs.y / 2.0), factor, false, false);
    if has && let Some((i, f)) = textures::anim_frame(ui.ctx(), &img.preview) {
        let tex = cx.tex.frame(ui.ctx(), &format!("preview/{i}"), f, false);
        w::paint_texture(ui.painter(), glass, tex, WHITE);
    }
    let col = Rect::from_min_max(pos2(inner.left() + fs.x + 20.0, inner.top()), inner.max);
    area(ui, col, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
        w::pixel_text(ui, "Так будет на колонке", PixStyle::new(12), p.text);
        let cur = match img.fit {
            Fit::Crop => 0,
            Fit::Fit => 1,
            Fit::Stretch => 2,
        };
        if let Some(i) = w::tabs(ui, &[("Кадрировать", None), ("Вписать", None), ("Растянуть", None)], cur) {
            cx.send(Command::SetFit(Fit::from_i64(i as i64)));
        }
        w::row(ui, 28.0, 14.0, |ui| {
            let r = w::checkbox(ui, img.pixel_art, "Пиксель-арт", true);
            let r = w::tip(r, "Масштабировать без сглаживания — для пиксельной графики");
            if r.clicked() {
                cx.send(Command::SetPixelArt(!img.pixel_art));
            }
            if img.fit == Fit::Crop && Key::new("Сбросить рамку").flat().icon("refresh").enabled(has).show(ui).clicked() {
                cx.send(Command::ResetCrop);
            }
        });
        w::para(ui, fit_hint(img.fit), w::font(12.0), p.text_dim);
        if img.source_frames > 92 {
            w::para(ui, WARN_92, w::font(13.0), p.warn);
        }
        if let Some(e) = &img.error {
            w::para(ui, e, w::font(13.0), p.danger);
        }
    });
    let key = Key::new("Отправить на колонку").icon("send").accent(true).height(46.0).pad(22.0).enabled(has);
    let ks = key.size(ui);
    let kr = Rect::from_min_size(inner.max - ks, ks);
    if key.show_at(ui, kr).clicked() {
        cx.send(Command::SendImage);
    }
    ui.allocate_rect(rect, Sense::hover());
}

pub fn show(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let full = ui.max_rect().shrink(20.0);
    let img = &cx.snap.image;
    let frames = img.source.as_ref().map(|s| s.frames.len().max(img.source_frames)).unwrap_or(0);
    let sub = header_subtitle(img.source.as_ref(), if img.source_frames > 0 { img.source_frames } else { frames });
    let header_h = area(ui, full, |ui| {
        w::page_header(ui, "Изображение", &sub, |ui| {
            if Key::new("Открыть…").icon("open").show(ui).clicked() {
                open_dialog(cx);
            }
        });
        ui.min_rect().height()
    });
    let body = Rect::from_min_max(pos2(full.left(), full.top() + header_h + GAP), full.max);
    let bh = bottom_height(ui, cx, body.width());
    let bottom_rect = Rect::from_min_max(pos2(body.left(), body.bottom() - bh), body.max);
    if st.expanded {
        let gal = Rect::from_min_max(body.min, pos2(body.right(), bottom_rect.top() - GAP));
        gallery(ui, cx, st, gal);
    } else {
        let gh = strip_height();
        let gal = Rect::from_min_max(pos2(body.left(), bottom_rect.top() - GAP - gh), pos2(body.right(), bottom_rect.top() - GAP));
        let ed = Rect::from_min_max(body.min, pos2(body.right(), (gal.top() - GAP).max(body.top() + 150.0)));
        editor(ui, cx, ed);
        gallery(ui, cx, st, gal);
    }
    bottom(ui, cx, bottom_rect);
}
