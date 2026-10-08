//! Divoom MiniToo wire protocol (§2): message framing, incoming frame parser,
//! media payload encoding and packetisation. Pure functions only.

use crate::frame::{Frame, HEIGHT, WIDTH};

pub const MAX_FRAMES: usize = 92;
pub const MAX_MEDIA_BYTES: usize = 307_200;
pub const CHUNK_SIZE: usize = 256;
pub const MAX_FRAME_LEN: usize = 4096;

/// Commands that hang the device until it is power-cycled. Never send them.
pub const FORBIDDEN_COMMANDS: [u8; 5] = [0x40, 0xA3, 0xA4, 0xAD, 0xAE];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ColorDepth {
    #[default]
    Full,
    Rgb565,
    Rgb444,
}

impl ColorDepth {
    /// `screen/quality`: 0 RGB888, 1 RGB565, 2 RGB444
    pub fn from_quality(q: i64) -> Self {
        match q {
            0 => ColorDepth::Full,
            2 => ColorDepth::Rgb444,
            _ => ColorDepth::Rgb565,
        }
    }
}

/// `01 | len LE16 | cmd | args | checksum LE16 | 02`
pub fn make_message(cmd: u8, args: &[u8]) -> Vec<u8> {
    let len = (args.len() + 3) as u16;
    let mut body = Vec::with_capacity(args.len() + 3);
    body.extend_from_slice(&len.to_le_bytes());
    body.push(cmd);
    body.extend_from_slice(args);
    let sum: u32 = body.iter().map(|&b| b as u32).sum();
    let mut msg = Vec::with_capacity(body.len() + 4);
    msg.push(0x01);
    msg.extend_from_slice(&body);
    msg.extend_from_slice(&((sum & 0xffff) as u16).to_le_bytes());
    msg.push(0x02);
    msg
}

/// JSON command (cmd `0x01`), compact UTF-8.
pub fn json_message(value: &serde_json::Value) -> Vec<u8> {
    make_message(0x01, serde_json::to_string(value).unwrap_or_default().as_bytes())
}

/// An incoming frame, already split into `(cmd, data)` (§2.2 step 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub cmd: u8,
    pub data: Vec<u8>,
    /// the frame body as received (cmd + args), for diagnostics
    pub raw: Vec<u8>,
}

/// Accumulates bytes from the socket and yields complete frames.
#[derive(Default)]
pub struct FrameParser {
    buf: Vec<u8>,
}

impl FrameParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Next complete frame, or `None` if more bytes are needed.
    pub fn next_frame(&mut self) -> Option<Incoming> {
        loop {
            let start = self.buf.iter().position(|&b| b == 0x01)?;
            self.buf.drain(..start);
            if self.buf.len() < 3 {
                return None;
            }
            let len = u16::from_le_bytes([self.buf[1], self.buf[2]]) as usize;
            if !(3..=MAX_FRAME_LEN).contains(&len) {
                self.buf.drain(..1);
                continue;
            }
            if self.buf.len() < len + 4 {
                return None;
            }
            if self.buf[len + 3] != 0x02 {
                self.buf.drain(..1);
                continue;
            }
            let body: Vec<u8> = self.buf[3..3 + len - 2].to_vec();
            self.buf.drain(..len + 4);
            if body.is_empty() {
                continue;
            }
            return Some(split_body(body));
        }
    }
}

fn split_body(body: Vec<u8>) -> Incoming {
    if body.len() >= 3 && body[0] == 0x04 && body[2] == 0x55 {
        Incoming { cmd: body[1], data: body[3..].to_vec(), raw: body }
    } else {
        Incoming { cmd: body[0], data: body[1..].to_vec(), raw: body }
    }
}

/// Quantises one RGB888 frame in place to the given depth (format stays RGB888).
pub fn quantize(rgb: &mut [u8], depth: ColorDepth) {
    match depth {
        ColorDepth::Full => {}
        ColorDepth::Rgb565 => {
            for px in rgb.chunks_exact_mut(3) {
                px[0] = (px[0] & 0xf8) | (px[0] >> 5);
                px[1] = (px[1] & 0xfc) | (px[1] >> 6);
                px[2] = (px[2] & 0xf8) | (px[2] >> 5);
            }
        }
        ColorDepth::Rgb444 => {
            for c in rgb.iter_mut() {
                *c = (*c & 0xf0) | (*c >> 4);
            }
        }
    }
}

pub fn frame_bytes(frame: &Frame, depth: ColorDepth) -> Vec<u8> {
    let mut out = frame.rgb().to_vec();
    quantize(&mut out, depth);
    out
}

/// Evenly spread indices: `i * count / keep`.
pub fn pick_frames(count: usize, keep: usize) -> Vec<usize> {
    (0..keep).map(|i| i * count / keep).collect()
}

