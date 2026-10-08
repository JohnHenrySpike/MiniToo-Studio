//! «Экран» (§16.8): live capture with the region CropBox and the streaming controls.

use super::area;
use crate::api::{CaptureStatus, Command, NRect};
use crate::ui::Cx;
use crate::ui::crop_box::{CropEvent, crop_box};
use crate::ui::pixel::{self, PixStyle};
use crate::ui::theme::{WHITE, hex, pal};
use crate::ui::widgets::{self as w, Key, Spin};
use egui::{Align, Rect, Sense, Ui, pos2, vec2};

#[derive(Default)]
pub struct State {}

const GAP: f32 = 16.0;

fn stale_panel(ui: &mut Ui, cx: &mut Cx) {
    let p = pal();
    let units = cx.snap.screen.stale_portals.join(", ");
    w::panel(ui, |ui| {
        let key = Key::new("Перезапустить").icon("refresh").accent(true);
        let kw = key.size(ui).x;
        let text_w = ui.available_width() - 36.0 - kw - 28.0;
        let msg = format!(
            "После обновления системы служба портала работает со старыми библиотеками, и окно выбора экрана не открывается. Её нужно перезапустить ({units})."
        );
        let th = 15.0 + 2.0 + w::galley_wrapped(ui, &msg, w::font(13.0), p.text_dim, text_w).size().y;
        w::row(ui, th.max(36.0), 14.0, |ui| {
            w::pixel_icon(ui, "warning", 3, p.warn, None);
            w::col(ui, text_w, th, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                w::pixel_text(ui, "Портал захвата экрана устарел", PixStyle::new(12), p.text);
                w::hint(ui, &msg);
            });
            if key.show(ui).clicked() {
                cx.send(Command::RestartPortals);
            }
        });
    });
}

fn canvas(ui: &mut Ui, cx: &mut Cx, rect: Rect) {
    let p = pal();
    let s = &cx.snap.screen;
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 12, p.canvas);
    let capturing = s.status == CaptureStatus::Capturing;
    if capturing && let Some(img) = &s.live {
        let (sw, sh) = s.source_size.map(|(w, h)| (w as f32, h as f32)).unwrap_or((img.width() as f32, img.height() as f32));
        let inner = rect.shrink(22.0);
        let k = (inner.width() / sw).min(inner.height() / sh);
        let shown = Rect::from_center_size(inner.center(), vec2(sw * k, sh * k));
        let tex = cx.tex.image(ui.ctx(), "capture", img, true);
        w::paint_texture(&painter.with_clip_rect(rect), shown, tex, WHITE);
        match crop_box(ui, ui.id().with("region"), shown, s.region) {
            Some(CropEvent::Edited(r)) => cx.send(Command::SetRegion(r)),
            Some(CropEvent::Reset) => cx.send(Command::SetRegion(NRect::center_5x4(sw as f64, sh as f64))),
            None => {}
        }
        return;
    }
    let selecting = s.status == CaptureStatus::Selecting;
    let width = (rect.width() - 60.0).min(540.0);
    let big = PixStyle::new(12).zoom(2);
    let title = if selecting { "Выберите экран или окно…" } else { "Захват не запущен" };
    let start = if s.has_token { "Начать захват" } else { "Выбрать экран…" };
    let text = if selecting {
        "KDE показывает диалог выбора — он может открыться за этим окном.".to_string()
    } else {
        format!("Нажмите «{start}». KDE спросит, какой экран или окно показать; выбор запомнится. Затем выделите область рамкой и нажмите «Транслировать».")
    };
    let g = ui.fonts_mut(|f| f.layout_job(w::job(&text, w::font(13.0), hex(0xb9b0a2), width, Align::Center)));
    let err = s.error.as_ref().map(|e| {
        ui.fonts_mut(|f| f.layout_job(w::job(&format!("Ошибка захвата: {e}"), w::font(13.0), hex(0xff8a70), width, Align::Center)))
    });
    let ts = big.measure(title);
    let h = 60.0 + 14.0 + ts.y + 14.0 + g.size().y + err.as_ref().map(|e| 14.0 + e.size().y).unwrap_or(0.0);
    let mut y = rect.center().y - h / 2.0;
    pixel::paint_icon(&painter, pos2(rect.center().x - 30.0, y), "screen", 5, hex(0x8f877c), Some(p.accent));
    y += 74.0;
    pixel::paint_text(&painter, pos2(rect.center().x - ts.x / 2.0, y), title, &big, hex(0xd8cfbf));
    y += ts.y + 14.0;
    let gh = g.size().y;
    painter.galley(pos2(rect.center().x, y), g, hex(0xb9b0a2));
    y += gh + 14.0;
    if let Some(e) = err {
        painter.galley(pos2(rect.center().x, y), e, hex(0xff8a70));
    }
}

