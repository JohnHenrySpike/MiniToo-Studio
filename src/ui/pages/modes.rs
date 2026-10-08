//! «Режимы» (§16.9): rotation, the list of modes with live thumbnails, the selected mode's
//! card and its settings (§8.x), desktop notifications.

use super::area;
use crate::api::{Command, DisplayMode, ModeInfo};
use crate::live::{City, ClockView, GithubView, ModeCommand, ModeView, NowPlayingView, PomodoroView, RunState, VisualizerView};
use crate::ui::Cx;
use crate::ui::pixel::{self, PixStyle};
use crate::ui::theme::{WHITE, pal};
use crate::ui::widgets::{self as w, Field, Key, Spin};
use egui::{Align, Id, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State {
    selected: Option<String>,
    city_query: String,
    city_typed: Option<Instant>,
    city_sent: String,
    result_focus: Option<usize>,
    repo_input: String,
    token_input: String,
    ignore_input: Option<String>,
}

const NOTIFY: &str = "notify";

fn icon_of(id: &str) -> &'static str {
    match id {
        "clock" => "clock",
        "sysmon" => "sysmon",
        "nowplaying" => "music",
        "pomodoro" => "timer",
        "claudestats" => "sparkle",
        "github" => "branch",
        "visualizer" => "wave",
        _ => "bell",
    }
}

fn mmss(sec: u32) -> String {
    format!("{}:{:02}", sec / 60, sec % 60)
}

fn rotation_box(ui: &mut Ui, cx: &mut Cx) {
    let p = pal();
    let rot = &cx.snap.rotation;
    let cycling = rot.running && rot.checked > 1;
    w::panel_ex(ui, [12.0, 12.0, 12.0, 15.0], None, 0.0, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        w::row(ui, 33.0, 6.0, |ui| {
            let c = if rot.running { p.accent_text } else { p.text };
            w::pixel_icon(ui, "refresh", 1, c, None);
            w::pixel_text(ui, "Ротация", PixStyle::new(12), c);
            w::right(ui, |ui| {
                let fmt = |v: i64| format!("{v} с");
                let before = ui.cursor().right();
                if let Some(v) = Spin::new("rot-interval", rot.interval as i64, 10, 600).step(5).width(124.0).fmt(&fmt).show(ui) {
                    cx.send(Command::SetRotationInterval(v as u32));
                }
                let r = Rect::from_min_max(pos2(ui.cursor().right(), ui.min_rect().top()), pos2(before, ui.min_rect().bottom()));
                let resp = ui.interact(r, ui.id().with("rot-tip"), Sense::hover());
                w::tip(resp, "Сколько секунд показывать каждый режим");
            });
        });
        w::row(ui, 33.0, 6.0, |ui| {
            let next_w = if cycling { 40.0 + 6.0 } else { 0.0 };
            let key = Key::new(if rot.running { "Ротация идёт" } else { "Запустить ротацию" })
                .icon(if rot.running { "check" } else { "play" })
                .accent(!rot.running)
                .checked(rot.running)
                .enabled(rot.running || rot.checked > 0)
                .width(ui.available_width() - next_w)
                .tip(if rot.running { "Остановить ротацию — текущий режим останется на колонке" } else { "Показывать отмеченные режимы по очереди" });
            if key.show(ui).clicked() {
                cx.send(if rot.running { Command::StopRotation } else { Command::StartRotation });
            }
            if cycling && Key::icon_only("next").tip("Следующий режим сейчас").show(ui).clicked() {
                cx.send(Command::RotationNext);
            }
        });
        if cycling {
            let w_ = ui.available_width();
            w::progress(ui, rot.progress.clamp(0.0, 1.0), w_, if rot.paused { p.text_dim } else { p.accent });
        }
        let text = if rot.checked == 0 {
            "Отметьте галочкой режимы, которые будут сменять друг друга на колонке.".to_string()
        } else if !rot.running {
            if rot.checked == 1 {
                "Отмечен один режим — он будет просто показан. Отметьте ещё, чтобы они сменялись.".into()
            } else {
                format!("Отмечено режимов: {}. Порядок — как в списке.", rot.checked)
            }
        } else if rot.checked == 1 {
            "Отмечен один режим — показывается без смены.".into()
        } else if rot.paused {
            "Пауза: поверх показывается уведомление или сигнал Claude.".into()
        } else {
            format!("Дальше «{}» через {}", rot.next_title, mmss(rot.seconds_left))
        };
        w::hint_small(ui, &text, 11.0);
    });
}

