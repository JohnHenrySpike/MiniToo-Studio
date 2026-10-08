//! Minimal animated GIF89a encoder (port of GifWriter.cpp): LZW, looping forever, one global
//! 256-colour palette when all frames fit into it (lossless), otherwise a local palette per frame
//! (frames with more than 256 colours are reduced to a 6×6×6 colour cube).

use crate::frame::{Frame, HEIGHT, WIDTH};
use std::collections::HashMap;
use std::path::Path;

const MIN_CODE_SIZE: u32 = 8; // palettes are always padded to 256 entries

struct BitWriter {
    bytes: Vec<u8>,
    acc: u32,
    bits: u32,
}

impl BitWriter {
    fn put(&mut self, code: u32, size: u32) {
        self.acc |= code << self.bits;
        self.bits += size;
        while self.bits >= 8 {
            self.bytes.push(self.acc as u8);
            self.acc >>= 8;
            self.bits -= 8;
        }
    }

    fn flush(&mut self) {
        if self.bits > 0 {
            self.bytes.push(self.acc as u8);
        }
        self.acc = 0;
        self.bits = 0;
    }
}

fn lzw(indices: &[u8]) -> Vec<u8> {
    let clear = 1u32 << MIN_CODE_SIZE;
    let eoi = clear + 1;
    let mut out = BitWriter { bytes: Vec::new(), acc: 0, bits: 0 };
    let mut dict: HashMap<u32, u32> = HashMap::new();
    let mut code_size = MIN_CODE_SIZE + 1;
    let mut max_code = eoi;
    out.put(clear, code_size);
    let Some((&first, rest)) = indices.split_first() else {
        out.put(eoi, code_size);
        out.flush();
        return out.bytes;
    };
    let mut prefix = first as u32;
    for &k in rest {
        let key = (prefix << 8) | k as u32;
        if let Some(&code) = dict.get(&key) {
            prefix = code;
            continue;
        }
        out.put(prefix, code_size);
        max_code += 1;
        dict.insert(key, max_code);
        if max_code >= (1 << code_size) {
            code_size += 1;
        }
        if max_code == 4095 {
            out.put(clear, code_size);
            dict.clear();
            code_size = MIN_CODE_SIZE + 1;
            max_code = eoi;
        }
        prefix = k as u32;
    }
    out.put(prefix, code_size);
    out.put(eoi, code_size);
    out.flush();
    out.bytes
}

fn put_u16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_palette(b: &mut Vec<u8>, pal: &[[u8; 3]]) {
    for i in 0..256 {
        b.extend_from_slice(&pal.get(i).copied().unwrap_or([0, 0, 0]));
    }
}

fn put_sub_blocks(b: &mut Vec<u8>, data: &[u8]) {
    for chunk in data.chunks(255) {
        b.push(chunk.len() as u8);
        b.extend_from_slice(chunk);
    }
    b.push(0);
}

/// Indices of the RGB pixels in `pal`, extending it as needed; `None` past 256 colours.
fn index_frame(rgb: &[u8], pal: &mut Vec<[u8; 3]>, lookup: &mut HashMap<[u8; 3], u8>) -> Option<Vec<u8>> {
    let mut indices = Vec::with_capacity(rgb.len() / 3);
    for &c in rgb.as_chunks::<3>().0 {
        let i = match lookup.get(&c) {
            Some(&i) => i,
            None => {
                if pal.len() >= 256 {
                    return None;
                }
                let i = pal.len() as u8;
                lookup.insert(c, i);
                pal.push(c);
                i
            }
        };
        indices.push(i);
    }
    Some(indices)
}

/// Fallback for frames with more than 256 colours: nearest entry of a 6×6×6 cube.
fn quantize(rgb: &[u8]) -> (Vec<[u8; 3]>, Vec<u8>) {
    let level = |v: u8| ((v as u32 * 5 + 127) / 255) as u8;
    let mut pal = Vec::with_capacity(216);
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                pal.push([r * 51, g * 51, b * 51]);
            }
        }
    }
    let idx = rgb.as_chunks::<3>().0.iter().map(|p| level(p[0]) * 36 + level(p[1]) * 6 + level(p[2])).collect();
    (pal, idx)
}