/// zstd with windowLog 17 (the device decoder has a small window) and the content size in
/// the frame header (mandatory for the device).
pub fn zstd_compress(src: &[u8], level: i32) -> Vec<u8> {
    use zstd::stream::raw::CParameter;
    let mut c = match zstd::bulk::Compressor::new(level.clamp(1, 22)) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let _ = c.set_parameter(CParameter::WindowLog(17));
    let _ = c.set_parameter(CParameter::ContentSizeFlag(true));
    c.compress(src).unwrap_or_default()
}

#[derive(Clone, Debug, Default)]
pub struct EncodedMedia {
    pub payload: Vec<u8>,
    pub frames: usize,
    pub speed: u32,
    pub raw_bytes: usize,
}

/// `25 | frames | speed BE16 | rows=08 | cols=0A | zlen BE32 | zstd(frames)`, decimating
/// frames until the compressed data fits the device limits (§2.3).
pub fn encode_media(frames: &[Frame], speed_ms: u32, level: i32, depth: ColorDepth) -> EncodedMedia {
    let mut result = EncodedMedia::default();
    if frames.is_empty() {
        return result;
    }
    let count = frames.len();
    let mut keep = count.min(MAX_FRAMES);
    let mut cache: std::collections::HashMap<usize, Vec<u8>> = Default::default();
    let z = loop {
        let picked = pick_frames(count, keep);
        let mut raw = Vec::with_capacity(keep * WIDTH * HEIGHT * 3);
        for i in picked {
            let bytes = cache.entry(i).or_insert_with(|| frame_bytes(&frames[i], depth));
            raw.extend_from_slice(bytes);
        }
        result.raw_bytes = raw.len();
        let z = zstd_compress(&raw, level);
        if z.len() <= MAX_MEDIA_BYTES || keep == 1 {
            break z;
        }
        keep = 1.max((keep - 1).min(keep * MAX_MEDIA_BYTES / z.len()));
    };
    let mut speed = speed_ms.clamp(1, 0xffff);
    if keep < count {
        speed = ((speed as f64 * count as f64 / keep as f64).round() as u32).clamp(1, 0xffff);
    }
    let mut payload = Vec::with_capacity(z.len() + 10);
    payload.push(0x25);
    payload.push(keep as u8);
    payload.extend_from_slice(&(speed as u16).to_be_bytes());
    payload.push((HEIGHT / 16) as u8);
    payload.push((WIDTH / 16) as u8);
    payload.extend_from_slice(&(z.len() as u32).to_be_bytes());
    payload.extend_from_slice(&z);
    result.payload = payload;
    result.frames = keep;
    result.speed = speed;
    result
}

/// `start: 00 | total LE32`, then `chunk: 01 | total LE32 | seq LE16 | ≤256 bytes`.
pub fn media_packets(payload: &[u8]) -> Vec<Vec<u8>> {
    let total = (payload.len() as u32).to_le_bytes();
    let mut packets = Vec::with_capacity(payload.len() / CHUNK_SIZE + 2);
    let mut start = vec![0x00];
    start.extend_from_slice(&total);
    packets.push(start);
    for (seq, chunk) in payload.chunks(CHUNK_SIZE).enumerate() {
        let mut p = Vec::with_capacity(chunk.len() + 7);
        p.push(0x01);
        p.extend_from_slice(&total);
        p.extend_from_slice(&(seq as u16).to_le_bytes());
        p.extend_from_slice(chunk);
        packets.push(p);
    }
    packets
}

/// Set-time command args: `yy%100, yy/100, MM, dd, HH, mm, ss, 00`.
pub fn time_args(t: &chrono::NaiveDateTime) -> Vec<u8> {
    use chrono::{Datelike, Timelike};
    let y = t.year();
    vec![
        (y % 100) as u8,
        (y / 100) as u8,
        t.month() as u8,
        t.day() as u8,
        t.hour() as u8,
        t.minute() as u8,
        t.second() as u8,
        0,
    ]
}