struct RowData<'a> {
    id: &'a str,
    title: &'a str,
    status: String,
    mode: Option<&'a ModeInfo>,
    live: bool,
}

/// One row of the list; returns (row clicked, rotation box clicked).
fn mode_row(ui: &mut Ui, cx: &mut Cx, d: &RowData, current: bool) -> (bool, bool) {
    let p = pal();
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 66.0), Sense::hover());
    let id = ui.id().with(("mode-row", d.id));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter().clone();
    if current {
        painter.rect_filled(rect, 8, p.selection);
        painter.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 10.0), vec2(3.0, rect.height() - 20.0)), 1, p.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect, 8, p.hover);
    }
    let thumb = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - 27.0), vec2(66.0, 54.0));
    if let Some(m) = d.mode {
        let (_, size) = w::screen_size(0.35);
        let glass = w::paint_screen_frame(&painter, thumb.center() - size / 2.0, 0.35, d.live, false);
        if let Some(f) = &m.frame {
            let tex = cx.tex.frame(ui.ctx(), &format!("mode/{}", m.id), f, true);
            w::paint_texture(&painter, glass, tex, WHITE);
        }
        if d.live {
            let lr = Rect::from_min_size(pos2(thumb.right() - 7.0, thumb.top() - 1.0), vec2(8.0, 8.0));
            w::paint_led(&painter, lr, p.accent, true, 1.0);
            w::tip(ui.interact(lr.expand(2.0), id.with("led"), Sense::hover()), "Сейчас на колонке");
        }
    } else {
        pixel::paint_icon_centered(&painter, thumb.center(), "bell", 3, p.text, None);
    }
    let rotatable = d.mode.is_some();
    let check_rect = Rect::from_min_size(pos2(rect.right() - 4.0 - 28.0, rect.center().y - 14.0), vec2(28.0, 28.0));
    let tx = thumb.right() + 10.0;
    let text_right = if rotatable { check_rect.left() - 6.0 } else { rect.right() - 6.0 };
    let tw = (text_right - tx).max(20.0);
    let st = PixStyle::new(12);
    let th = st.height(d.title);
    let status = w::galley_elided(ui, &d.status, w::font(11.0), p.text_dim, tw);
    let col_h = th + 3.0 + status.size().y;
    let top = rect.center().y - col_h / 2.0;
    let fg = if current { p.accent_text } else { p.text };
    pixel::paint_icon(&painter, pos2(tx, top + th / 2.0 - 6.0), icon_of(d.id), 1, if current { p.accent_text } else { p.text_dim }, None);
    pixel::paint_text(&painter, pos2(tx + 18.0, top), &st.elide(d.title, tw - 18.0), &st, fg);
    painter.galley(pos2(tx, top + th + 3.0), status, p.text_dim);
    let mut toggled = false;
    if let Some(m) = d.mode {
        let cresp = ui.interact(check_rect, id.with("rot"), Sense::click());
        let b = Rect::from_min_size(check_rect.center() - vec2(10.0, 10.0), vec2(20.0, 20.0));
        let line = if m.in_rotation {
            p.accent_edge
        } else if cresp.hovered() {
            p.text_dim
        } else {
            p.well_line
        };
        w::paint_well(&painter, b, 4.0, false, if m.in_rotation { p.accent } else { p.well }, line);
        if m.in_rotation {
            pixel::paint_icon(&painter, b.center() - vec2(6.0, 6.0), "check", 1, WHITE, None);
        }
        let cresp = w::tip(cresp, if m.in_rotation { "В ротации — убрать" } else { "Добавить в ротацию" });
        toggled = cresp.clicked();
    }
    (resp.clicked() && !toggled, toggled)
}