fn bottom(ui: &mut Ui, cx: &mut Cx, rect: Rect) {
    let p = pal();
    let s = &cx.snap.screen;
    w::paint_panel(ui.painter(), rect);
    let inner = Rect::from_min_max(rect.min + vec2(14.0, 14.0), rect.max - vec2(14.0, 17.0));
    let capturing = s.status == CaptureStatus::Capturing;
    let key = Key::new(if s.streaming { "Остановить" } else { "Транслировать" })
        .icon(if s.streaming { "stop" } else { "play" })
        .accent(!s.streaming)
        .height(48.0)
        .pad(22.0)
        .enabled(capturing);
    let ks = key.size(ui);
    let kr = Rect::from_min_size(pos2(inner.right() - ks.x, inner.center().y - ks.y / 2.0), ks);
    if key.show_at(ui, kr).clicked() {
        cx.send(if s.streaming { Command::StopStream } else { Command::StartStream });
    }
    let left = Rect::from_min_max(inner.min, pos2(kr.left() - 18.0, inner.bottom()));
    area(ui, left, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 18.0;
            let col = ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                w::row(ui, 33.0, 10.0, |ui| {
                    w::text(ui, "Частота", w::font(13.0), p.text);
                    let fmt = |v: i64| format!("{v} к/с");
                    if let Some(v) = Spin::new("fps", s.fps as i64, 1, 20).width(130.0).fmt(&fmt).show(ui) {
                        cx.send(Command::SetFps(v as u32));
                    }
                    let r = w::checkbox(ui, s.crisp, "Чёткие пиксели", true);
                    let r = w::tip(r, "Без сглаживания при уменьшении — для пиксель-арта и мелкого текста");
                    if r.clicked() {
                        cx.send(Command::SetCrisp(!s.crisp));
                    }
                });
                w::row(ui, 33.0, 10.0, |ui| {
                    w::text(ui, "Качество", w::font(13.0), p.text);
                    let before = ui.cursor().left();
                    if let Some(i) = w::tabs(ui, &[("Максимум", None), ("Баланс", None), ("Скорость", None)], s.quality.clamp(0, 2) as usize) {
                        cx.send(Command::SetQuality(i as i64));
                    }
                    let tabs_rect = Rect::from_min_max(pos2(before, ui.min_rect().top()), pos2(ui.cursor().left(), ui.min_rect().bottom()));
                    let r = ui.interact(tabs_rect, ui.id().with("quality-tip"), Sense::hover());
                    w::tip(
                        r,
                        "«Баланс» и «Скорость» уменьшают цвет до RGB565/RGB444 — кадр меньше и приходит быстрее. Неизменный экран повторно не передаётся.",
                    );
                });
            });
            let rest = ui.available_width();
            let _ = col;
            ui.vertical(|ui| {
                ui.set_width(rest);
                ui.spacing_mut().item_spacing.y = 4.0;
                if s.streaming {
                    let mut t = format!("Фактически {:.1} к/с", s.actual_fps);
                    if s.frame_kb > 0.0 {
                        t.push_str(&format!(",  кадр {:.1} КБ", s.frame_kb));
                    }
                    w::para(ui, &t, w::font(13.0), p.text);
                }
                if s.paused {
                    w::para(ui, "Пауза: на колонке тревога Claude или уведомление", w::font(13.0), p.warn);
                }
            });
        });
    });
}

pub fn show(ui: &mut Ui, cx: &mut Cx, _st: &mut State) {
    let full = ui.max_rect().shrink(20.0);
    let s = &cx.snap.screen;
    let capturing = s.status == CaptureStatus::Capturing;
    let selecting = s.status == CaptureStatus::Selecting;
    let sub = match (capturing, s.source_size) {
        (true, Some((w, h))) => format!("источник {w}×{h} — выделите область рамкой"),
        _ => "трансляция области экрана или окна".into(),
    };
    let top = area(ui, full, |ui| {
        ui.spacing_mut().item_spacing.y = GAP;
        w::page_header(ui, "Экран", &sub, |ui| {
            if s.has_token || capturing {
                let k = Key::new("Другой источник…").icon("refresh").enabled(!selecting).tip("Снова показать диалог выбора экрана или окна");
                if k.show(ui).clicked() {
                    cx.send(Command::SelectSource);
                }
            }
            let (label, icon) = if capturing {
                ("Остановить захват", "stop")
            } else if selecting {
                ("Ожидание выбора…", "screen")
            } else if s.has_token {
                ("Начать захват", "screen")
            } else {
                ("Выбрать экран…", "screen")
            };
            if Key::new(label).icon(icon).enabled(!selecting).show(ui).clicked() {
                cx.send(if capturing {
                    Command::StopCapture
                } else if s.has_token {
                    Command::StartCapture
                } else {
                    Command::SelectSource
                });
            }
        });
        if !s.stale_portals.is_empty() {
            stale_panel(ui, cx);
        }
        ui.min_rect().bottom()
    });
    let bh = 14.0 + 74.0 + 17.0;
    let bottom_rect = Rect::from_min_max(pos2(full.left(), full.bottom() - bh), full.max);
    let canvas_rect = Rect::from_min_max(pos2(full.left(), top + GAP), pos2(full.right(), bottom_rect.top() - GAP));
    canvas(ui, cx, canvas_rect);
    bottom(ui, cx, bottom_rect);
}
