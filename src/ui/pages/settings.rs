//! «Настройки» (§16.12): theme, connection, transfer, application.

use super::scroll_page;
use crate::api::{Command, Theme};
use crate::ui::Cx;
use crate::ui::theme::pal;
use crate::ui::widgets::{self as w, Field, Key, Spin};
use egui::text::LayoutJob;
use egui::{Rect, Sense, Ui, pos2, vec2};

#[derive(Default)]
pub struct State {
    mac: Option<String>,
}

fn label_cell(ui: &mut Ui, text: &str, width: f32, height: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    if !text.is_empty() {
        let g = w::galley(ui, text, w::font(13.0), pal().text);
        ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, pal().text);
    }
}

/// A grid row: label column, then the content.
fn grid_row(ui: &mut Ui, label: &str, lw: f32, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        label_cell(ui, label, lw, 33.0);
        ui.vertical(|ui| add(ui));
    });
}

fn spin_hint(ui: &mut Ui, cx: &Cx, spin: Spin, hint: &str, cmd: impl Fn(i64) -> Command) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        if let Some(v) = spin.show(ui) {
            cx.send(cmd(v));
        }
        let rest = ui.available_width();
        let g = w::galley_wrapped(ui, hint, w::font(13.0), pal().text_dim, rest);
        let h = g.size().y;
        let (r, _) = ui.allocate_exact_size(vec2(rest, h.max(33.0)), Sense::hover());
        ui.painter().galley(pos2(r.left(), r.center().y - h / 2.0), g, pal().text_dim);
    });
}

fn cli_hint(ui: &mut Ui) {
    let p = pal();
    let parts: [(&str, bool); 11] = [
        ("Командная строка: ", false),
        ("--send файл.gif [--fit crop|fit|stretch]", true),
        (", ", false),
        ("--state working|alerting|chilling", true),
        (", ", false),
        ("--status", true),
        (", ", false),
        ("--hidden", true),
        (", ", false),
        ("--debug", true),
        (" (журнал и диагностика протокола).", false),
    ];
    let width = ui.available_width();
    let mut job = LayoutJob::default();
    for (t, mono) in parts {
        let font = if mono { w::mono(12.0) } else { w::font(13.0) };
        let fmt = egui::TextFormat {
            extra_letter_spacing: if mono { 0.0 } else { w::tight(13.0) },
            font_id: font,
            color: p.text_dim,
            valign: egui::Align::BOTTOM,
            ..Default::default()
        };
        job.append(t, 0.0, fmt);
    }
    job.wrap.max_width = width;
    let g = ui.fonts_mut(|f| f.layout_job(job));
    let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    ui.painter().galley(r.min, g, p.text_dim);
}

