//! Loading pictures and animations (§6.1) and rendering them into 160×128 (§6.2).

use crate::api::{Fit, NRect};
use crate::frame::{Frame, HEIGHT, WIDTH};
use crate::protocol::MAX_FRAMES;
use image::imageops::FilterType;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, RgbaImage};
use std::io::BufRead;
use std::path::Path;
use std::sync::Arc;

pub const MAX_LOAD_FRAMES: usize = 600;
/// Total decoded pixels kept in memory; larger animations are downscaled on load.
const MAX_TOTAL_PIXELS: u64 = 160_000_000;

pub const IMAGE_SUFFIXES: [&str; 13] =
    ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "avif", "jxl", "heic", "heif", "tif", "tiff"];

pub fn is_image_file(path: &Path) -> bool {
    path.extension()
        .map(|e| IMAGE_SUFFIXES.contains(&e.to_string_lossy().to_lowercase().as_str()))
        .unwrap_or(false)
}

#[derive(Clone, Debug, Default)]
pub struct Animation {
    pub frames: Vec<Arc<RgbaImage>>,
    /// per-frame delay in ms
    pub delays: Vec<u32>,
    /// original size (before any memory-saving downscale)
    pub width: u32,
    pub height: u32,
}

impl Animation {
    /// `max(20, sum / count)`
    pub fn average_delay(&self) -> u32 {
        if self.delays.is_empty() {
            return 1000;
        }
        let sum: u64 = self.delays.iter().map(|&d| d as u64).sum();
        ((sum / self.delays.len() as u64) as u32).max(20)
    }

    fn finish(mut self) -> Result<Animation, String> {
        if self.frames.is_empty() {
            return Err("не удалось прочитать изображение".into());
        }
        if self.width == 0 {
            self.width = self.frames[0].width();
            self.height = self.frames[0].height();
        }
        if self.frames.len() == 1 {
            self.delays = vec![1000];
        }
        for d in self.delays.iter_mut() {
            if *d == 0 {
                *d = 100;
            }
        }
        // keep memory bounded: huge animations are scaled down (the device shows 160×128)
        let total: u64 = self.frames.iter().map(|f| f.width() as u64 * f.height() as u64).sum();
        if total > MAX_TOTAL_PIXELS {
            let k = (MAX_TOTAL_PIXELS as f64 / total as f64).sqrt();
            for f in self.frames.iter_mut() {
                let w = ((f.width() as f64 * k) as u32).max(WIDTH as u32);
                let h = ((f.height() as f64 * k) as u32).max(HEIGHT as u32);
                *f = Arc::new(image::imageops::resize(f.as_ref(), w, h, FilterType::Triangle));
            }
        }
        Ok(self)
    }
}

fn single(img: DynamicImage) -> Animation {
    let rgba = img.into_rgba8();
    Animation { width: rgba.width(), height: rgba.height(), frames: vec![Arc::new(rgba)], delays: vec![1000] }
}

fn from_frames(frames: image::Frames<'_>, max_frames: usize) -> Result<Animation, String> {
    let mut anim = Animation::default();
    for f in frames.take(max_frames) {
        let f = f.map_err(|e| e.to_string())?;
        let (num, den) = f.delay().numer_denom_ms();
        let ms = if den == 0 { 0 } else { num / den };
        anim.delays.push(ms);
        anim.frames.push(Arc::new(f.into_buffer()));
    }
    Ok(anim)
}

/// Loads a picture or an animation (up to `max_frames`), honouring EXIF orientation. The format
/// is detected from the content.
pub fn load(path: &Path, max_frames: usize) -> Result<Animation, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    load_bytes(&bytes, path, max_frames)
}