fn mode_list(ui: &mut Ui, cx: &mut Cx, st: &mut State, selected: &str, rect: Rect) {
    let p = pal();
    w::paint_panel(ui.painter(), rect);
    let inner = Rect::from_min_max(rect.min + vec2(6.0, 6.0), rect.max - vec2(6.0, 9.0));
    let snap = cx.snap;
    area(ui, inner, |ui| {
        egui::ScrollArea::vertical().id_salt("mode-list").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for m in &snap.modes {
                let live = snap.mode == DisplayMode::Live && snap.live_on_device == Some(m.id);
                let d = RowData { id: m.id, title: m.title, status: m.status.clone(), mode: Some(m), live };
                let (clicked, toggled) = mode_row(ui, cx, &d, selected == m.id);
                if clicked {
                    st.selected = Some(m.id.to_string());
                }
                if toggled {
                    cx.send(Command::SetRotationMember(m.id.to_string(), !m.in_rotation));
                }
            }
            w::separator(ui, 8.0);
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                w::pixel_text(ui, "ПОВЕРХ РЕЖИМОВ", PixStyle::new(9).spacing(1), p.text_dim);
            });
            let n = &snap.notify;
            let status = if n.enabled { format!("включены, {} с", n.duration) } else { "выключены".into() };
            let d = RowData { id: NOTIFY, title: "Уведомления", status, mode: None, live: false };
            if mode_row(ui, cx, &d, selected == NOTIFY).0 {
                st.selected = Some(NOTIFY.to_string());
            }
        });
    });
}

fn mode_card(ui: &mut Ui, cx: &mut Cx, m: &ModeInfo) {
    let p = pal();
    let snap = cx.snap;
    let on_device = snap.mode == DisplayMode::Live && snap.live_on_device == Some(m.id);
    let rotating = snap.rotation.running;
    w::panel_ex(ui, [18.0, 18.0, 18.0, 21.0], None, 0.0, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let width = ui.available_width();
        let factor = (((width - 16.0) / 160.0 * 4.0).floor() / 4.0).clamp(1.0, 2.0);
        let two_cols = width >= 600.0;
        let (_, fs) = w::screen_size(factor);
        let draw_screen = |ui: &mut Ui, cx: &mut Cx| {
            let (rect, _) = ui.allocate_exact_size(vec2(if two_cols { fs.x } else { width }, fs.y), Sense::hover());
            let glass = w::paint_screen_frame(ui.painter(), pos2(rect.center().x - fs.x / 2.0, rect.top()), factor, on_device, false);
            if let Some(f) = &m.frame {
                let tex = cx.tex.frame(ui.ctx(), &format!("mode/{}/big", m.id), f, false);
                w::paint_texture(ui.painter(), glass, tex, WHITE);
            }
        };
        let info = |ui: &mut Ui, cx: &mut Cx| {
            ui.spacing_mut().item_spacing.y = 8.0;
            w::row(ui, 28.0, 10.0, |ui| {
                w::pixel_icon(ui, icon_of(m.id), 2, p.text, Some(p.accent));
                w::pixel_text(ui, m.title, PixStyle::new(12).zoom(2), p.text);
            });
            w::para(ui, m.subtitle, w::font(13.0), p.text_dim);
            if !m.status.is_empty() {
                w::para(ui, &m.status, w::font(13.0), p.text);
            }
            ui.add_space(6.0);
            let text = if on_device {
                if rotating { "На колонке, идёт ротация" } else { "Показывается на колонке" }
            } else if rotating {
                "Показать только этот режим"
            } else {
                "Показать на колонке"
            };
            let key = Key::new(text).icon(if on_device { "check" } else { "send" }).accent(!on_device).checked(on_device).height(42.0).width(ui.available_width());
            if key.show(ui).clicked() && !on_device {
                cx.send(Command::ShowLive(m.id.to_string()));
            }
            let hint = if rotating && !on_device {
                "Идёт ротация: выбор одного режима её остановит.".to_string()
            } else if on_device {
                format!(
                    "Колонка получает новый кадр, только когда картинка меняется. {} вернётся после перезапуска приложения.",
                    if rotating { "Ротация" } else { "Режим" }
                )
            } else {
                "Предпросмотр живой: так режим будет выглядеть на колонке.".to_string()
            };
            w::hint_small(ui, &hint, 11.0);
        };
        if two_cols {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 22.0;
                draw_screen(ui, cx);
                ui.vertical(|ui| info(ui, cx));
            });
        } else {
            ui.spacing_mut().item_spacing.y = 16.0;
            draw_screen(ui, cx);
            ui.vertical(|ui| info(ui, cx));
        }
    });
}

