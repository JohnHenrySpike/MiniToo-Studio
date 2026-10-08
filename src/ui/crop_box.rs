//! CropBox (§16.7): an aspect-locked selection over a displayed picture, in normalised
//! coordinates of that picture.

use super::theme::{argb, pal};
use crate::api::NRect;
use egui::{Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use std::time::{Duration, Instant};

pub enum CropEvent {
    Edited(NRect),
    Reset,
}

#[derive(Clone, Copy)]
enum Grab {
    Move { start: Pos2, rect: Rect },
    Corner { anchor: Pos2, right: bool, bottom: bool },
}

#[derive(Clone, Copy)]
struct Local {
    grab: Option<Grab>,
    /// what we last sent, shown until the snapshot catches up
    edited: Option<(NRect, Instant)>,
}

const ASPECT: f32 = 160.0 / 128.0;

fn emit_px(area: Vec2, x: f32, y: f32, w: f32) -> NRect {
    let mut w = w.max(16.0).min(area.x);
    let mut h = w / ASPECT;
    if h > area.y {
        h = area.y;
        w = h * ASPECT;
    }
    let x = x.clamp(0.0, (area.x - w).max(0.0));
    let y = y.clamp(0.0, (area.y - h).max(0.0));
    NRect { x: (x / area.x) as f64, y: (y / area.y) as f64, w: (w / area.x) as f64, h: (h / area.y) as f64 }
}

/// `img` is where the picture is painted; `crop` the current normalised rectangle.
pub fn crop_box(ui: &mut Ui, id: Id, img: Rect, crop: NRect) -> Option<CropEvent> {
    if img.width() < 4.0 || img.height() < 4.0 {
        return None;
    }
    let mut st: Local = ui.data(|d| d.get_temp(id)).unwrap_or(Local { grab: None, edited: None });
    let shown = match st.edited {
        Some((r, at)) if at.elapsed() < Duration::from_millis(900) && r != crop => r,
        _ => crop,
    };
    let area = img.size();
    let sel = Rect::from_min_size(
        img.min + vec2(shown.x as f32 * area.x, shown.y as f32 * area.y),
        vec2(shown.w as f32 * area.x, shown.h as f32 * area.y),
    );
    let mut out = None;

    // body: move, wheel zoom, double-click reset
    let body = ui.interact(sel, id.with("body"), Sense::click_and_drag());
    let cursor = if body.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab };
    let body = body.on_hover_cursor(cursor);
    if body.drag_started() {
        if let Some(p) = body.interact_pointer_pos() {
            st.grab = Some(Grab::Move { start: p, rect: sel });
        }
    }
    if body.dragged() {
        if let (Some(Grab::Move { start, rect }), Some(p)) = (st.grab, body.interact_pointer_pos()) {
            let d = p - start;
            out = Some(CropEvent::Edited(emit_px(area, rect.left() - img.left() + d.x, rect.top() - img.top() + d.y, rect.width())));
        }
    }
    if body.double_clicked() {
        out = Some(CropEvent::Reset);
        st.edited = None;
    }
    if body.hovered() {
        let dy = super::widgets::wheel_delta(ui).y;
        if dy != 0.0 {
            let f = if dy > 0.0 { 0.92 } else { 1.08 };
            let c = sel.center() - img.min;
            let nw = sel.width() * f;
            out = Some(CropEvent::Edited(emit_px(area, c.x - nw / 2.0, c.y - nw / ASPECT / 2.0, nw)));
        }
    }

    // corner handles
    let corners = [(false, false), (true, false), (false, true), (true, true)];
    for (i, (right, bottom)) in corners.into_iter().enumerate() {
        let c = pos2(if right { sel.right() } else { sel.left() }, if bottom { sel.bottom() } else { sel.top() });
        let hit = Rect::from_center_size(c, Vec2::splat(30.0));
        let resp = ui.interact(hit, id.with(("corner", i)), Sense::drag());
        let resp = resp.on_hover_cursor(if right != bottom { CursorIcon::ResizeNeSw } else { CursorIcon::ResizeNwSe });
        if resp.drag_started() {
            let anchor = pos2(if right { sel.left() } else { sel.right() }, if bottom { sel.top() } else { sel.bottom() }) - img.min.to_vec2();
            st.grab = Some(Grab::Corner { anchor, right, bottom });
        }
        if resp.dragged() {
            if let (Some(Grab::Corner { anchor, right, bottom }), Some(p)) = (st.grab, resp.interact_pointer_pos()) {
                let p = p - img.min.to_vec2();
                let mut w = (p.x - anchor.x).abs().max((p.y - anchor.y).abs() * ASPECT);
                let max_w = if right { area.x - anchor.x } else { anchor.x };
                let max_h = if bottom { area.y - anchor.y } else { anchor.y };
                w = w.min(max_w).min(max_h * ASPECT).max(16.0);
                let h = w / ASPECT;
                let x = if right { anchor.x } else { anchor.x - w };
                let y = if bottom { anchor.y } else { anchor.y - h };
                out = Some(CropEvent::Edited(emit_px(area, x, y, w)));
            }
        }
    }
    if !ui.input(|i| i.pointer.any_down()) {
        st.grab = None;
    }

    if let Some(CropEvent::Edited(r)) = &out {
        st.edited = Some((*r, Instant::now()));
    }
    ui.data_mut(|d| d.insert_temp(id, st));

    // paint
    let painter = ui.painter();
    let dim = argb(0xa0000000);
    painter.rect_filled(Rect::from_min_max(img.min, pos2(img.right(), sel.top())), 0, dim);
    painter.rect_filled(Rect::from_min_max(pos2(img.left(), sel.bottom()), img.max), 0, dim);
    painter.rect_filled(Rect::from_min_max(pos2(img.left(), sel.top()), pos2(sel.left(), sel.bottom())), 0, dim);
    painter.rect_filled(Rect::from_min_max(pos2(sel.right(), sel.top()), pos2(img.right(), sel.bottom())), 0, dim);
    let third = argb(0x60ffffff);
    for k in 1..3 {
        let x = sel.left() + sel.width() * k as f32 / 3.0;
        painter.rect_filled(Rect::from_min_size(pos2(x, sel.top()), vec2(1.0, sel.height())), 0, third);
        let y = sel.top() + sel.height() * k as f32 / 3.0;
        painter.rect_filled(Rect::from_min_size(pos2(sel.left(), y), vec2(sel.width(), 1.0)), 0, third);
    }
    painter.rect_stroke(sel, 0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
    for (right, bottom) in corners {
        let c = pos2(if right { sel.right() } else { sel.left() }, if bottom { sel.bottom() } else { sel.top() });
        painter.circle(c, 5.5, Color32::WHITE, Stroke::new(3.0, pal().accent));
    }
    out
}