pub fn load_bytes(bytes: &[u8], path: &Path, max_frames: usize) -> Result<Animation, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let head = &bytes[..bytes.len().min(512)];
    let looks_svg = ext == "svg" || ext == "svgz" || {
        let s = String::from_utf8_lossy(head);
        s.contains("<svg") || (s.trim_start().starts_with("<?xml") && s.contains("svg"))
    };
    if looks_svg {
        return load_svg(bytes).and_then(Animation::finish);
    }
    if bytes.starts_with(&[0xff, 0x0a]) || bytes.starts_with(&[0, 0, 0, 0x0c, b'J', b'X', b'L', b' ']) {
        return load_jxl(bytes, max_frames).and_then(Animation::finish);
    }
    match load_raster(bytes, max_frames) {
        Ok(a) => a.finish(),
        Err(native) => match load_external(path) {
            Ok(a) => a.finish(),
            Err(_) => Err(native),
        },
    }
}

fn load_raster(bytes: &[u8], max_frames: usize) -> Result<Animation, String> {
    use image::ImageFormat;
    let cursor = || std::io::Cursor::new(bytes);
    let format = image::guess_format(bytes).map_err(|_| "формат не поддерживается".to_string())?;
    match format {
        ImageFormat::Gif => {
            let d = image::codecs::gif::GifDecoder::new(cursor()).map_err(|e| e.to_string())?;
            from_frames(d.into_frames(), max_frames)
        }
        ImageFormat::WebP => {
            let d = image::codecs::webp::WebPDecoder::new(cursor()).map_err(|e| e.to_string())?;
            if d.has_animation() {
                from_frames(d.into_frames(), max_frames)
            } else {
                Ok(single(DynamicImage::from_decoder(d).map_err(|e| e.to_string())?))
            }
        }
        ImageFormat::Png => {
            let d = image::codecs::png::PngDecoder::new(cursor()).map_err(|e| e.to_string())?;
            if d.is_apng().unwrap_or(false) {
                let a = d.apng().map_err(|e| e.to_string())?;
                from_frames(a.into_frames(), max_frames)
            } else {
                Ok(single(DynamicImage::from_decoder(d).map_err(|e| e.to_string())?))
            }
        }
        _ => {
            let reader = image::ImageReader::with_format(std::io::BufReader::new(cursor()), format);
            let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
            let orientation = decoder.orientation().ok();
            let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
            if let Some(o) = orientation {
                img.apply_orientation(o);
            }
            Ok(single(img))
        }
    }
}

fn load_svg(bytes: &[u8]) -> Result<Animation, String> {
    use resvg::{tiny_skia, usvg};
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(bytes, &opt).map_err(|e| format!("SVG: {e}"))?;
    let size = tree.size();
    // render at a useful resolution: at least the device size, at most 1024 px
    let scale = (1024.0 / size.width().max(size.height())).min(8.0).max(
        (WIDTH as f32 / size.width()).max(HEIGHT as f32 / size.height()),
    );
    let w = (size.width() * scale).round().max(1.0) as u32;
    let h = (size.height() * scale).round().max(1.0) as u32;
    let mut pm = tiny_skia::Pixmap::new(w, h).ok_or("SVG: размер")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    let rgba = RgbaImage::from_raw(w, h, pm.take_demultiplied()).ok_or("SVG")?;
    Ok(Animation { width: w, height: h, frames: vec![Arc::new(rgba)], delays: vec![1000] })
}

fn load_jxl(bytes: &[u8], max_frames: usize) -> Result<Animation, String> {
    let image = jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(bytes)).map_err(|e| format!("JPEG XL: {e}"))?;
    let mut anim = Animation::default();
    let count = image.num_loaded_keyframes().max(1).min(max_frames);
    for i in 0..count {
        let render = image.render_frame(i).map_err(|e| format!("JPEG XL: {e}"))?;
        let fb = render.image_all_channels();
        let (w, h, ch) = (fb.width() as u32, fb.height() as u32, fb.channels());
        let buf = fb.buf();
        let mut rgba = RgbaImage::new(w, h);
        for (i, px) in rgba.pixels_mut().enumerate() {
            let at = |c: usize| (buf[i * ch + c.min(ch - 1)].clamp(0.0, 1.0) * 255.0).round() as u8;
            *px = match ch {
                1 => image::Rgba([at(0), at(0), at(0), 255]),
                2 => image::Rgba([at(0), at(0), at(0), at(1)]),
                3 => image::Rgba([at(0), at(1), at(2), 255]),
                _ => image::Rgba([at(0), at(1), at(2), at(3)]),
            };
        }
        let delay = image
            .frame_header(i)
            .and_then(|h| image.image_header().metadata.animation.as_ref().map(|a| (h.duration, a)))
            .map(|(d, a)| (d as f64 * 1000.0 * a.tps_denominator as f64 / a.tps_numerator.max(1) as f64) as u32)
            .unwrap_or(100);
        anim.frames.push(Arc::new(rgba));
        anim.delays.push(delay);
    }
    Ok(anim)
}