pub fn show(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    let s = &cx.snap.settings;
    let d = &cx.snap.device;
    scroll_page(ui, "settings-page", Some(860.0), |ui| {
        w::page_header(ui, "Настройки", "связь с колонкой, передача и приложение", |_| {});

        w::group(ui, "Оформление", |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let cur = if cx.snap.theme == Theme::Dark { 1 } else { 0 };
                if let Some(i) = w::tabs(ui, &[("Бежевая", Some("sun")), ("Ночная", Some("moon"))], cur) {
                    cx.theme_request = Some(if i == 1 { Theme::Dark } else { Theme::Beige });
                }
                let rest = ui.available_width();
                w::para_w(ui, "Цвета корпуса MiniToo или тёмный стол для вечера. Сама колонка справа всегда бежевая.", w::font(13.0), p.text_dim, rest, egui::Align::Min);
            });
        });

        let labels = ["MAC-адрес", "Канал RFCOMM", "Не давать засыпать"];
        let lw = labels.iter().map(|l| w::text_width(ui, l, w::font(13.0))).fold(0.0, f32::max) + 4.0;
        w::group(ui, "Колонка", |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            grid_row(ui, "MAC-адрес", lw, |ui| {
                w::row(ui, 33.0, 12.0, |ui| {
                    let key = Key::new(if d.discovering { "Поиск…" } else { "Найти" }).icon("search").enabled(!d.discovering);
                    let kw = key.size(ui).x;
                    let buf = st.mac.get_or_insert_with(|| s.mac.clone());
                    let r = w::text_field(ui, buf, Field { hint: "B1:21:81:05:E2:65", width: ui.available_width() - kw - 12.0, mono: true, ..Default::default() });
                    if r.lost_focus() {
                        let v = buf.trim().to_uppercase();
                        if v != s.mac {
                            cx.send(Command::SetMac(v));
                        }
                    }
                    if !r.has_focus() && !r.lost_focus() && *buf != s.mac {
                        *buf = s.mac.clone();
                    }
                    if key.show(ui).clicked() {
                        cx.send(Command::Discover);
                    }
                });
                if !d.discovered.is_empty() {
                    let width = ui.available_width();
                    let h = d.discovered.len() as f32 * 34.0 + 8.0;
                    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    w::paint_panel(ui.painter(), rect);
                    let inner = Rect::from_min_max(rect.min + vec2(4.0, 4.0), rect.max - vec2(4.0, 4.0));
                    let mut picked = None;
                    super::area(ui, inner, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (name, addr) in &d.discovered {
                            let n = if name.is_empty() { "без имени" } else { name };
                            if w::item_row(ui, &format!("{n}   {addr}"), *addr == s.mac, inner.width()).clicked() {
                                picked = Some(addr.clone());
                            }
                        }
                    });
                    if let Some(a) = picked {
                        st.mac = Some(a.clone());
                        cx.send(Command::SetMac(a));
                    }
                }
            });
            grid_row(ui, "Канал RFCOMM", lw, |ui| {
                if let Some(v) = Spin::new("rfcomm", s.channel as i64, 1, 30).show(ui) {
                    cx.send(Command::SetChannel(v as u8));
                }
            });
            grid_row(ui, "", lw, |ui| {
                if w::checkbox(ui, s.auto_connect, "Подключаться при запуске", true).clicked() {
                    cx.send(Command::SetAutoConnect(!s.auto_connect));
                }
            });
            grid_row(ui, "Не давать засыпать", lw, |ui| {
                spin_hint(
                    ui,
                    cx,
                    Spin::new("keepalive", s.keepalive as i64, 0, 600).step(10),
                    "секунд между служебными пингами (0 — выключено). Держит соединение активным, чтобы колонка не отключалась.",
                    |v| Command::SetKeepalive(v as u32),
                );
            });
        });

        let labels = ["Пауза между пакетами", "Сжатие zstd"];
        let lw = labels.iter().map(|l| w::text_width(ui, l, w::font(13.0))).fold(0.0, f32::max) + 4.0;
        w::group(ui, "Передача изображения", |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            grid_row(ui, "Пауза между пакетами", lw, |ui| {
                spin_hint(
                    ui,
                    cx,
                    Spin::new("chunk-delay", s.chunk_delay as i64, 0, 60),
                    "мс. Меньше — быстрее (особенно трансляция экрана), но колонка может не успевать и запрашивать пакеты повторно.",
                    |v| Command::SetChunkDelay(v as u32),
                );
            });
            grid_row(ui, "Сжатие zstd", lw, |ui| {
                spin_hint(
                    ui,
                    cx,
                    Spin::new("zstd", s.zstd_level as i64, 1, 22),
                    "уровень для картинок и анимаций (19 — компактно). Трансляция экрана всегда использует быстрый уровень.",
                    |v| Command::SetZstdLevel(v as i32),
                );
            });
        });

        w::group(ui, "Приложение", |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            if w::checkbox(ui, s.close_to_tray, "Сворачивать в трей при закрытии окна", true).clicked() {
                cx.send(Command::SetCloseToTray(!s.close_to_tray));
            }
            if w::checkbox(ui, s.start_hidden, "Запускаться свёрнутым в трей", true).clicked() {
                cx.send(Command::SetStartHidden(!s.start_hidden));
            }
            cli_hint(ui);
        });
    });
}
