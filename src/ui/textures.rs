//! Textures for device frames and images: uploaded only when the content changes
//! (frames are compared by their shared buffer, images by `Arc` identity) and dropped when
//! not drawn for a while.

use crate::api::Anim;
use crate::frame::{Frame, HEIGHT, WIDTH};
use egui::{TextureHandle, TextureId, TextureOptions};
use image::RgbaImage;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

struct FrameTex {
    frame: Frame,
    smooth: bool,
    tex: TextureHandle,
    used: u64,
}

struct ImageTex {
    image: Arc<RgbaImage>,
    smooth: bool,
    tex: TextureHandle,
    used: u64,
}

#[derive(Default)]
pub struct Textures {
    frames: HashMap<String, FrameTex>,
    images: HashMap<String, ImageTex>,
    pass: u64,
}

pub fn options(smooth: bool) -> TextureOptions {
    if smooth {
        TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR }
    } else {
        TextureOptions::NEAREST
    }
}

fn frame_image(frame: &Frame) -> egui::ColorImage {
    egui::ColorImage::from_rgb([WIDTH, HEIGHT], frame.rgb())
}

fn rgba_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

impl Textures {
    /// Texture for a 160×128 frame under `key`; re-uploaded only when the frame changed.
    pub fn frame(&mut self, ctx: &egui::Context, key: &str, frame: &Frame, smooth: bool) -> TextureId {
        let pass = self.pass;
        if let Some(e) = self.frames.get_mut(key) {
            if e.frame != *frame || e.smooth != smooth {
                e.tex.set(frame_image(frame), options(smooth));
                e.frame = frame.clone();
                e.smooth = smooth;
            }
            e.used = pass;
            return e.tex.id();
        }
        let tex = ctx.load_texture(key, frame_image(frame), options(smooth));
        let id = tex.id();
        self.frames.insert(key.to_string(), FrameTex { frame: frame.clone(), smooth, tex, used: pass });
        id
    }

    pub fn image(&mut self, ctx: &egui::Context, key: &str, img: &Arc<RgbaImage>, smooth: bool) -> TextureId {
        let pass = self.pass;
        if let Some(e) = self.images.get_mut(key) {
            if !Arc::ptr_eq(&e.image, img) || e.smooth != smooth {
                e.tex.set(rgba_image(img), options(smooth));
                e.image = img.clone();
                e.smooth = smooth;
            }
            e.used = pass;
            return e.tex.id();
        }
        let tex = ctx.load_texture(key, rgba_image(img), options(smooth));
        let id = tex.id();
        self.images.insert(key.to_string(), ImageTex { image: img.clone(), smooth, tex, used: pass });
        id
    }

    /// Call once per pass: forgets textures not drawn for ~10 s worth of passes.
    pub fn end_pass(&mut self) {
        self.pass += 1;
        let keep = self.pass.saturating_sub(600);
        self.frames.retain(|_, e| e.used >= keep);
        self.images.retain(|_, e| e.used >= keep);
    }
}

/// Index of the frame to show now for an animation played with `speed` ms per frame, and
/// schedules the next repaint.
pub fn anim_index(ctx: &egui::Context, count: usize, speed: u32) -> usize {
    if count <= 1 {
        return 0;
    }
    let speed = speed.max(30) as f64;
    let ms = ctx.input(|i| i.time) * 1000.0;
    let idx = (ms / speed) as usize % count;
    let next = speed - ms % speed;
    ctx.request_repaint_after(Duration::from_millis(next.ceil() as u64 + 1));
    idx
}

/// The current frame of an animation (None when empty).
pub fn anim_frame<'a>(ctx: &egui::Context, anim: &'a Anim) -> Option<(usize, &'a Frame)> {
    if anim.frames.is_empty() {
        return None;
    }
    let i = anim_index(ctx, anim.frames.len(), anim.speed);
    Some((i, &anim.frames[i]))
}