/// Last resort for formats without a native decoder here (AVIF, HEIC, …): the system's
/// ImageMagick, if installed. Only the first frame.
fn load_external(path: &Path) -> Result<Animation, String> {
    for tool in ["magick", "convert"] {
        let spec = format!("{}[0]", path.display());
        let Ok(out) = std::process::Command::new(tool)
            .args([spec.as_str(), "-auto-orient", "png:-"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        if out.status.success() && !out.stdout.is_empty() {
            let img = image::load_from_memory_with_format(&out.stdout, image::ImageFormat::Png).map_err(|e| e.to_string())?;
            return Ok(single(img));
        }
    }
    Err("формат не поддерживается".into())
}

/// Size and frame count without decoding everything (for gallery entries added by drop).
pub fn probe(path: &Path) -> Option<(u32, u32, u32)> {
    let f = std::fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::new(f);
    let head = r.fill_buf().ok()?.to_vec();
    if let Ok(fmt) = image::guess_format(&head) {
        if let Ok((w, h)) = image::ImageReader::with_format(r, fmt).into_dimensions() {
            let frames = if fmt == image::ImageFormat::Gif { gif_frame_count(path).unwrap_or(1) } else { 1 };
            return Some((w, h, frames));
        }
    }
    let a = load(path, MAX_LOAD_FRAMES).ok()?;
    Some((a.width, a.height, a.frames.len() as u32))
}

fn gif_frame_count(path: &Path) -> Option<u32> {
    let f = std::fs::File::open(path).ok()?;
    let d = image::codecs::gif::GifDecoder::new(std::io::BufReader::new(f)).ok()?;
    Some(d.into_frames().take(MAX_LOAD_FRAMES).count().max(1) as u32)
}

/// The default crop: the largest centred 5:4 rectangle.
pub fn default_crop(w: u32, h: u32) -> NRect {
    NRect::center_5x4(w as f64, h as f64)
}

/// Pixel art by default when the source fits the screen.
pub fn auto_pixel_art(w: u32, h: u32) -> bool {
    w as usize <= WIDTH && h as usize <= HEIGHT
}

fn scale(img: &RgbaImage, w: u32, h: u32, pixel_art: bool) -> RgbaImage {
    if img.width() == w && img.height() == h {
        return img.clone();
    }
    let filter = if pixel_art { FilterType::Nearest } else { FilterType::Triangle };
    image::imageops::resize(img, w.max(1), h.max(1), filter)
}

/// One source frame → 160×128 on black (§6.2).
pub fn render(src: &RgbaImage, crop: NRect, fit: Fit, pixel_art: bool) -> Frame {
    let (cw, ch) = (WIDTH as u32, HEIGHT as u32);
    let mut canvas = RgbaImage::from_pixel(cw, ch, image::Rgba([0, 0, 0, 255]));
    if src.width() == 0 || src.height() == 0 {
        return Frame::from_image(&canvas);
    }
    let (sw, sh) = (src.width() as f64, src.height() as f64);
    let (part, x, y) = match fit {
        Fit::Crop => {
            let c = crop.clamped();
            // aligned (covering) integer rectangle, intersected with the source
            let x0 = (c.x * sw).floor().max(0.0) as u32;
            let y0 = (c.y * sh).floor().max(0.0) as u32;
            let x1 = ((c.x + c.w) * sw).ceil().min(sw) as u32;
            let y1 = ((c.y + c.h) * sh).ceil().min(sh) as u32;
            let (x0, y0, w, h) =
                if x1 > x0 && y1 > y0 { (x0, y0, x1 - x0, y1 - y0) } else { (0, 0, src.width(), src.height()) };
            let view = image::imageops::crop_imm(src, x0, y0, w, h).to_image();
            (scale(&view, cw, ch, pixel_art), 0, 0)
        }
        Fit::Fit => {
            let k = (cw as f64 / sw).min(ch as f64 / sh);
            let w = ((sw * k).round() as u32).clamp(1, cw);
            let h = ((sh * k).round() as u32).clamp(1, ch);
            (scale(src, w, h, pixel_art), (cw - w) / 2, (ch - h) / 2)
        }
        Fit::Stretch => (scale(src, cw, ch, pixel_art), 0, 0),
    };
    image::imageops::overlay(&mut canvas, &part, x as i64, y as i64);
    Frame::from_image(&canvas)
}

pub fn render_all(anim: &Animation, crop: NRect, fit: Fit, pixel_art: bool) -> Vec<Frame> {
    anim.frames.iter().map(|f| render(f, crop, fit, pixel_art)).collect()
}

/// Thins frames to at most 92 (`frames[i * count / keep]`), stretching the speed so the
/// total duration stays the same.
pub fn decimate(frames: Vec<Frame>, speed: u32) -> (Vec<Frame>, u32) {
    let count = frames.len();
    if count <= MAX_FRAMES {
        return (frames, speed);
    }
    let keep = MAX_FRAMES;
    let picked: Vec<Frame> = (0..keep).map(|i| frames[i * count / keep].clone()).collect();
    (picked, ((speed as u64 * count as u64) / keep as u64).clamp(1, 0xffff) as u32)
}

/// A downscaled copy for the editor canvas (≤ `max` px on the long side), shared if small.
pub fn display_copy(img: &Arc<RgbaImage>, max: u32) -> Arc<RgbaImage> {
    let long = img.width().max(img.height());
    if long <= max {
        return img.clone();
    }
    let k = max as f64 / long as f64;
    let w = ((img.width() as f64 * k).round() as u32).max(1);
    let h = ((img.height() as f64 * k).round() as u32).max(1);
    Arc::new(image::imageops::resize(img.as_ref(), w, h, FilterType::Triangle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| if (x + y) % 2 == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) })
    }

    #[test]
    fn render_sizes_and_modes() {
        let src = checker(320, 128);
        let f = render(&src, default_crop(320, 128), Fit::Fit, true);
        // letterboxed: top rows are black
        assert_eq!(f.pixel(80, 0), [0, 0, 0]);
        assert_ne!(f.pixel(80, 64), [0, 0, 0]);
        let f = render(&src, default_crop(320, 128), Fit::Crop, true);
        assert_ne!(f.pixel(80, 0), [0, 0, 0]);
        let f = render(&src, NRect::FULL, Fit::Stretch, false);
        assert_eq!(f.rgb().len(), WIDTH * HEIGHT * 3);
        assert!(auto_pixel_art(160, 128) && !auto_pixel_art(161, 10));
    }

    #[test]
    fn decimation_keeps_duration() {
        let frames: Vec<Frame> = (0..184).map(|i| Frame::solid(i as u8, 0, 0)).collect();
        let (f, speed) = decimate(frames, 50);
        assert_eq!(f.len(), 92);
        assert_eq!(speed, 100);
        assert_eq!(f[1].pixel(0, 0)[0], 2);
    }

    #[test]
    fn gif_roundtrip_with_delays() {
        let dir = std::env::temp_dir().join(format!("minitoo-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.gif");
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut enc = image::codecs::gif::GifEncoder::new(file);
            let frames = (0..3).map(|i| {
                image::Frame::from_parts(
                    RgbaImage::from_pixel(20, 10, image::Rgba([i * 80, 0, 0, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(if i == 0 { 0 } else { 70 }, 1),
                )
            });
            enc.encode_frames(frames).unwrap();
        }
        let a = load(&path, MAX_LOAD_FRAMES).unwrap();
        assert_eq!(a.frames.len(), 3);
        assert_eq!(a.delays, vec![100, 70, 70]);
        assert_eq!((a.width, a.height), (20, 10));
        assert_eq!(a.average_delay(), 80);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