/// Parses a diagnostic hex string "cmd args…" (spaces optional).
pub fn parse_hex(text: &str) -> Option<(u8, Vec<u8>)> {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',' && *c != ':')
        .collect::<String>()
        .trim_start_matches("0x")
        .to_string();
    if clean.is_empty() || clean.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).ok())
        .collect();
    let bytes = bytes?;
    Some((bytes[0], bytes[1..].to_vec()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// Built-in notification icon codes for cmd `0x50` (§2.4).
pub const NOTIFY_APPS: [(&str, u8); 12] = [
    ("Divoom", 13),
    ("Telegram", 18),
    ("WhatsApp", 7),
    ("Discord", 20),
    ("Instagram", 1),
    ("Facebook", 3),
    ("Messenger", 15),
    ("Twitter", 4),
    ("Skype", 9),
    ("VK", 17),
    ("WeChat", 11),
    ("TikTok", 19),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_message_vector() {
        assert_eq!(make_message(0x74, &[0x32]), vec![0x01, 0x04, 0x00, 0x74, 0x32, 0xaa, 0x00, 0x02]);
    }

    #[test]
    fn long_message_checksum_masked() {
        let args = vec![0xffu8; 263];
        let m = make_message(0x8b, &args);
        assert_eq!(m.len(), 1 + 2 + 1 + 263 + 2 + 1);
        let sum: u32 = m[1..m.len() - 3].iter().map(|&b| b as u32).sum();
        assert_eq!(u16::from_le_bytes([m[m.len() - 3], m[m.len() - 2]]), (sum & 0xffff) as u16);
    }

    #[test]
    fn parser_roundtrip_and_resync() {
        let mut p = FrameParser::new();
        let mut stream = vec![0xaa, 0x01, 0x02, 0x00]; // garbage incl. a fake start (len 2 < 3)
        stream.extend(make_message(0x04, &[0x09, 0x55, 0x07]));
        stream.extend(make_message(0xf7, b"Nob"));
        p.push(&stream[..5]);
        assert!(p.next_frame().is_none());
        p.push(&stream[5..]);
        let a = p.next_frame().unwrap();
        assert_eq!((a.cmd, a.data.clone()), (0x09, vec![0x07]));
        let b = p.next_frame().unwrap();
        assert_eq!((b.cmd, b.data.clone()), (0xf7, b"Nob".to_vec()));
        assert!(p.next_frame().is_none());
    }

    #[test]
    fn packets_600() {
        let payload = vec![7u8; 600];
        let p = media_packets(&payload);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], vec![0x00, 0x58, 0x02, 0x00, 0x00]);
        for (i, chunk) in p[1..].iter().enumerate() {
            assert_eq!(chunk[0], 0x01);
            assert_eq!(&chunk[1..5], &[0x58, 0x02, 0, 0]);
            assert_eq!(u16::from_le_bytes([chunk[5], chunk[6]]) as usize, i);
        }
        assert_eq!(p[3].len() - 7, 88);
    }

    #[test]
    fn media_header_and_zstd_params() {
        let frames = vec![Frame::solid(10, 20, 30), Frame::solid(200, 100, 50)];
        let m = encode_media(&frames, 100, 19, ColorDepth::Full);
        assert_eq!(m.payload[0], 0x25);
        assert_eq!(m.payload[1], 2);
        assert_eq!(u16::from_be_bytes([m.payload[2], m.payload[3]]), 100);
        assert_eq!(&m.payload[4..6], &[0x08, 0x0a]);
        let zlen = u32::from_be_bytes([m.payload[6], m.payload[7], m.payload[8], m.payload[9]]) as usize;
        assert_eq!(zlen, m.payload.len() - 10);
        let z = &m.payload[10..];
        // the content size is in the frame header
        let size = zstd::zstd_safe::get_frame_content_size(z).unwrap().unwrap();
        assert_eq!(size as usize, 2 * WIDTH * HEIGHT * 3);
        let raw = zstd::bulk::decompress(z, 1 << 20).unwrap();
        assert_eq!(&raw[..3], &[10, 20, 30]);
    }

    #[test]
    fn zstd_window_is_17() {
        // 20 distinct noisy frames: content (1.2 MB) is bigger than the window, so the frame
        // header carries a window descriptor. Exponent = 17 - 10 = 7, mantissa 0.
        let mut frames = Vec::new();
        let mut seed = 1u32;
        for _ in 0..20 {
            let mut rgb = vec![0u8; WIDTH * HEIGHT * 3];
            for b in rgb.iter_mut() {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                *b = (seed >> 24) as u8 & 0x0f;
            }
            frames.push(Frame::from_rgb(rgb));
        }
        let raw: Vec<u8> = frames.iter().flat_map(|f| f.rgb().to_vec()).collect();
        let z = zstd_compress(&raw, 3);
        assert_eq!(&z[..4], &[0x28, 0xb5, 0x2f, 0xfd]);
        let fhd = z[4];
        assert_eq!(fhd & 0x20, 0, "not single segment");
        assert_ne!(fhd >> 6, 0, "content size present");
        assert_eq!(z[5], 7 << 3);
    }

    #[test]
    fn decimation() {
        assert_eq!(pick_frames(10, 4), vec![0, 2, 5, 7]);
        let frames: Vec<Frame> = (0..100).map(|i| Frame::solid(i as u8, 0, 0)).collect();
        let m = encode_media(&frames, 50, 3, ColorDepth::Full);
        assert_eq!(m.frames, 92);
        assert_eq!(m.speed, (50.0f64 * 100.0 / 92.0).round() as u32);
    }

    #[test]
    fn color_depth() {
        let mut px = vec![0xffu8, 0x81, 0x13];
        quantize(&mut px, ColorDepth::Rgb565);
        assert_eq!(px, vec![0xff, 0x82, 0x10]);
        let mut px = vec![0xffu8, 0x81, 0x13];
        quantize(&mut px, ColorDepth::Rgb444);
        assert_eq!(px, vec![0xff, 0x88, 0x11]);
    }

    #[test]
    fn hex_parse() {
        assert_eq!(parse_hex("09"), Some((0x09, vec![])));
        assert_eq!(parse_hex("72 02 01 00"), Some((0x72, vec![2, 1, 0])));
        assert_eq!(parse_hex("7"), None);
    }
}