// ---------------------------------------------------------------------- per-mode settings

fn send_mode(cx: &Cx, id: &str, c: ModeCommand) {
    cx.send(Command::Mode(id.to_string(), c));
}

fn clock_settings(ui: &mut Ui, cx: &mut Cx, st: &mut State, v: &ClockView) {
    let p = pal();
    let faces = [("Небо", None), ("Неон", None), ("Пиксели", None)];
    let lw = w::text_width(ui, "Циферблат", w::font(13.0));
    let picked = if lw + 10.0 + w::tabs_width(ui, &faces) <= ui.available_width() {
        w::row(ui, 33.0, 10.0, |ui| {
            w::text(ui, "Циферблат", w::font(13.0), p.text);
            w::tabs(ui, &faces, v.style.clamp(0, 2) as usize)
        })
    } else {
        w::text(ui, "Циферблат", w::font(13.0), p.text);
        w::tabs(ui, &faces, v.style.clamp(0, 2) as usize)
    };
    if let Some(i) = picked {
        send_mode(cx, "clock", ModeCommand::ClockStyle(i as i64));
    }
    w::row(ui, 34.0, 8.0, |ui| {
        w::pixel_icon(ui, "location", 2, p.text, Some(p.accent));
        let h = 15.0 + if v.city.as_ref().is_some_and(|c| !c.region.is_empty()) { 14.0 } else { 0.0 };
        let cw = ui.available_width() - if v.city.is_some() { 140.0 } else { 0.0 };
        w::col(ui, cw.max(60.0), h, |ui| {
            match &v.city {
                Some(c) => {
                    w::pixel_text(ui, &c.name, PixStyle::new(12), p.text);
                    if !c.region.is_empty() {
                        w::text(ui, &c.region, w::font(11.0), p.text_dim);
                    }
                }
                None => {
                    w::pixel_text(ui, "Город не выбран", PixStyle::new(12), p.text_dim);
                }
            }
        });
        if v.city.is_some() {
            w::right(ui, |ui| {
                if Key::new("Убрать погоду").icon("close").flat().show(ui).clicked() {
                    send_mode(cx, "clock", ModeCommand::ClockClearCity);
                }
            });
        }
    });

    // search with live results
    let field_id = Id::new("city-search");
    let busy = v.searching;
    let mut field_resp = None;
    w::row(ui, 32.0, 8.0, |ui| {
        let width = ui.available_width() - if busy { 38.0 } else { 0.0 };
        let hint = if v.city.is_some() { "Другой город…" } else { "Начните вводить название города…" };
        let r = w::text_field(ui, &mut st.city_query, Field { hint, width, id: Some(field_id), ..Default::default() });
        if busy {
            w::busy(ui);
        }
        field_resp = Some(r);
    });
    let r = field_resp.unwrap();
    if r.changed() {
        st.city_typed = Some(Instant::now());
        st.result_focus = None;
    }
    if let Some(t) = st.city_typed {
        if t.elapsed() >= Duration::from_millis(300) {
            st.city_typed = None;
            if st.city_query != st.city_sent {
                st.city_sent = st.city_query.clone();
                send_mode(cx, "clock", ModeCommand::ClockSearch(st.city_query.trim().to_string()));
            }
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(300) - t.elapsed());
        }
    }
    let pick = |cx: &Cx, st: &mut State, c: &City| {
        send_mode(cx, "clock", ModeCommand::ClockPickCity(c.clone()));
        st.city_query.clear();
        st.city_sent.clear();
        st.result_focus = None;
    };
    if w::submitted(ui, &r) && !v.results.is_empty() {
        pick(cx, st, &v.results[0]);
    }
    if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !v.results.is_empty() {
        st.result_focus = Some(0);
        r.surrender_focus();
    }
    if let Some(i) = st.result_focus {
        let n = v.results.len();
        if n == 0 {
            st.result_focus = None;
        } else {
            let (down, up, enter, esc) = ui.input(|inp| {
                (inp.key_pressed(egui::Key::ArrowDown), inp.key_pressed(egui::Key::ArrowUp), inp.key_pressed(egui::Key::Enter), inp.key_pressed(egui::Key::Escape))
            });
            if down {
                st.result_focus = Some((i + 1).min(n - 1));
            } else if up {
                if i == 0 {
                    st.result_focus = None;
                    ui.memory_mut(|m| m.request_focus(field_id));
                } else {
                    st.result_focus = Some(i - 1);
                }
            } else if enter {
                let c = v.results[i.min(n - 1)].clone();
                pick(cx, st, &c);
            } else if esc {
                st.result_focus = None;
            }
        }
    }
    let typed_pending = st.city_typed.is_some();
    let show_results = !v.results.is_empty() || (st.city_query.chars().count() >= 2 && !busy && !typed_pending);
    if show_results && !st.city_query.is_empty() || !v.results.is_empty() {
        let width = ui.available_width();
        const NOTHING: &str = "Ничего не найдено. Попробуйте по-русски или по-английски.";
        let nothing_h = w::galley_wrapped(ui, NOTHING, w::font(13.0), p.text_dim, width - 24.0).size().y;
        let h = if v.results.is_empty() { nothing_h + 16.0 } else { v.results.len() as f32 * 32.0 } + 8.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
        w::well(ui.painter(), rect, 6.0, false);
        let inner = rect.shrink(4.0);
        let mut picked = None;
        area(ui, inner, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if v.results.is_empty() {
                let r = Rect::from_min_size(inner.min + vec2(8.0, 8.0), vec2(inner.width() - 16.0, nothing_h));
                area(ui, r, |ui| w::para(ui, NOTHING, w::font(13.0), p.text_dim));
            }
            for (i, c) in v.results.iter().enumerate() {
                let label = if c.region.is_empty() { c.name.clone() } else { format!("{}  ·  {}", c.name, c.region) };
                if w::item_row(ui, &label, st.result_focus == Some(i), inner.width()).clicked() {
                    picked = Some(c.clone());
                }
            }
        });
        if let Some(c) = picked {
            pick(cx, st, &c);
        }
    }
    if let Some(e) = &v.search_error {
        w::para(ui, e, w::font(13.0), p.danger);
    }
    w::hint_small(ui, "Погода — Open-Meteo, без регистрации; обновляется раз в 15 минут.", 11.0);
}

