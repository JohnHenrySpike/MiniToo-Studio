//! «Claude» (§16.10): current state, scenes per state, sessions and the hooks.

use super::{pick_files, scroll_page};
use crate::api::{Command, DisplayMode, SceneSet};
use crate::claude::{ClaudeState, Session};
use crate::faces;
use crate::frame::Frame;
use crate::ui::Cx;
use crate::ui::pixel::PixStyle;
use crate::ui::textures;
use crate::ui::theme::{Palette, WHITE, pal};
use crate::ui::widgets::{self as w, Key, Spin};
use egui::{Color32, Rect, Sense, Ui, pos2, vec2};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SceneThumb {
    frames: Vec<Frame>,
    speed: u32,
    key: usize,
}

#[derive(Default)]
pub struct State {
    scenes: HashMap<(ClaudeState, String), Arc<SceneThumb>>,
    copied: Option<Instant>,
    hooks_seen: Option<(bool, String)>,
}

pub fn state_color(p: &Palette, s: ClaudeState) -> Color32 {
    match s {
        ClaudeState::Working => p.accent_text,
        ClaudeState::Alerting => p.danger,
        ClaudeState::Chilling => p.info,
    }
}

fn ago(s: &Session) -> String {
    let secs = (chrono::Local::now() - s.updated).num_seconds().max(0);
    if secs < 60 {
        format!("{secs} с назад")
    } else if secs < 3600 {
        format!("{} мин назад", (secs as f64 / 60.0).round() as i64)
    } else {
        format!("{} ч назад", (secs as f64 / 3600.0).round() as i64)
    }
}

fn scene_thumb(st: &mut State, state: ClaudeState, id: &str) -> Arc<SceneThumb> {
    st.scenes
        .entry((state, id.to_string()))
        .or_insert_with(|| {
            let sc = faces::generate(state, id);
            let speed = if sc.delays.is_empty() { 200 } else { (sc.delays.iter().sum::<u32>() / sc.delays.len() as u32).max(20) };
            let key = faces::key_frame(state, id).min(sc.frames.len().saturating_sub(1));
            Arc::new(SceneThumb { frames: sc.frames, speed, key })
        })
        .clone()
}

fn paint_anim(ui: &mut Ui, cx: &mut Cx, glass: Rect, set: &SceneSet, key: &str) {
    if let Some((i, f)) = textures::anim_frame(ui.ctx(), &set.anim) {
        let tex = cx.tex.frame(ui.ctx(), &format!("{key}/{i}"), f, false);
        w::paint_texture(ui.painter(), glass, tex, WHITE);
    }
}

fn status_card(ui: &mut Ui, cx: &mut Cx) {
    let p = pal();
    let c = &cx.snap.claude;
    let mode_claude = cx.snap.mode == DisplayMode::Claude;
    w::panel_ex(ui, [18.0, 18.0, 18.0, 21.0], None, 0.0, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 22.0;
            let (_, glass) = w::screen_frame(ui, 1.5, mode_claude);
            if let Some(set) = c.scenes.iter().find(|s| s.state == c.state) {
                paint_anim(ui, cx, glass, set, "claude-now");
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                w::plate(ui, "статус");
                w::pixel_text(ui, c.state.title(), PixStyle::new(13).zoom(3), state_color(&p, c.state));
                let n = c.sessions.len();
                let t = if n == 0 { "Активных сессий нет".to_string() } else { format!("Сессий: {n}") };
                w::text(ui, &t, w::font(13.0), p.text_dim);
                if w::switch(ui, c.interrupt, "Тревога поверх картинки и трансляции", true).clicked() {
                    cx.send(Command::SetInterrupt(!c.interrupt));
                }
                if w::switch(ui, c.idle_alerts, "«Ждёт ввода» после простоя — тоже тревога", true).clicked() {
                    cx.send(Command::SetIdleAlerts(!c.idle_alerts));
                }
                let r = w::switch(ui, c.alert_caption, "Проект и вопрос на сцене «Ждёт вас»", true);
                let r = w::tip(r, "Например: «divoom — Разрешить Bash?». Если ждут несколько сессий, рядом с проектом будет «+N».");
                if r.clicked() {
                    cx.send(Command::SetAlertCaption(!c.alert_caption));
                }
            });
        });
    });
}

