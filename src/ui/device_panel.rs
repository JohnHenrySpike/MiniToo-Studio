//! Right panel (§16.6): the drawn MiniToo whose screen mirrors the device and whose keys and
//! knob work, the connection line, what is on the screen, brightness, volume and the log.

use super::pixel::{self, PixStyle};
use super::textures;
use super::theme::{WHITE, hex, pal};
use super::widgets::{self as w, Key};
use super::Cx;
use crate::api::{Command, Conn};
use egui::{Align, Color32, Layout, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use std::time::{Duration, Instant};

/// A slider value that is sent only after the pointer rests for `delay`.
#[derive(Default)]
pub struct Debounce {
    pending: Option<(f32, Instant)>,
    sent: Option<(f32, Instant)>,
}

impl Debounce {
    pub fn value(&self, actual: f32) -> f32 {
        if let Some((v, _)) = self.pending {
            return v;
        }
        match self.sent {
            Some((v, at)) if at.elapsed() < Duration::from_millis(1500) && (v - actual).abs() > 0.01 => v,
            _ => actual,
        }
    }

    pub fn moved(&mut self, v: f32) {
        self.pending = Some((v, Instant::now()));
    }

    /// The value to send now, if it has rested long enough.
    pub fn poll(&mut self, ui: &Ui, delay_ms: u64) -> Option<f32> {
        let (v, at) = self.pending?;
        let delay = Duration::from_millis(delay_ms);
        if at.elapsed() >= delay {
            self.pending = None;
            self.sent = Some((v, Instant::now()));
            Some(v)
        } else {
            ui.ctx().request_repaint_after(delay - at.elapsed());
            None
        }
    }
}

#[derive(Default)]
pub struct State {
    pub brightness: Debounce,
    pub volume: Debounce,
    log_open: bool,
}

const KEY_FACE: Color32 = hex(0xdcd8d1);
const KEY_FACE_HI: Color32 = hex(0xebe7e1);
const KEY_EDGE: Color32 = hex(0x9c958a);
const KEY_HL: Color32 = hex(0xf4f1ec);
const KEY_ICON: Color32 = hex(0x59534b);

fn device_key(ui: &mut Ui, rect: Rect, icon: &str, enabled: bool, tip: &str) -> Response {
    let id = ui.id().with(("devkey", icon, rect.min.x as i32));
    let resp = ui.interact(rect, id, if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let down = enabled && resp.is_pointer_button_down_on();
    let op = if enabled { 1.0 } else { 0.6 };
    let painter = ui.painter();
    w::paint_plastic(painter, rect, (if hovered { KEY_FACE_HI } else { KEY_FACE }).gamma_multiply(op), KEY_EDGE.gamma_multiply(op), Some(KEY_HL), 4.0, 4.0, down);
    let cy = rect.center().y + if down { 1.0 } else { -1.0 };
    pixel::paint_icon(painter, pos2(rect.center().x - 6.0, cy - 6.0), icon, 1, KEY_ICON.gamma_multiply(op), None);
    w::tip_disabled(resp, tip)
}

/// The MiniToo drawing: 312 × 360 at `origin`.
fn mock(ui: &mut Ui, cx: &mut Cx, origin: egui::Pos2) {
    let p = pal();
    let d = &cx.snap.device;
    let connected = d.conn == Conn::Connected;
    let painter = ui.painter().clone();
    let mon = Rect::from_min_size(origin + vec2(14.0, 0.0), vec2(284.0, 282.0));
    let base = Rect::from_min_size(origin + vec2(0.0, 276.0), vec2(312.0, 84.0));

    // keyboard base first: the monitor sits on it
    w::paint_plastic(&painter, base, p.body, p.body_edge, Some(p.body_hi), 9.0, 12.0, false);
    // a hint of the CRT's back casing
    painter.rect_filled(Rect::from_min_size(pos2(mon.center().x - 112.0, mon.top() - 4.0), vec2(224.0, 8.0)), 4, p.body_lo);
    w::paint_plastic(&painter, mon, p.body, p.body_edge, Some(p.body_hi), 5.0, 18.0, false);

    // screen
    let (_, ssize) = w::screen_size(1.5);
    let sf_min = pos2(mon.center().x - ssize.x / 2.0, mon.top() + 16.0);
    let glass = w::paint_screen_frame(&painter, sf_min, 1.5, false, true);
    let mirror = &cx.snap.mirror;
    if !mirror.frames.is_empty() {
        if d.screen_on
            && let Some((i, f)) = textures::anim_frame(ui.ctx(), mirror)
        {
            let tex = cx.tex.frame(ui.ctx(), &format!("mirror/{i}"), f, false);
            w::paint_texture(&painter, glass, tex, WHITE);
        }
    } else {
        let big = PixStyle::new(11).zoom(2);
        let small = PixStyle::new(9);
        let s1 = big.measure("MINITOO");
        let placeholder = tr!("panel.screen_placeholder");
        let s2 = small.measure(placeholder);
        let h = s1.y + 6.0 + s2.y;
        let top = glass.center().y - h / 2.0;
        pixel::paint_text(&painter, pos2(glass.center().x - s1.x / 2.0, top), "MINITOO", &big, hex(0x3a3f4c));
        pixel::paint_text(&painter, pos2(glass.center().x - s2.x / 2.0, top + s1.y + 6.0), placeholder, &small, hex(0x5a6070));
    }
    let frame_bottom = sf_min.y + ssize.y;
    w::paint_plate(&painter, pos2(sf_min.x, frame_bottom + 12.0), "minitoo", hex(0x4a4744), hex(0xe9e0d1));

    // the small orange key: screen on / off
    let skey = Rect::from_min_size(pos2(sf_min.x + ssize.x - 46.0, frame_bottom + 9.0), vec2(46.0, 22.0));
    let sresp = ui.interact(skey, ui.id().with("screen-key"), if connected { Sense::click() } else { Sense::hover() });
    let hov = connected && sresp.hovered();
    let down = connected && sresp.is_pointer_button_down_on();
    let op = if connected { 1.0 } else { 0.6 };
    w::paint_plastic(&painter, skey, (if hov { p.accent_hi } else { p.accent }).gamma_multiply(op), p.accent_edge.gamma_multiply(op), None, 4.0, 3.0, down);
    let cy = skey.center().y + if down { 1.0 } else { -1.0 };
    pixel::paint_icon(&painter, pos2(skey.center().x - 6.0, cy - 6.0), if d.screen_on { "eye" } else { "power" }, 1, WHITE, None);
    let sresp = w::tip_disabled(sresp, if d.screen_on { tr!("panel.screen_off") } else { tr!("panel.screen_on") });
    if sresp.clicked() {
        cx.send(Command::ScreenOnOff(!d.screen_on));
    }

    // keys
    let vol = d.volume.unwrap_or(0);
    let playing = d.playing.unwrap_or(false);
    let keys: [(&str, &str, Command); 5] = [
        ("volume-down", tr!("panel.volume_down"), Command::SetVolume(vol.saturating_sub(1))),
        ("volume-up", tr!("panel.volume_up"), Command::SetVolume((vol + 1).min(15))),
        ("prev", tr!("panel.prev_track"), Command::PrevTrack),
        (if playing { "pause" } else { "play" }, tr!("panel.play_pause"), Command::PlayPause),
        ("next", tr!("panel.next_track"), Command::NextTrack),
    ];
    for (i, (icon, tip, cmd)) in keys.into_iter().enumerate() {
        let r = Rect::from_min_size(base.min + vec2(18.0 + i as f32 * 40.0, 18.0), vec2(34.0, 26.0));
        if device_key(ui, r, icon, connected, tip).clicked() {
            cx.send(cmd);
        }
    }

    // the round red knob: connect / disconnect
    let knob = Rect::from_min_size(base.min + vec2(312.0 - 38.0 - 22.0, 13.0), vec2(38.0, 38.0));
    let kresp = ui.interact(knob, ui.id().with("knob"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let c = knob.center();
    painter.circle_filled(c, 19.0, p.body_lo);
    let ring = match d.conn {
        Conn::Connected => Some((p.ok, 1.0)),
        Conn::Connecting => Some((p.warn, w::blink_opacity(ui, 0.4, 0.2))),
        Conn::Disconnected => None,
    };
    if let Some((col, op)) = ring {
        painter.circle_stroke(c, 15.0, Stroke::new(2.0, col.gamma_multiply(op)));
    }
    let kdown = kresp.is_pointer_button_down_on();
    let dc = c + vec2(0.0, if kdown { 1.0 } else { 0.0 });
    painter.circle_filled(dc, 12.0, if kresp.hovered() { hex(0xf25a44) } else { hex(0xe04a35) });
    painter.circle_stroke(dc, 11.0, Stroke::new(2.0, hex(0xa8301f)));
    let hl = Rect::from_min_size(dc + vec2(-6.0, -8.0), vec2(8.0, 4.0));
    painter.rect_filled(hl, 2, hex(0xff9a86).gamma_multiply(0.8));
    let kresp = w::tip(kresp, if d.conn == Conn::Disconnected { tr!("panel.connect") } else { tr!("panel.disconnect") });
    if kresp.clicked() {
        cx.send(Command::Connect(d.conn == Conn::Disconnected));
    }

    let label = "· PIXEL DISPLAY 160×128 ·";
    let st = PixStyle::new(7).regular();
    let lw = st.measure(label).x;
    pixel::paint_text(&painter, pos2(base.center().x - 24.0 - lw / 2.0, base.top() + 54.0), label, &st, hex(0x9a8f7c));
}

pub fn show(ui: &mut Ui, cx: &mut Cx, st: &mut State) {
    let p = pal();
    let full = ui.max_rect();
    let area = Rect::from_min_max(full.min + vec2(17.0, 22.0), full.max - vec2(16.0, 16.0));
    let mut ui = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Min)));
    let ui = &mut ui;
    ui.spacing_mut().item_spacing = vec2(8.0, 12.0);
    let d = cx.snap.device.clone();

    let (mrect, _) = ui.allocate_exact_size(vec2(area.width(), 360.0 + 6.0), Sense::hover());
    mock(ui, cx, pos2(mrect.center().x - 156.0, mrect.top()));

    // connection and battery
    w::row(ui, 22.0, 8.0, |ui| {
        let (color, text) = match d.conn {
            Conn::Disconnected => (p.text_disabled, tr!("panel.conn.disconnected")),
            Conn::Connecting => (p.warn, tr!("panel.conn.connecting")),
            Conn::Connected => (p.ok, tr!("panel.conn.connected")),
        };
        w::led(ui, color, d.conn != Conn::Disconnected, d.conn == Conn::Connecting);
        w::pixel_text(ui, text, PixStyle::new(12), p.text);
        if d.busy {
            w::busy(ui);
        }
        if d.battery.is_some() {
            w::right(ui, |ui| w::battery(ui, d.battery.map(|b| b.min(100)), 2));
        }
    });

    let on_screen = if cx.snap.interrupted {
        tr!("panel.on_screen_alert", what = cx.snap.on_screen)
    } else {
        tr!("panel.on_screen", what = cx.snap.on_screen)
    };
    w::text_elided(ui, &on_screen, w::font(13.0), p.text_dim, None);
    if !cx.snap.last_transfer.is_empty() {
        ui.add_space(-8.0);
        w::text_elided(ui, &cx.snap.last_transfer, w::font(11.0), p.text_dim, None);
    }

    // brightness and volume
    ui.spacing_mut().item_spacing.y = 2.0;
    let slider_w = ui.available_width() - 12.0 - 8.0 - 34.0 - 8.0;
    w::row(ui, 30.0, 8.0, |ui| {
        w::pixel_icon(ui, "sun", 1, p.text_dim, None);
        let shown = st.brightness.value(d.brightness as f32);
        let (_, v) = w::slider(ui, shown, 0.0, 100.0, 5.0, slider_w, true, tr!("panel.brightness"));
        if (v - shown).abs() > 0.01 {
            st.brightness.moved(v);
        }
        w::pixel_text(ui, &format!("{}%", v.round() as i32), PixStyle::new(11), p.text);
    });
    if let Some(v) = st.brightness.poll(ui, 250) {
        cx.send(Command::SetBrightness(v.round() as u8));
    }
    w::row(ui, 30.0, 8.0, |ui| {
        w::pixel_icon(ui, "volume", 1, p.text_dim, None);
        let known = d.volume.is_some();
        let shown = st.volume.value(d.volume.unwrap_or(0) as f32);
        let (_, v) = w::slider(ui, shown, 0.0, 15.0, 1.0, slider_w, known, tr!("panel.volume"));
        if known && (v - shown).abs() > 0.01 {
            st.volume.moved(v);
        }
        let label = if known { format!("{}/15", v.round() as i32) } else { "—".into() };
        w::pixel_text(ui, &label, PixStyle::new(11), p.text);
    });
    if let Some(v) = st.volume.poll(ui, 200) {
        cx.send(Command::SetVolume(v.round() as u8));
    }
    ui.spacing_mut().item_spacing.y = 12.0;

    // log (--debug)
    if cx.debug {
        ui.add_space(2.0);
        let n = cx.snap.log.len();
        let label = match (st.log_open, n > 0) {
            (true, true) => tr!("panel.log_hide_count", count = n),
            (true, false) => tr!("panel.log_hide").to_string(),
            (false, true) => tr!("panel.log_show_count", count = n),
            (false, false) => tr!("panel.log_show").to_string(),
        };
        let key = Key::new(&label).flat().icon(if st.log_open { "down" } else { "up" }).width(ui.available_width());
        if key.show(ui).clicked() {
            st.log_open = !st.log_open;
        }
        if st.log_open {
            let rect = ui.available_rect_before_wrap();
            let rect = Rect::from_min_size(rect.min, vec2(rect.width(), rect.height().max(60.0)));
            ui.allocate_rect(rect, Sense::hover());
            w::well(ui.painter(), rect, 6.0, false);
            let inner = rect.shrink(6.0);
            let mut lu = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(&mut lu, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let start = cx.snap.log.len().saturating_sub(200);
                for line in &cx.snap.log[start..] {
                    w::para(ui, line, w::mono(11.0), p.text.gamma_multiply(0.85));
                }
            });
            let tr = Rect::from_min_size(pos2(rect.right() - 34.0, rect.top() + 4.0), vec2(30.0, 30.0));
            if Key::icon_only("trash").flat().tip(tr!("panel.log_clear")).show_at(ui, tr).clicked() {
                cx.send(Command::ClearLog);
            }
        }
    }
}
