//! A 160×128 RGB888 device frame. Cheap to clone (shared buffer), compared byte by byte.

use std::sync::Arc;

pub const WIDTH: usize = 160;
pub const HEIGHT: usize = 128;
pub const FRAME_BYTES: usize = WIDTH * HEIGHT * 3;

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Frame(Arc<[u8]>);

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}×{})", WIDTH, HEIGHT)
    }
}

impl Frame {
    /// `rgb` must hold exactly 160·128·3 bytes; shorter input is padded with black.
    pub fn from_rgb(mut rgb: Vec<u8>) -> Self {
        rgb.resize(FRAME_BYTES, 0);
        Frame(rgb.into())
    }

    pub fn black() -> Self {
        Self::solid(0, 0, 0)
    }

    pub fn solid(r: u8, g: u8, b: u8) -> Self {
        let mut v = Vec::with_capacity(FRAME_BYTES);
        for _ in 0..WIDTH * HEIGHT {
            v.extend_from_slice(&[r, g, b]);
        }
        Frame(v.into())
    }

    /// From straight (non-premultiplied) or premultiplied RGBA: the colour is composed over
    /// black either way when `premultiplied` is true.
    pub fn from_rgba(rgba: &[u8], premultiplied: bool) -> Self {
        let mut v = Vec::with_capacity(FRAME_BYTES);
        for px in rgba.chunks_exact(4).take(WIDTH * HEIGHT) {
            if premultiplied || px[3] == 255 {
                v.extend_from_slice(&px[..3]);
            } else {
                let a = px[3] as u32;
                v.extend_from_slice(&[
                    (px[0] as u32 * a / 255) as u8,
                    (px[1] as u32 * a / 255) as u8,
                    (px[2] as u32 * a / 255) as u8,
                ]);
            }
        }
        Self::from_rgb(v)
    }

    pub fn rgb(&self) -> &[u8] {
        &self.0
    }

    pub fn pixel(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * WIDTH + x) * 3;
        [self.0[i], self.0[i + 1], self.0[i + 2]]
    }

    pub fn to_rgba(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(WIDTH * HEIGHT * 4);
        for px in self.0.chunks_exact(3) {
            v.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        v
    }

    pub fn to_image(&self) -> image::RgbImage {
        image::RgbImage::from_raw(WIDTH as u32, HEIGHT as u32, self.0.to_vec()).expect("frame size")
    }

    pub fn from_image(img: &image::RgbaImage) -> Self {
        if img.width() as usize == WIDTH && img.height() as usize == HEIGHT {
            return Self::from_rgba(img.as_raw(), false);
        }
        let scaled =
            image::imageops::resize(img, WIDTH as u32, HEIGHT as u32, image::imageops::FilterType::Triangle);
        Self::from_rgba(scaled.as_raw(), false)
    }

    pub fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let img = self.to_image();
        let _ = img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
        out
    }

    /// Stable identity of the buffer contents (for caching textures and keys).
    pub fn digest(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut h);
        h.finish()
    }
}

/// Frames plus the per-frame duration the device plays them with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub frames: Vec<Frame>,
    pub speed: u32,
}

impl Content {
    pub fn new(frames: Vec<Frame>, speed: u32) -> Self {
        Self { frames, speed }
    }
    pub fn single(frame: Frame) -> Self {
        Self { frames: vec![frame], speed: 1000 }
    }
}