fn face_card(ui: &mut Ui, cx: &mut Cx, st: &mut State, set: &SceneSet, width: f32) {
    let p = pal();
    let c = &cx.snap.claude;
    let s = set.state;
    let live = c.state == s && cx.snap.mode == DisplayMode::Claude;
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 8.0;
        let (_, fs) = w::screen_size(1.0);
        let (r, _) = ui.allocate_exact_size(vec2(width, fs.y), Sense::hover());
        let glass = w::paint_screen_frame(ui.painter(), pos2(r.center().x - fs.x / 2.0, r.top()), 1.0, live, false);
        paint_anim(ui, cx, glass, set, &format!("face/{}", s.id()));
        let st12 = PixStyle::new(12);
        let tw = st12.measure(s.title()).x;
        let (r, _) = ui.allocate_exact_size(vec2(width, st12.height(s.title())), Sense::hover());
        crate::ui::pixel::paint_text(ui.painter(), pos2(r.center().x - tw / 2.0, r.top()), s.title(), &st12, state_color(&p, s));
        let caption = match &set.custom {
            Some(path) => format!("свой файл: {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            None => format!("сцена «{}»", faces::variant_title(s, &set.current)),
        };
        let cap = w::elide_middle(ui, &caption, &w::font(11.0), width);
        w::para_w(ui, &cap, w::font(11.0), p.text_dim, width, egui::Align::Center);

        // keys
        let k1 = Key::icon_only("send").tip("Показать на колонке");
        let k2 = Key::new("Свой GIF…");
        let k3 = Key::icon_only("refresh").tip("Вернуть встроенные сцены");
        let mut kw = k1.size(ui).x + 6.0 + k2.size(ui).x;
        if set.custom.is_some() {
            kw += 6.0 + k3.size(ui).x;
        }
        w::row(ui, 33.0, 6.0, |ui| {
            ui.add_space(((width - kw) / 2.0 - 6.0).max(0.0));
            if k1.show(ui).clicked() {
                cx.send(Command::ShowStateOnDevice(s));
            }
            if k2.show(ui).clicked() {
                let core = cx.core.clone();
                const FILTERS: &[(&str, &[&str])] = &[("Изображения", &["gif", "png", "webp", "jpg", "jpeg"]), ("Все файлы", &["*"])];
                pick_files("GIF или изображение для статуса", FILTERS, false, move |f| {
                    if let Some(path) = f.into_iter().next() {
                        core.send(Command::SetCustomFace(s, Some(path)));
                    }
                });
            }
            if set.custom.is_some() && k3.show(ui).clicked() {
                cx.send(Command::SetCustomFace(s, None));
            }
        });

        // scene thumbnails
        let variants = faces::variants(s);
        let grid_w = (3.0 * 72.0 + 2.0 * 6.0f32).min(width);
        let cols = (((grid_w + 6.5) / 78.0).floor() as usize).max(1);
        let rows = variants.len().div_ceil(cols);
        let cell_h = 60.0 + 2.0 + 28.0;
        let (area, _) = ui.allocate_exact_size(vec2(width, rows as f32 * (cell_h + 6.0)), Sense::hover());
        let gx = area.center().x - (cols as f32 * 78.0 - 6.0) / 2.0;
        let dim = if set.custom.is_some() { 0.45 } else { 1.0 };
        for (n, id) in variants.iter().enumerate() {
            let x = gx + (n % cols) as f32 * 78.0;
            let y = area.top() + (n / cols) as f32 * (cell_h + 6.0);
            let enabled = !set.off.iter().any(|o| o == id);
            let current = set.current == *id && set.custom.is_none();
            let thumb = scene_thumb(st, s, id);
            let frame_rect = Rect::from_min_size(pos2(x, y), vec2(72.0, 60.0));
            let rid = ui.id().with(("scene", s.id(), *id));
            let resp = ui.interact(frame_rect, rid, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let hovered = resp.hovered();
            let glass = w::paint_screen_frame(ui.painter(), frame_rect.min, 0.375, current, false);
            let idx = if hovered && thumb.frames.len() > 1 { textures::anim_index(ui.ctx(), thumb.frames.len(), thumb.speed) } else { thumb.key };
            if let Some(f) = thumb.frames.get(idx) {
                let tex = cx.tex.frame(ui.ctx(), &format!("scene/{}/{id}/{idx}", s.id()), f, true);
                let op = dim * if enabled { 1.0 } else { 0.35 };
                w::paint_texture(ui.painter(), glass, tex, Color32::WHITE.gamma_multiply(op));
            }
            if dim < 1.0 || !enabled {
                let op = 1.0 - dim * if enabled { 1.0 } else { 0.35 };
                ui.painter().rect_filled(frame_rect.expand(3.0), 4, p.shell.gamma_multiply(op * 0.85));
            }
            let title = faces::variant_title(s, id);
            let resp = w::tip(resp, &format!("«{title}» — нажмите, чтобы показать на колонке"));
            if resp.clicked() {
                cx.send(Command::PickScene(s, id.to_string()));
            }
            let cb = Rect::from_min_size(pos2(x + 22.0, y + 62.0), vec2(28.0, 28.0));
            let cresp = ui.interact(cb, rid.with("on"), Sense::click());
            let b = Rect::from_min_size(cb.center() - vec2(10.0, 10.0), vec2(20.0, 20.0));
            let line = if enabled { p.accent_edge } else if cresp.hovered() { p.text_dim } else { p.well_line };
            w::paint_well(ui.painter(), b, 4.0, false, (if enabled { p.accent } else { p.well }).gamma_multiply(dim), line.gamma_multiply(dim));
            if enabled {
                crate::ui::pixel::paint_icon(ui.painter(), b.center() - vec2(6.0, 6.0), "check", 1, WHITE.gamma_multiply(dim), None);
            }
            let cresp = w::tip(cresp, if enabled { "В наборе — убрать" } else { "Добавить в набор" });
            if cresp.clicked() {
                cx.send(Command::SetSceneEnabled(s, id.to_string(), !enabled));
            }
        }
    });
}

