//! «Колонка» (§16.11): battery, screen and sound cards, the lock screen, built-in screens,
//! the notification card, what the device reports and (with `--debug`) raw commands.

use super::scroll_page;
use crate::api::{Builtin, Command, Conn};
use crate::ui::Cx;
use crate::ui::device_panel::Debounce;
use crate::ui::pixel::PixStyle;
use crate::ui::theme::pal;
use crate::ui::widgets::{self as w, Field, Key, Spin};
use egui::{Rect, Sense, Ui, pos2, vec2};

pub struct State {
    card_h: [f32; 2],
    volume: Debounce,
    away: Debounce,
    blue: i64,
    red: i64,
    app: usize,
    text: String,
    hex: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            card_h: [0.0; 2],
            volume: Debounce::default(),
            away: Debounce::default(),
            blue: 0,
            red: 0,
            app: 0,
            text: "Привет!".into(),
            hex: String::new(),
        }
    }
}

const APPS: [&str; 12] = ["Divoom", "Telegram", "WhatsApp", "Discord", "Instagram", "Facebook", "Messenger", "Twitter", "Skype", "VK", "WeChat", "TikTok"];

fn battery_card(ui: &mut Ui, cx: &mut Cx, min_h: f32) -> f32 {
    let d = &cx.snap.device;
    w::panel_measured(ui, [14.0, 14.0, 14.0, 17.0], None, min_h, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        w::plate(ui, "батарея");
        w::battery(ui, d.battery.map(|b| b.min(100)), 4);
        let t = if d.battery.is_some() {
            "Сообщает профиль Hands-Free, через BlueZ. Обновляется раз в 30 секунд."
        } else {
            "По протоколу приложения колонка заряд не сообщает. Его передаёт профиль Hands-Free, когда колонка подключена к компьютеру как аудиоустройство."
        };
        w::hint_small(ui, t, 12.0);
        if !d.audio_connected {
            let k = Key::new("Подключить как аудио").icon("bluetooth").tip("Колонка станет звуковым выходом компьютера (A2DP / Hands-Free)");
            if k.show(ui).clicked() {
                cx.send(Command::ConnectAudio);
            }
        }
    })
    .1
}

fn screen_card(ui: &mut Ui, cx: &mut Cx, min_h: f32) -> f32 {
    w::panel_measured(ui, [14.0, 14.0, 14.0, 17.0], None, min_h, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        w::plate(ui, "экран");
        let width = ui.available_width();
        w::row(ui, 33.0, 8.0, |ui| {
            let half = (width - 8.0) / 2.0;
            if Key::new("Выключить").icon("power").width(half).show(ui).clicked() {
                cx.send(Command::ScreenOnOff(false));
            }
            if Key::new("Включить").icon("eye").width(half).show(ui).clicked() {
                cx.send(Command::ScreenOnOff(true));
            }
        });
        let k = Key::new("Синхронизировать часы").icon("clock").width(width).tip("Установить на колонке время компьютера (для встроенных часов колонки)");
        if k.show(ui).clicked() {
            cx.send(Command::SyncTime);
        }
        w::hint_small(ui, "Яркость — ползунком под колонкой справа.", 12.0);
    })
    .1
}

fn sound_card(ui: &mut Ui, cx: &mut Cx, st: &mut State, min_h: f32) -> f32 {
    let p = pal();
    let d = &cx.snap.device;
    w::panel_measured(ui, [14.0, 14.0, 14.0, 17.0], None, min_h, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        w::plate(ui, "звук");
        let known = d.volume.is_some();
        let shown = st.volume.value(d.volume.unwrap_or(0) as f32);
        let label = if known { format!("{} / 15", shown.round() as i32) } else { "—".into() };
        let lw = PixStyle::new(12).measure("15 / 15").x;
        w::row(ui, 30.0, 10.0, |ui| {
            w::pixel_icon(ui, "volume", 2, p.text, None);
            let sw = ui.available_width() - lw - 10.0;
            let (_, v) = w::slider(ui, shown, 0.0, 15.0, 1.0, sw, known, "");
            if known && (v - shown).abs() > 0.01 {
                st.volume.moved(v);
            }
            w::pixel_text(ui, &label, PixStyle::new(12), p.text);
        });
        if let Some(v) = st.volume.poll(ui, 200) {
            cx.send(Command::SetVolume(v.round() as u8));
        }
        let audio = d.audio_connected;
        let playing = d.playing.unwrap_or(false);
        w::row(ui, 33.0, 8.0, |ui| {
            if Key::icon_only("prev").enabled(audio).tip("Предыдущий трек").show(ui).clicked() {
                cx.send(Command::PrevTrack);
            }
            let k = Key::icon_only(if playing { "pause" } else { "play" }).enabled(audio).tip(if playing { "Пауза" } else { "Играть" });
            if k.show(ui).clicked() {
                cx.send(Command::PlayPause);
            }
            if Key::icon_only("next").enabled(audio).tip("Следующий трек").show(ui).clicked() {
                cx.send(Command::NextTrack);
            }
            let rest = ui.available_width();
            w::para_w(ui, "плеер на устройстве, которое играет через колонку", w::font(12.0), p.text_dim, rest, egui::Align::Min);
        });
        let t = if audio {
            "Кнопки работают как клавиши на колонке: передают «играть / пауза / трек» плееру компьютера. Если плеер не запущен, ничего не произойдёт."
        } else {
            "Колонка сейчас не подключена как аудио, поэтому кнопкам плеера некому передавать команды."
        };
        w::hint_small(ui, t, 12.0);
    })
    .1
}