fn player_settings(ui: &mut Ui, cx: &mut Cx, v: &NowPlayingView) {
    let p = pal();
    if !v.available || v.player.is_empty() {
        w::hint(ui, "Ни один плеер не запущен. Подойдёт любой с поддержкой MPRIS: браузер, Spotify, Elisa, VLC…");
    } else {
        let track = match (v.artist.is_empty(), v.title.is_empty()) {
            (false, false) => format!("{} — {}", v.artist, v.title),
            (true, false) => v.title.clone(),
            (false, true) => v.artist.clone(),
            _ => String::new(),
        };
        let line = if track.is_empty() { v.player.clone() } else { format!("{}: {track}", v.player) };
        w::para(ui, &line, w::font(13.0), p.text);
    }
    let on = v.available && !v.player.is_empty();
    w::row(ui, 33.0, 8.0, |ui| {
        if Key::icon_only("prev").enabled(on).show(ui).clicked() {
            send_mode(cx, "nowplaying", ModeCommand::Previous);
        }
        let k = Key::new(if v.playing { "Пауза" } else { "Играть" }).icon(if v.playing { "pause" } else { "play" }).accent(true).enabled(on);
        if k.show(ui).clicked() {
            send_mode(cx, "nowplaying", ModeCommand::PlayPause);
        }
        if Key::icon_only("next").enabled(on).show(ui).clicked() {
            send_mode(cx, "nowplaying", ModeCommand::Next);
        }
    });
}