fn faces_group(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    let snap = cx.snap;
    w::group(ui, "Анимации состояний", |ui| {
        ui.spacing_mut().item_spacing.y = 14.0;
        let width = ui.available_width();
        let three = width >= 690.0;
        let sets: Vec<SceneSet> = ClaudeState::ALL
            .iter()
            .map(|&s| snap.claude.scenes.iter().find(|x| x.state == s).cloned().unwrap_or(SceneSet { state: s, ..Default::default() }))
            .collect();
        if three {
            let cw = (width - 24.0) / 3.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for set in &sets {
                    face_card(ui, cx, st, set, cw);
                }
            });
        } else {
            ui.spacing_mut().item_spacing.y = 18.0;
            for set in &sets {
                face_card(ui, cx, st, set, width);
            }
            ui.spacing_mut().item_spacing.y = 14.0;
        }
        const SCENE_HINT: &str = "При каждой смене состояния показывается случайная сцена из отмеченных галочкой; если состояние долго не меняется — следующая через заданное время (кроме «Ждёт вас»). Свой GIF заменяет все сцены состояния.";
        let lw = w::text_width(ui, "Менять сцену каждые", w::font(13.0));
        let rest = ui.available_width() - lw - 140.0 - 20.0;
        let hh = w::galley_wrapped(ui, SCENE_HINT, w::font(12.0), p.text_dim, rest).size().y;
        w::row(ui, hh.max(33.0), 10.0, |ui| {
            w::text(ui, "Менять сцену каждые", w::font(13.0), p.text);
            let fmt = |v: i64| if v == 0 { "нет".to_string() } else { format!("{v} мин") };
            if let Some(v) = Spin::new("scene-minutes", snap.claude.scene_minutes as i64, 0, 60).width(140.0).fmt(&fmt).show(ui) {
                cx.send(Command::SetSceneMinutes(v as u32));
            }
            w::para_w(ui, SCENE_HINT, w::font(12.0), p.text_dim, rest, egui::Align::Min);
        });
    });
}

fn sessions_group(ui: &mut Ui, cx: &mut Cx) {
    let p = pal();
    let c = &cx.snap.claude;
    ui.ctx().request_repaint_after(Duration::from_secs(5));
    w::group(ui, "Сессии", |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        for s in &c.sessions {
            w::row(ui, 34.0, 12.0, |ui| {
                w::led(ui, state_color(&p, s.state), true, s.state == ClaudeState::Alerting);
                w::col(ui, 220.0, 30.0, |ui| {
                    w::pixel_text_elided(ui, &s.project(), PixStyle::new(12), p.text, 220.0);
                    let cwd = w::elide_middle(ui, &s.cwd, &w::font(11.0), 220.0);
                    w::text(ui, &cwd, w::font(11.0), p.text_dim);
                });
                let (r, _) = ui.allocate_exact_size(vec2(90.0, 18.0), Sense::hover());
                let g = w::galley(ui, s.state.title(), w::font(13.0), state_color(&p, s.state));
                ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, Color32::WHITE);
                let when = ago(s);
                let when_w = w::text_width(ui, &when, w::font(13.0));
                let msg = if s.message.is_empty() { &s.last_event } else { &s.message };
                let mw = (ui.available_width() - when_w - 12.0).max(20.0);
                w::text_elided(ui, msg, w::font(13.0), p.text, Some(mw));
                w::right(ui, |ui| w::text(ui, &when, w::font(13.0), p.text_dim));
            });
        }
        if c.sessions.is_empty() {
            let t = if c.hooks_installed {
                "Нет активных сессий. Запустите Claude Code — сессии появятся здесь."
            } else {
                "Установите хуки ниже, чтобы Claude Code сообщал свой статус."
            };
            w::hint(ui, t);
        }
        ui.add_space(4.0);
        w::row(ui, 30.0, 6.0, |ui| {
            w::text(ui, "Проверить:", w::font(13.0), p.text_dim);
            for s in ClaudeState::ALL {
                let k = Key::new(s.title()).flat().tip("На 10 секунд: колонка покажет сцену в любом режиме, затем вернётся к прежнему");
                if k.show(ui).clicked() {
                    cx.send(Command::TestState(s));
                }
            }
            w::right(ui, |ui| {
                if Key::new("Очистить").icon("trash").flat().show(ui).clicked() {
                    cx.send(Command::ClearSessions);
                }
            });
        });
    });
}