fn cards(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let width = ui.available_width();
    let wide = width + 40.0 >= 980.0;
    let gap = 16.0;
    let top = ui.cursor().top();
    let left = ui.cursor().left();
    let at = |ui: &mut Ui, rect: Rect| ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
    let mh = st.card_h[0];
    let (natural, total) = if wide {
        let unit = (width - 2.0 * gap) / 3.4;
        let (w1, w2, w3) = (unit, unit, unit * 1.4);
        let h1 = battery_card(&mut at(ui, Rect::from_min_size(pos2(left, top), vec2(w1, 2000.0))), cx, mh);
        let h2 = screen_card(&mut at(ui, Rect::from_min_size(pos2(left + w1 + gap, top), vec2(w2, 2000.0))), cx, mh);
        let h3 = sound_card(&mut at(ui, Rect::from_min_size(pos2(left + w1 + w2 + 2.0 * gap, top), vec2(w3, 2000.0))), cx, st, mh);
        let n = h1.max(h2).max(h3);
        (n, n)
    } else {
        let half = (width - gap) / 2.0;
        let h1 = battery_card(&mut at(ui, Rect::from_min_size(pos2(left, top), vec2(half, 2000.0))), cx, mh);
        let h2 = screen_card(&mut at(ui, Rect::from_min_size(pos2(left + half + gap, top), vec2(half, 2000.0))), cx, mh);
        let n = h1.max(h2);
        let h3 = sound_card(&mut at(ui, Rect::from_min_size(pos2(left, top + n.max(mh) + gap), vec2(width, 2000.0))), cx, st, 0.0);
        (n, n.max(mh) + gap + h3)
    };
    if (natural - st.card_h[0]).abs() > 0.5 {
        st.card_h[0] = natural;
        ui.ctx().request_repaint();
    }
    ui.allocate_rect(Rect::from_min_size(pos2(left, top), vec2(width, total.max(mh))), Sense::hover());
}

fn lock_group(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    let d = &cx.snap.device;
    w::group(ui, "Когда экран компьютера заблокирован", |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        if w::switch(ui, d.away_enabled, "Показывать часы и приглушать яркость", true).clicked() {
            cx.send(Command::SetAwayEnabled(!d.away_enabled));
        }
        let en = d.away_enabled;
        w::row(ui, 30.0, 10.0, |ui| {
            w::text(ui, "Яркость:", w::font(13.0), if en { p.text } else { p.text_disabled });
            let shown = st.away.value(d.away_brightness as f32);
            let sw = (ui.available_width() - 60.0).min(320.0);
            let (_, v) = w::slider(ui, shown, 0.0, 100.0, 5.0, sw, en, "");
            if (v - shown).abs() > 0.01 {
                st.away.moved(v);
            }
            w::pixel_text(ui, &format!("{}%", v.round() as i32), PixStyle::new(12), p.text);
        });
        if let Some(v) = st.away.poll(ui, 250) {
            cx.send(Command::SetAwayBrightness(v.round() as u8));
        }
        let t = format!(
            "{}Режим не меняется: после разблокировки колонка вернёт прежнюю картинку и яркость. Тревоги Claude и уведомления показываются и поверх часов.",
            if cx.snap.away { "Сейчас экран заблокирован. " } else { "" }
        );
        w::hint_small(ui, &t, 12.0);
    });
}

fn builtin_group(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    w::group(ui, "Встроенные экраны колонки", |ui| {
        ui.spacing_mut().item_spacing.y = 12.0;
        {
            let items = [
                ("Космонавт", "sparkle", Builtin::Cosmonaut),
                ("Галерея", "image", Builtin::Gallery),
                ("Шумомер", "wave", Builtin::NoiseMeter),
                ("Тетрис", "game", Builtin::Tetris),
                ("Игра 2", "game", Builtin::Game2),
                ("Игра 3", "game", Builtin::Game3),
                ("Выйти из игры", "close", Builtin::ExitGame),
            ];
            let keys = items.iter().map(|(t, icon, _)| Key::new(t).icon(icon)).collect();
            if let Some(i) = w::flow_keys(ui, None, keys, 8.0) {
                cx.send(Command::Builtin(items[i].2));
            }
        }
        w::row(ui, 33.0, 8.0, |ui| {
            w::text(ui, "Табло:", w::font(13.0), p.text);
            w::text(ui, "синие", w::font(13.0), p.info);
            if let Some(v) = Spin::new("score-blue", st.blue, 0, 999).show(ui) {
                st.blue = v;
            }
            w::text(ui, "красные", w::font(13.0), p.danger);
            if let Some(v) = Spin::new("score-red", st.red, 0, 999).show(ui) {
                st.red = v;
            }
            if Key::new("Показать").show(ui).clicked() {
                cx.send(Command::Scoreboard { red: st.red as u16, blue: st.blue as u16 });
            }
        });
        w::hint_small(ui, "Из встроенного экрана колонку выводит кнопка на ней самой или любая отправка из приложения.", 11.0);
    });
}