fn label_col(ui: &mut Ui, text: &str, width: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(width, 33.0), Sense::hover());
    let g = w::galley(ui, text, w::font(13.0), pal().text);
    ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, pal().text);
}

fn pomodoro_settings(ui: &mut Ui, cx: &mut Cx, v: &PomodoroView) {
    w::row(ui, 33.0, 8.0, |ui| {
        let k = Key::new(if v.running { "Пауза" } else { "Старт" }).icon(if v.running { "pause" } else { "play" }).accent(true);
        if k.show(ui).clicked() {
            send_mode(cx, "pomodoro", ModeCommand::PomodoroStartPause);
        }
        if Key::new("Пропустить").icon("next").show(ui).clicked() {
            send_mode(cx, "pomodoro", ModeCommand::PomodoroSkip);
        }
        if Key::new("Сброс").icon("refresh").show(ui).clicked() {
            send_mode(cx, "pomodoro", ModeCommand::PomodoroReset);
        }
    });
    type Row<'a> = (&'a str, &'a str, u32, i64, fn(u32) -> ModeCommand);
    let rows: [Row; 3] = [
        ("Фокус, мин", "pomo-work", v.work_min, 180, ModeCommand::PomodoroWork),
        ("Перерыв, мин", "pomo-break", v.break_min, 60, ModeCommand::PomodoroBreak),
        ("Длинный перерыв, мин", "pomo-long", v.long_min, 90, ModeCommand::PomodoroLong),
    ];
    let lw = rows.iter().map(|r| w::text_width(ui, r.0, w::font(13.0))).fold(0.0, f32::max) + 12.0;
    ui.spacing_mut().item_spacing.y = 8.0;
    for (label, id, value, max, cmd) in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            label_col(ui, label, lw);
            if let Some(n) = Spin::new(id, value as i64, 1, max).show(ui) {
                send_mode(cx, "pomodoro", cmd(n as u32));
            }
        });
    }
    w::hint_small(
        ui,
        "Длинный перерыв — после каждого четвёртого помидора. Когда этап заканчивается, на колонке на несколько секунд появляется карточка, даже если показывается другой режим.",
        11.0,
    );
}