fn hooks_group(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    let c = &cx.snap.claude;
    if c.hooks_message != st.hooks_seen {
        st.hooks_seen = c.hooks_message.clone();
        st.copied = None;
    }
    w::group(ui, "Подключение к Claude Code", |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        let lw = w::text_width(ui, "Сервер событий", w::font(13.0));
        w::row(ui, 33.0, 10.0, |ui| {
            w::led(ui, if c.listening { p.ok } else { p.danger }, true, false);
            let (r, _) = ui.allocate_exact_size(vec2(lw, 18.0), Sense::hover());
            let g = w::galley(ui, "Сервер событий", w::font(13.0), p.text);
            ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text);
            ui.spacing_mut().item_spacing.x = 8.0;
            w::text(ui, "127.0.0.1:", w::font(13.0), p.text);
            let fmt = |v: i64| v.to_string();
            if let Some(v) = Spin::new("http-port", c.port as i64, 1024, 65535).width(150.0).fmt(&fmt).show(ui) {
                cx.send(Command::SetPort(v as u16));
            }
            let (t, col) = if c.listening {
                ("слушает; после смены порта переустановите хуки", p.text_dim)
            } else {
                ("порт занят — выберите другой", p.danger)
            };
            let rest = ui.available_width();
            w::para_w(ui, t, w::font(12.0), col, rest, egui::Align::Min);
        });
        w::row(ui, 22.0, 10.0, |ui| {
            w::led(ui, p.ok, c.hooks_installed, false);
            let (r, _) = ui.allocate_exact_size(vec2(lw, 18.0), Sense::hover());
            let g = w::galley(ui, "Хуки", w::font(13.0), p.text);
            ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text);
            let t = if c.hooks_installed { format!("установлены в {}", c.hooks_path) } else { "не установлены".into() };
            w::text_elided(ui, &t, w::font(13.0), p.text, None);
        });
        w::hint(
            ui,
            &format!(
                "Кнопка добавит в {} хуки SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Notification, Stop и SessionEnd (с резервной копией файла). Каждый хук — короткий curl на локальный порт; если приложение не запущено, он молча завершится и Claude не заметит.",
                c.hooks_path
            ),
        );
        w::row(ui, 33.0, 8.0, |ui| {
            if Key::new("Установить хуки").icon("plus").accent(!c.hooks_installed).show(ui).clicked() {
                cx.send(Command::InstallHooks);
            }
            if Key::new("Удалить хуки").icon("trash").enabled(c.hooks_installed).show(ui).clicked() {
                cx.send(Command::UninstallHooks);
            }
            if Key::new("Скопировать JSON").icon("copy").show(ui).clicked() {
                ui.ctx().copy_text(c.snippet.clone());
                st.copied = Some(Instant::now());
            }
        });
        if st.copied.is_some() {
            w::para(ui, "JSON скопирован в буфер обмена", w::font(13.0), p.ok);
        } else if let Some((ok, msg)) = &c.hooks_message {
            w::para(ui, msg, w::font(13.0), if *ok { p.ok } else { p.danger });
        }
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 170.0), Sense::hover());
        w::well(ui.painter(), rect, 6.0, false);
        let inner = rect.shrink(8.0);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        egui::ScrollArea::both().id_salt("hooks-snippet").auto_shrink([false, false]).show(&mut child, |ui| {
            let mut text: &str = &c.snippet;
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .font(w::mono(11.0))
                    .text_color(p.text)
                    .desired_width(f32::INFINITY)
                    .code_editor(),
            );
        });
    });
}

pub fn show(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    scroll_page(ui, "claude-page", None, |ui| {
        let on = cx.snap.mode == DisplayMode::Claude;
        w::page_header(ui, "Claude", "статус Claude Code на экране колонки", |ui| {
            if w::switch(ui, on, "Показывать на колонке", true).clicked() {
                cx.send(Command::SetClaudeMode(!on));
            }
        });
        status_card(ui, cx);
        faces_group(ui, cx, st);
        sessions_group(ui, cx);
        hooks_group(ui, cx, st);
    });
}
