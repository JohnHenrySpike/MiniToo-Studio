//! Window title bar in the speaker style. The system frame is off, so moving the window,
//! double-click maximize, the window keys and the resize edges are all drawn here.

use super::pixel::{self, PixStyle};
use super::theme::{WHITE, darker, pal};
use super::widgets::{self as w};
use egui::{CursorIcon, Id, LayerId, Order, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Ui, Vec2, ViewportCommand, pos2, vec2};

pub const HEIGHT: f32 = 36.0;
const KEY: Vec2 = vec2(34.0, 24.0);
const KEY_GAP: f32 = 6.0;
const EDGE: f32 = 5.0;
const CORNER: f32 = 12.0;

#[derive(Clone, Copy)]
enum Action {
    Minimize,
    Maximize,
    Close,
}

fn maximized(ui: &Ui) -> bool {
    ui.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

/// The strip itself: drag to move, double-click to maximize, keys on the right.
pub fn show(ui: &mut Ui, title: &str) {
    let p = pal();
    let rect = ui.max_rect();
    let painter = ui.painter().clone();
    painter.rect_filled(Rect::from_min_max(pos2(rect.left(), rect.bottom() - 1.0), rect.max), 0, p.shell_lo);
    let max = maximized(ui);

    // the whole strip moves the window; the keys are added later and win the hit test
    let strip = ui.interact(rect, Id::new("titlebar-drag"), Sense::click_and_drag());
    if strip.double_clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!max));
    } else if strip.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }

    let st = PixStyle::new(9);
    let size = st.measure(title);
    pixel::paint_text(&painter, pos2((rect.center().x - size.x / 2.0).round(), (rect.center().y - size.y / 2.0).round()), title, &st, p.text_dim);

    let keys = [
        ("close", tr!("titlebar.close"), Action::Close),
        if max { ("win-restore", tr!("titlebar.restore"), Action::Maximize) } else { ("win-max", tr!("titlebar.maximize"), Action::Maximize) },
        ("win-min", tr!("titlebar.minimize"), Action::Minimize),
    ];
    let y = (rect.center().y - KEY.y / 2.0 + 1.0).round();
    let mut x = rect.right() - 10.0 - KEY.x;
    for (icon, tip, action) in keys {
        let r = Rect::from_min_size(pos2(x, y), KEY);
        x -= KEY.x + KEY_GAP;
        if window_key(ui, r, icon, matches!(action, Action::Close), tip).clicked() {
            let cmd = match action {
                Action::Minimize => ViewportCommand::Minimized(true),
                Action::Maximize => ViewportCommand::Maximized(!max),
                Action::Close => ViewportCommand::Close,
            };
            ui.ctx().send_viewport_cmd(cmd);
        }
    }
}

/// Small plastic key; the close key turns red under the cursor.
fn window_key(ui: &mut Ui, rect: Rect, icon: &str, close: bool, tip: &str) -> egui::Response {
    let p = pal();
    let resp = ui.interact(rect, Id::new(("titlebar-key", icon)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let hovered = resp.hovered();
    let down = resp.is_pointer_button_down_on();
    let red = close && (hovered || down);
    let (face, edge) = if red {
        (p.danger, darker(p.danger, 1.35))
    } else if hovered {
        (p.key_hi, p.key_edge)
    } else {
        (p.key, p.key_edge)
    };
    let painter = ui.painter();
    w::paint_plastic(painter, rect, face, edge, None, 2.0, 5.0, down);
    let cy = rect.top() + (rect.height() - 2.0) / 2.0 + if down { 1.0 } else { 0.0 };
    pixel::paint_icon(painter, pos2(rect.center().x - 6.0, (cy - 6.0).round()), icon, 1, if red { WHITE } else { p.key_text }, None);
    w::tip(resp, tip)
}

/// Resize zones along the window edges and a 1 px outline; call after everything else so the
/// zones sit on top. Nothing while maximized.
pub fn edges(ui: &mut Ui) {
    if maximized(ui) {
        return;
    }
    let r = ui.ctx().content_rect();
    let p = pal();
    ui.ctx()
        .layer_painter(LayerId::new(Order::Foreground, Id::new("window-outline")))
        .rect_stroke(r, 0, Stroke::new(1.0, p.shell_lo), StrokeKind::Inside);

    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    use ResizeDirection as D;
    // sides first, corners last so the corners win where they overlap
    let zones = [
        (Rect::from_min_max(pos2(l, t), pos2(rt, t + EDGE)), D::North, CursorIcon::ResizeNorth),
        (Rect::from_min_max(pos2(l, b - EDGE), pos2(rt, b)), D::South, CursorIcon::ResizeSouth),
        (Rect::from_min_max(pos2(l, t), pos2(l + EDGE, b)), D::West, CursorIcon::ResizeWest),
        (Rect::from_min_max(pos2(rt - EDGE, t), pos2(rt, b)), D::East, CursorIcon::ResizeEast),
        (Rect::from_min_max(pos2(l, t), pos2(l + CORNER, t + CORNER)), D::NorthWest, CursorIcon::ResizeNorthWest),
        (Rect::from_min_max(pos2(rt - CORNER, t), pos2(rt, t + CORNER)), D::NorthEast, CursorIcon::ResizeNorthEast),
        (Rect::from_min_max(pos2(l, b - CORNER), pos2(l + CORNER, b)), D::SouthWest, CursorIcon::ResizeSouthWest),
        (Rect::from_min_max(pos2(rt - CORNER, b - CORNER), pos2(rt, b)), D::SouthEast, CursorIcon::ResizeSouthEast),
    ];
    let pressed = ui.input(|i| i.pointer.primary_pressed());
    for (i, (zone, dir, cursor)) in zones.into_iter().enumerate() {
        let resp = ui.interact(zone, Id::new(("window-edge", i)), Sense::drag()).on_hover_cursor(cursor);
        if pressed && resp.hovered() {
            ui.ctx().send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}