fn github_settings(ui: &mut Ui, cx: &mut Cx, st: &mut State, v: &GithubView) {
    let p = pal();
    for repo in &v.repos {
        w::row(ui, 34.0, 10.0, |ui| {
            let (color, lit, blink) = match repo.state {
                RunState::Passed => (p.ok, true, false),
                RunState::Failed | RunState::Error => (p.danger, true, false),
                RunState::Running => (p.warn, true, true),
                RunState::Neutral | RunState::Loading => (p.text_dim, false, false),
            };
            w::led(ui, color, lit, blink);
            let avail = ui.available_width() - 30.0 - 10.0;
            w::col(ui, avail, 30.0, |ui| {
                w::pixel_text_elided(ui, &repo.name, PixStyle::new(12), p.text, avail);
                w::text_elided(ui, &repo.detail, w::font(11.0), p.text_dim, Some(avail));
            });
            if Key::icon_only("close").flat().tip("Убрать").show(ui).clicked() {
                send_mode(cx, "github", ModeCommand::GithubRemove(repo.name.clone()));
            }
        });
    }
    if v.repos.len() < 4 {
        let mut add = false;
        w::row(ui, 33.0, 8.0, |ui| {
            let key = Key::new("Добавить").icon("plus");
            let kw = key.size(ui).x;
            let r = w::text_field(ui, &mut st.repo_input, Field { hint: "owner/repo или ссылка на GitHub", width: ui.available_width() - kw - 8.0, ..Default::default() });
            if w::submitted(ui, &r) {
                add = true;
            }
            if key.show(ui).clicked() {
                add = true;
            }
        });
        if add && !st.repo_input.trim().is_empty() {
            send_mode(cx, "github", ModeCommand::GithubAdd(st.repo_input.trim().to_string()));
            st.repo_input.clear();
        }
    }
    if let Some(e) = &v.add_error {
        w::para(ui, e, w::font(13.0), p.danger);
    }
    if v.repos.len() < 4 {
        let examples = ["cli/cli", "neovim/neovim", "rust-lang/rust", "microsoft/vscode"];
        let keys = examples.iter().map(|e| Key::new(e).flat()).collect();
        if let Some(i) = w::flow_keys(ui, Some("Например:"), keys, 6.0) {
            send_mode(cx, "github", ModeCommand::GithubAdd(examples[i].to_string()));
        }
    }
    w::separator(ui, 0.0);
    w::row(ui, 33.0, 8.0, |ui| {
        let save = Key::new("Сохранить").enabled(!st.token_input.is_empty());
        let del = Key::new("Удалить");
        let kw = save.size(ui).x + if v.has_token { del.size(ui).x + 8.0 } else { 0.0 };
        let hint = if v.has_token { "токен сохранён — введите новый, чтобы заменить" } else { "токен GitHub (нужен для приватных репозиториев)" };
        w::text_field(ui, &mut st.token_input, Field { hint, width: ui.available_width() - kw - 8.0, password: true, ..Default::default() });
        if save.show(ui).clicked() {
            send_mode(cx, "github", ModeCommand::GithubToken(std::mem::take(&mut st.token_input)));
        }
        if v.has_token && del.show(ui).clicked() {
            send_mode(cx, "github", ModeCommand::GithubClearToken);
        }
    });
    let mins = ((v.interval as f64) / 60.0).round().max(1.0) as u64;
    w::hint_small(
        ui,
        &format!(
            "Показывается последний запуск Actions в каждом репозитории. Без токена GitHub даёт 60 запросов в час, поэтому опрос раз в {mins} мин; с токеном — раз в минуту."
        ),
        11.0,
    );
}

fn visualizer_settings(ui: &mut Ui, cx: &mut Cx, v: &VisualizerView) {
    if let Some(i) = w::tabs(ui, &[("С пиками", None), ("Зеркальные", None)], v.style.clamp(0, 1) as usize) {
        send_mode(cx, "visualizer", ModeCommand::VisualizerStyle(i as i64));
    }
    if let Some(e) = &v.error {
        w::para(ui, e, w::font(13.0), pal().danger);
    }
    w::hint_small(
        ui,
        "Спектр того, что звучит на компьютере (монитор устройства вывода PipeWire). Это поток кадров: пока режим на колонке, она получает 5–8 кадров в секунду.",
        11.0,
    );
}

fn notify_settings(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    let n = &cx.snap.notify;
    if w::switch(ui, n.enabled, "Показывать уведомления KDE на колонке", true).clicked() {
        cx.send(Command::SetNotifyEnabled(!n.enabled));
    }
    if let Some(e) = &n.error {
        w::para(ui, e, w::font(13.0), p.danger);
    }
    let lw = w::text_width(ui, "Показывать, секунд", w::font(13.0)) + 12.0;
    let en = n.enabled;
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        label_col(ui, "Показывать, секунд", lw);
        if let Some(v) = Spin::new("notify-duration", n.duration as i64, 2, 60).enabled(en).show(ui) {
            cx.send(Command::SetNotifyDuration(v as u32));
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        label_col(ui, "Не показывать от", lw);
        let joined = n.ignore.join(", ");
        let buf = st.ignore_input.get_or_insert_with(|| joined.clone());
        let width = (ui.available_width()).max(220.0);
        let r = w::text_field(ui, buf, Field { hint: "приложения через запятую", width, enabled: en, ..Default::default() });
        if r.lost_focus() {
            let list: Vec<String> = buf.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            if list != n.ignore {
                cx.send(Command::SetNotifyIgnore(list));
            }
        }
        if !r.has_focus() && !r.lost_focus() && *buf != joined {
            *buf = joined;
        }
    });
    if Key::new("Показать тестовое").icon("bell").show(ui).clicked() {
        cx.send(Command::TestNotification);
    }
    w::hint_small(ui, "Карточка поверх любого режима, изображения или трансляции; затем на колонке снова то, что было.", 11.0);
}