fn notice_group(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    w::group(ui, "Уведомление на колонке", |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        w::row(ui, 33.0, 8.0, |ui| {
            if let Some(i) = w::combo(ui, "notice-app", &APPS, st.app, 160.0) {
                st.app = i;
            }
            let send = Key::new("Отправить").icon("send");
            let icon = Key::new("Значок").icon("bell").tip("Встроенное уведомление колонки: только значок выбранного приложения");
            let kw = send.size(ui).x + icon.size(ui).x + 16.0;
            w::text_field(ui, &mut st.text, Field { hint: "Текст уведомления", width: ui.available_width() - kw, ..Default::default() });
            if send.show(ui).clicked() {
                cx.send(Command::DeviceNotifyCard { app: APPS[st.app].to_string(), text: st.text.clone() });
            }
            if icon.show(ui).clicked() {
                cx.send(Command::DeviceNotifyIcon { app: APPS[st.app].to_string() });
            }
        });
        w::hint_small(
            ui,
            &format!(
                "Встроенное уведомление колонки умеет показывать только значок, текст прошивка не выводит. Поэтому «Отправить» рисует карточку с текстом в приложении и держит её на экране {} с, как уведомления рабочего стола.",
                cx.snap.notify.duration
            ),
            11.0,
        );
    });
}

fn info_group(ui: &mut Ui, cx: &mut Cx) {
    let p = pal();
    let d = &cx.snap.device;
    w::group(ui, "Что сообщает колонка", |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let width = ui.available_width();
        let col_w = (width - 16.0) / 2.0;
        let value_w = col_w - 150.0 - 16.0;
        for pair in d.reported.chunks(2) {
            w::row(ui, 18.0, 0.0, |ui| {
                for (k, v) in pair {
                    let (r, _) = ui.allocate_exact_size(vec2(150.0 + 16.0, 18.0), Sense::hover());
                    let g = w::galley_elided(ui, k, w::font(13.0), p.text_dim, 150.0);
                    ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text_dim);
                    let (r, _) = ui.allocate_exact_size(vec2(value_w + if pair.len() == 2 { 16.0 } else { 0.0 }, 18.0), Sense::hover());
                    crate::ui::pixel::paint_text_in(ui.painter(), Rect::from_min_size(r.min, vec2(value_w, 18.0)), v, &PixStyle::new(12), p.text, egui::Align::Min);
                }
            });
        }
        if cx.debug {
            w::row(ui, 18.0, 0.0, |ui| {
                let (r, _) = ui.allocate_exact_size(vec2(166.0, 18.0), Sense::hover());
                let g = w::galley(ui, "Пульс колонки", w::font(13.0), p.text_dim);
                ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, p.text_dim);
                let hb = d.heartbeat.clone().unwrap_or_else(|| "—".into());
                w::text_elided(ui, &hb, w::mono(11.0), p.text, None);
            });
        }
    });
}

fn diag_group(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    w::group(ui, "Диагностика", |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let mut send = false;
        w::row(ui, 33.0, 8.0, |ui| {
            w::pixel_icon(ui, "terminal", 2, pal().text, None);
            let key = Key::new("Отправить");
            let kw = key.size(ui).x;
            let r = w::text_field(ui, &mut st.hex, Field { hint: "hex: <команда> <аргументы>, например 09", width: ui.available_width() - kw - 8.0, mono: true, ..Default::default() });
            if w::submitted(ui, &r) {
                send = true;
            }
            if key.show(ui).clicked() {
                send = true;
            }
        });
        if send && !st.hex.trim().is_empty() {
            cx.send(Command::RawHex(st.hex.trim().to_string()));
        }
        w::hint_small(
            ui,
            &format!(
                "Ответы — в журнале справа и на http://127.0.0.1:{}/device. Не отправляйте команды сна 0x40, 0xa3, 0xa4, 0xad, 0xae — они зависают колонку до выключения питания.",
                cx.snap.claude.port
            ),
            11.0,
        );
    });
}

pub fn show(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    scroll_page(ui, "device-page", None, |ui| {
        let connected = cx.snap.device.conn == Conn::Connected;
        let sub = if connected { "что умеет и что сообщает MiniToo" } else { "колонка не подключена — команды уйдут после подключения" };
        w::page_header(ui, "Колонка", sub, |_| {});
        cards(ui, cx, st);
        lock_group(ui, cx, st);
        builtin_group(ui, cx, st);
        notice_group(ui, cx, st);
        info_group(ui, cx);
        if cx.debug {
            diag_group(ui, cx, st);
        }
    });
}