/// Encodes RGB888 frames of `width × height` (each `width·height·3` bytes; shorter ones are
/// padded with black). Missing delays default to 100 ms. Empty input gives an empty result.
pub fn encode_rgb(width: u16, height: u16, frames: &[Vec<u8>], delays_ms: &[u32]) -> Vec<u8> {
    if frames.is_empty() {
        return Vec::new();
    }
    let size = width as usize * height as usize * 3;
    let rgb: Vec<Vec<u8>> = frames
        .iter()
        .map(|f| {
            let mut f = f.clone();
            f.resize(size, 0);
            f
        })
        .collect();

    // try one shared palette first
    let mut global = Vec::new();
    let mut lookup = HashMap::new();
    let mut indices = Vec::with_capacity(rgb.len());
    let mut shared = true;
    for f in &rgb {
        match index_frame(f, &mut global, &mut lookup) {
            Some(idx) => indices.push(idx),
            None => {
                shared = false;
                break;
            }
        }
    }

    let mut out = b"GIF89a".to_vec();
    put_u16(&mut out, width);
    put_u16(&mut out, height);
    out.push(if shared { 0xf7 } else { 0x70 }); // global table flag + 8-bit colour resolution + 256 entries
    out.push(0); // background index
    out.push(0); // aspect ratio
    if shared {
        put_palette(&mut out, &global);
    }
    // NETSCAPE2.0 loop forever
    out.extend_from_slice(b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00");

    for (i, f) in rgb.iter().enumerate() {
        let ms = delays_ms.get(i).copied().unwrap_or(100);
        let cs = ((ms + 5) / 10).clamp(2, u16::MAX as u32) as u16;
        out.extend_from_slice(b"\x21\xf9\x04");
        out.push(0x04); // disposal: leave in place, no transparency
        put_u16(&mut out, cs);
        out.push(0);
        out.push(0);

        let (local, idx) = if shared {
            (Vec::new(), std::mem::take(&mut indices[i]))
        } else {
            let mut local = Vec::new();
            match index_frame(f, &mut local, &mut HashMap::new()) {
                Some(idx) => (local, idx),
                None => quantize(f),
            }
        };
        out.push(0x2c);
        put_u16(&mut out, 0);
        put_u16(&mut out, 0);
        put_u16(&mut out, width);
        put_u16(&mut out, height);
        out.push(if shared { 0x00 } else { 0x87 });
        if !shared {
            put_palette(&mut out, &local);
        }
        out.push(MIN_CODE_SIZE as u8);
        put_sub_blocks(&mut out, &lzw(&idx));
    }
    out.push(0x3b);
    out
}

/// Encodes 160×128 device frames.
pub fn encode_gif(frames: &[Frame], delays_ms: &[u32]) -> Vec<u8> {
    let rgb: Vec<Vec<u8>> = frames.iter().map(|f| f.rgb().to_vec()).collect();
    encode_rgb(WIDTH as u16, HEIGHT as u16, &rgb, delays_ms)
}

pub fn write_gif(path: &Path, frames: &[Frame], delays_ms: &[u32]) -> std::io::Result<()> {
    if frames.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no frames"));
    }
    std::fs::write(path, encode_gif(frames, delays_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;

    fn decode(bytes: Vec<u8>) -> Vec<image::Frame> {
        GifDecoder::new(std::io::Cursor::new(bytes)).unwrap().into_frames().collect_frames().unwrap()
    }

    fn same(g: &image::Frame, f: &Frame) -> bool {
        g.buffer().pixels().zip(f.rgb().as_chunks::<3>().0).all(|(p, q)| p.0[..3] == q[..])
    }

    /// Frame `k` with `n` distinct colours.
    fn colourful(k: u32, n: u32) -> Frame {
        let mut v = Vec::new();
        for i in 0..(WIDTH * HEIGHT) as u32 {
            let c = (i % n) + k * n;
            v.extend_from_slice(&[c as u8, (c >> 8) as u8, (i / 160) as u8 & 0x80]);
        }
        Frame::from_rgb(v)
    }

    #[test]
    fn roundtrip_is_lossless() {
        // shared global palette
        let small: Vec<Frame> = (0..4u8).map(|k| Frame::solid(k * 40, 255 - k, 7)).collect();
        let got = decode(encode_gif(&small, &[140, 70, 220]));
        assert_eq!(got.len(), 4);
        assert!(got.iter().zip(&small).all(|(g, f)| same(g, f)));
        assert_eq!(got[0].delay().numer_denom_ms(), (140, 1));
        assert_eq!(got[3].delay().numer_denom_ms(), (100, 1));

        // more than 256 colours in total: local palettes, still exact per frame
        let many: Vec<Frame> = (0..3).map(|k| colourful(k, 100)).collect();
        let got = decode(encode_gif(&many, &[100; 3]));
        assert!(got.iter().zip(&many).all(|(g, f)| same(g, f)));

        // too many colours in one frame: quantized, but still decodable
        let got = decode(encode_gif(&[colourful(0, 2000)], &[100]));
        assert_eq!(got.len(), 1);
        assert!(encode_gif(&[], &[]).is_empty());
    }
}