fn settings_group(ui: &mut Ui, cx: &mut Cx, st: &mut State, selected: &str) {
    if selected == NOTIFY {
        w::group(ui, "Уведомления рабочего стола", |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            notify_settings(ui, cx, st);
        });
        return;
    }
    let Some(m) = cx.snap.modes.iter().find(|m| m.id == selected) else { return };
    let view = m.view.clone();
    let status = m.status.clone();
    w::group(ui, "Настройки", |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        match (&view, selected) {
            (ModeView::Clock(v), _) => clock_settings(ui, cx, st, v),
            (ModeView::NowPlaying(v), _) => player_settings(ui, cx, v),
            (ModeView::Pomodoro(v), _) => pomodoro_settings(ui, cx, v),
            (ModeView::Github(v), _) => github_settings(ui, cx, st, v),
            (ModeView::Visualizer(v), _) => visualizer_settings(ui, cx, v),
            (_, "sysmon") => {
                w::hint(ui, "Загрузка процессора и видеокарты, температуры, память. Данные: /proc, датчики hwmon, nvidia-smi или sysfs AMD. Кадр раз в 2 секунды.");
            }
            (_, "claudestats") => {
                let t = format!(
                    "{}Считается по журналам Claude Code в ~/.claude/projects за сегодня: запросы, ответы и токены.",
                    if status.is_empty() { String::new() } else { format!("{status}\n\n") }
                );
                w::hint(ui, &t);
            }
            _ => {}
        }
    });
}

pub fn show(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let full = ui.max_rect().shrink(20.0);
    let snap = cx.snap;
    let selected = st
        .selected
        .clone()
        .or_else(|| snap.live_on_device.map(str::to_string))
        .unwrap_or_else(|| "clock".to_string());
    let header_h = area(ui, full, |ui| {
        w::page_header(ui, "Режимы", "экраны, которые рисуются сами и обновляются на колонке", |ui| {
            if snap.mode == DisplayMode::Live {
                let k = Key::new("Остановить").icon("stop").tip("Перестать обновлять колонку (картинка останется)");
                if k.show(ui).clicked() {
                    cx.send(Command::StopLive);
                }
            }
        });
        ui.min_rect().height()
    });
    let body = Rect::from_min_max(pos2(full.left(), full.top() + header_h + 16.0), full.max);
    let left_w = if ui.max_rect().width() < 700.0 { 266.0 } else { 310.0 };
    let left = Rect::from_min_max(body.min, pos2(body.left() + left_w, body.bottom()));
    let rot_h = area(ui, left, |ui| {
        rotation_box(ui, cx);
        ui.min_rect().height()
    });
    let list = Rect::from_min_max(pos2(left.left(), left.top() + rot_h + 12.0), left.max);
    mode_list(ui, cx, st, &selected, list);

    let right = Rect::from_min_max(pos2(left.right() + 16.0, body.top()), body.max);
    let mut child = ui.new_child(UiBuilder::new().max_rect(right).layout(Layout::top_down(Align::Min)));
    egui::ScrollArea::vertical().id_salt("mode-detail").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 14.0;
        if selected != NOTIFY
            && let Some(m) = snap.modes.iter().find(|m| m.id == selected)
        {
            mode_card(ui, cx, m);
        }
        settings_group(ui, cx, st, &selected);
        ui.add_space(4.0);
    });
}
