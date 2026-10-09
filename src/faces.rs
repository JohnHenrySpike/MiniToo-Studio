//! Built-in Claude scenes (§11.3, §17.3): a 1:1 port of ClaudeFaces.cpp. Everything is drawn on a
//! 40×32 grid as a deterministic function of the frame index and upscaled ×4 to 160×128.

use crate::canvas::{Align, Canvas, r};
use crate::claude::ClaudeState;
use crate::color::Color;
use crate::fonts::FontSpec;
use crate::frame::{Frame, HEIGHT, WIDTH};
use std::f64::consts::PI;
use std::path::Path;

/// One scene: frames (160×128, upscaled ×4 from the 40×32 grid) and per-frame delays in ms.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub frames: Vec<Frame>,
    pub delays: Vec<u32>,
}

impl Scene {
    fn push(&mut self, g: &Grid, delay: u32) {
        self.frames.push(g.scaled());
        self.delays.push(delay);
    }
}

// ------------------------------------------------------------------------------------------ grid

const GW: i32 = 40;
const GH: i32 = 32;
const SCALE: usize = WIDTH / GW as usize;

const ORANGE: Color = Color::hex(0xd97757);
const ORANGE_DARK: Color = Color::hex(0xb0553a);
const INK: Color = Color::hex(0x1a1412);
const WHITE: Color = Color::hex(0xf6f1e8);
const BLUSH: Color = Color::hex(0xf09088);

const fn c(rgb: u32) -> Color {
    Color::hex(rgb)
}

/// Qt's `qRound` (half away from zero).
fn qround(d: f64) -> i32 {
    if d >= 0.0 { (d + 0.5) as i32 } else { (d - 0.5) as i32 }
}

fn mix(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let m = |a: u8, b: u8| qround(a as f64 + (b as f64 - a as f64) * t) as u8;
    Color::rgb(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// IEEE `remainder(x, y)` (C's `std::remainder`), computed exactly via `fmod`.
fn ieee_remainder(x: f64, y: f64) -> f64 {
    let y = y.abs();
    let mut r = x % y;
    let half = y / 2.0;
    if r.abs() > half || (r.abs() == half && ((x - r) / y).rem_euclid(2.0) != 0.0) {
        r -= y.copysign(r);
    }
    r
}

struct Grid {
    px: Vec<Color>,
}

impl Grid {
    fn new(bg: Color) -> Self {
        Grid { px: vec![bg; (GW * GH) as usize] }
    }

    fn px(&mut self, x: i32, y: i32, c: Color) {
        if x >= 0 && y >= 0 && x < GW && y < GH {
            self.px[(y * GW + x) as usize] = c;
        }
    }

    fn blend(&mut self, x: i32, y: i32, c: Color, a: f64) {
        if x >= 0 && y >= 0 && x < GW && y < GH {
            let i = (y * GW + x) as usize;
            self.px[i] = mix(self.px[i], c, a);
        }
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Color) {
        for j in 0..h {
            for i in 0..w {
                self.px(x + i, y + j, c);
            }
        }
    }

    fn glyph(&mut self, x: i32, y: i32, rows: &[&str], c: Color) {
        self.sprite(x, y, rows, &[('#', c)]);
    }

    /// Multi-colour sprite: every character found in `pal` is painted, everything else is transparent.
    fn sprite(&mut self, x: i32, y: i32, rows: &[&str], pal: &[(char, Color)]) {
        for (j, row) in rows.iter().enumerate() {
            for (i, ch) in row.chars().enumerate() {
                for &(k, c) in pal {
                    if ch == k {
                        self.px(x + i as i32, y + j as i32, c);
                    }
                }
            }
        }
    }

    /// Bresenham.
    fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, c: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.px(x0, y0, c);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }

    fn disc(&mut self, cx: f64, cy: f64, r: f64, c: Color) {
        for y in (cy - r) as i32 - 1..=(cy + r) as i32 + 1 {
            for x in (cx - r) as i32 - 1..=(cx + r) as i32 + 1 {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                if dx * dx + dy * dy <= r * r {
                    self.px(x, y, c);
                }
            }
        }
    }

    fn vgrad(&mut self, y0: i32, y1: i32, top: Color, bottom: Color) {
        for y in y0..=y1 {
            self.rect(0, y, GW, 1, mix(top, bottom, (y - y0) as f64 / (y1 - y0).max(1) as f64));
        }
    }

    fn scaled(&self) -> Frame {
        let mut v = Vec::with_capacity(WIDTH * HEIGHT * 3);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let p = self.px[(y / SCALE) * GW as usize + x / SCALE];
                v.extend_from_slice(&[p.r, p.g, p.b]);
            }
        }
        Frame::from_rgb(v)
    }
}

// ------------------------------------------------------------------------------------------ the mascot

#[derive(Clone, Copy, PartialEq, Eq)]
enum Eyes {
    Open,
    Closed,
    Wide,
    Happy,
    Relaxed,
    Down,
    Up,
    Shades,
    Focus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mouth {
    None,
    Smile,
    O,
    Shout,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Legs {
    Stand,
    RunA,
    RunB,
    Short,
    None,
}

/// Relative to the body origin; `w == 0` hides it.
#[derive(Clone, Copy)]
struct Limb {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

const fn limb(x: i32, y: i32, w: i32, h: i32) -> Limb {
    Limb { x, y, w, h }
}

const L_DOWN: Limb = limb(-2, 5, 2, 3);
const R_DOWN: Limb = limb(16, 5, 2, 3);
const L_UP: Limb = limb(-2, 1, 2, 4);
const R_UP: Limb = limb(16, 1, 2, 4);
const NO_LIMB: Limb = limb(0, 0, 0, 0);

#[derive(Clone, Copy)]
struct Pose {
    eyes: Eyes,
    look_x: i32,
    arm_l: Limb,
    arm_r: Limb,
    legs: Legs,
    mouth: Mouth,
    blush: bool,
}

impl Default for Pose {
    fn default() -> Self {
        Pose {
            eyes: Eyes::Open,
            look_x: 0,
            arm_l: L_DOWN,
            arm_r: R_DOWN,
            legs: Legs::Stand,
            mouth: Mouth::None,
            blush: false,
        }
    }
}

/// The mascot: a rounded terracotta block (16×12) with stubby arms and four legs.
fn critter(g: &mut Grid, bx: i32, by: i32, p: &Pose) {
    g.rect(bx + 1, by, 14, 12, ORANGE);
    g.rect(bx, by + 1, 16, 10, ORANGE);
    g.rect(bx + 1, by + 11, 14, 1, ORANGE_DARK);

    for (i, leg_x) in [2, 5, 9, 12].into_iter().enumerate() {
        let (mut x, mut h) = (leg_x, 3);
        match p.legs {
            Legs::Stand => {}
            Legs::RunA | Legs::RunB => {
                let planted = (i % 2 == 0) == (p.legs == Legs::RunA);
                x += if planted { -1 } else { 1 };
                h = if planted { 3 } else { 2 };
            }
            Legs::Short => h = 1,
            Legs::None => h = 0,
        }
        g.rect(bx + x, by + 12, 2, h, ORANGE_DARK);
    }

    for l in [p.arm_l, p.arm_r] {
        if l.w > 0 {
            g.rect(bx + l.x, by + l.y, l.w, l.h, ORANGE);
        }
    }

    let lx = p.look_x;
    match p.eyes {
        Eyes::Open => {
            g.rect(bx + 4 + lx, by + 4, 2, 3, INK);
            g.rect(bx + 10 + lx, by + 4, 2, 3, INK);
        }
        Eyes::Closed => {
            g.rect(bx + 3, by + 6, 4, 1, INK);
            g.rect(bx + 9, by + 6, 4, 1, INK);
        }
        Eyes::Wide => {
            g.rect(bx + 3, by + 3, 4, 4, WHITE);
            g.rect(bx + 9, by + 3, 4, 4, WHITE);
            g.rect(bx + 4, by + 4, 2, 2, INK);
            g.rect(bx + 10, by + 4, 2, 2, INK);
            g.rect(bx + 7, by + 8, 2, 2, INK);
        }
        Eyes::Happy => {
            for ex in [3, 9] {
                g.glyph(bx + ex, by + 4, &[".##.", "#..#"], INK);
            }
        }
        Eyes::Relaxed => {
            for ex in [3, 9] {
                g.glyph(bx + ex, by + 5, &["#..#", ".##."], INK);
            }
        }
        Eyes::Down => {
            g.rect(bx + 4 + lx, by + 6, 2, 2, INK);
            g.rect(bx + 10 + lx, by + 6, 2, 2, INK);
        }
        Eyes::Up => {
            g.rect(bx + 4 + lx, by + 3, 2, 2, INK);
            g.rect(bx + 10 + lx, by + 3, 2, 2, INK);
        }
        Eyes::Shades => {
            g.rect(bx + 2, by + 4, 12, 1, INK);
            g.rect(bx + 3, by + 4, 4, 3, INK);
            g.rect(bx + 9, by + 4, 4, 3, INK);
        }
        Eyes::Focus => {
            // determined: brows slanting down to the middle
            g.rect(bx + 4 + lx, by + 5, 2, 2, INK);
            g.rect(bx + 10 + lx, by + 5, 2, 2, INK);
            g.glyph(bx + 3, by + 2, &["##..", "..##"], INK);
            g.glyph(bx + 9, by + 2, &["..##", "##.."], INK);
        }
    }

    match p.mouth {
        Mouth::None => {}
        Mouth::Smile => g.glyph(bx + 6, by + 8, &["#..#", ".##."], INK),
        Mouth::O => g.rect(bx + 7, by + 8, 2, 2, INK),
        Mouth::Shout => {
            g.rect(bx + 6, by + 8, 4, 3, INK);
            g.rect(bx + 7, by + 9, 2, 2, c(0x8a2a20));
        }
    }
    if p.blush {
        g.rect(bx + 1, by + 8, 2, 1, BLUSH);
        g.rect(bx + 13, by + 8, 2, 1, BLUSH);
    }
}

/// Arm raised diagonally up and out (waving).
fn arm_out(g: &mut Grid, bx: i32, by: i32, left: bool) {
    if left {
        g.glyph(bx - 5, by - 1, &["##...", "###..", ".###.", "..###", "...##"], ORANGE);
    } else {
        g.glyph(bx + 16, by - 1, &["...##", "..###", ".###.", "###..", "##..."], ORANGE);
    }
}

// ------------------------------------------------------------------------------------------ classic set

#[derive(Clone, Copy)]
enum Arms {
    Down,
    Up,
    TypeLeft,
    TypeRight,
}

fn draw_critter(g: &mut Grid, bx: i32, by: i32, eyes: Eyes, look: i32, arms: Arms) {
    let mut p = Pose { eyes, look_x: look, ..Pose::default() };
    match arms {
        Arms::Down => {}
        Arms::Up => (p.arm_l, p.arm_r) = (L_UP, R_UP),
        Arms::TypeLeft => (p.arm_l, p.arm_r) = (limb(-2, 7, 2, 2), limb(16, 8, 2, 2)),
        Arms::TypeRight => (p.arm_l, p.arm_r) = (limb(-2, 8, 2, 2), limb(16, 7, 2, 2)),
    }
    critter(g, bx, by, &p);
}

fn working() -> Scene {
    let mut a = Scene::default();
    let looks = [0, 0, 1, 1, 0, 0, -1, -1];
    for f in 0..8 {
        let mut g = Grid::new(c(0x151524));
        // laptop
        g.rect(9, 27, 22, 2, c(0x5b6075));
        g.rect(11, 26, 18, 1, c(0x3d4152));
        let arms = if f % 2 != 0 { Arms::TypeLeft } else { Arms::TypeRight };
        draw_critter(&mut g, 12, 9 + (f % 2), Eyes::Open, looks[f as usize], arms);
        // thinking dots fill up
        for d in 0..3 {
            let lit = d < f % 4;
            g.rect(14 + d * 5, 4, 2, 2, if lit { ORANGE } else { c(0x3a3a50) });
        }
        // spinning spark
        let spark = c(0xf0a070);
        if f % 2 == 0 {
            g.glyph(32, 2, &["..#..", "..#..", "#####", "..#..", "..#.."], spark);
        } else {
            g.glyph(32, 2, &["#...#", ".#.#.", "..#..", ".#.#.", "#...#"], spark);
        }
        // progress sweep along the bottom edge
        for i in 0..8 {
            g.px((f * 5 + i) % GW, 31, ORANGE);
        }
        a.push(&g, 140);
    }
    a
}

fn alerting() -> Scene {
    let mut a = Scene::default();
    let jump = [0, -2, -3, -2, 0, 0];
    for (f, dy) in jump.into_iter().enumerate() {
        let mut g = Grid::new(if f % 2 != 0 { c(0x2c0b0b) } else { c(0x4d1210) });
        if f % 2 == 0 {
            let amber = c(0xffb020);
            g.rect(0, 0, GW, 1, amber);
            g.rect(0, GH - 1, GW, 1, amber);
            g.rect(0, 0, 1, GH, amber);
            g.rect(GW - 1, 0, 1, GH, amber);
        }
        draw_critter(&mut g, 10, 12 + dy, Eyes::Wide, 0, Arms::Up);
        // speech bubble with "!"
        g.rect(30, 2, 7, 10, WHITE);
        g.rect(29, 3, 9, 8, WHITE);
        g.px(30, 12, WHITE);
        g.px(29, 13, WHITE);
        let mark = if f % 2 != 0 { c(0xe03020) } else { c(0xff8010) };
        g.rect(33, 4, 2, 4, mark);
        g.rect(33, 9, 2, 1, mark);
        a.push(&g, 170);
    }
    a
}

fn chilling() -> Scene {
    let mut a = Scene::default();
    let breathe = [0, 0, 0, 1, 1, 1, 0, 0];
    let stars = [(4, 4), (12, 2), (22, 6), (35, 3), (30, 10), (2, 14), (37, 16)];
    for f in 0..8 {
        let mut g = Grid::new(c(0x0c1226));
        // moon
        g.glyph(3, 3, &[".###.", "##...", "#....", "##...", ".###."], c(0xf2e6a6));
        for (s, &(sx, sy)) in stars.iter().enumerate() {
            let on = (f + s as i32) % 3 != 0;
            g.px(sx + 6, sy, if on { c(0xc8d0ff) } else { c(0x404870) });
        }
        g.rect(0, 28, GW, 4, c(0x18223c)); // ground
        draw_critter(&mut g, 11, 13 + breathe[f as usize], Eyes::Closed, 0, Arms::Down);
        // rising z's
        for k in 0..3 {
            let phase = (f + k * 3) % 9;
            if phase > 7 {
                continue;
            }
            let x = 28 + phase / 2 + k;
            let y = 12 - phase;
            let shade = (255 - phase * 22) as u8;
            let zc = Color::rgb(shade, shade, 255);
            if k == 1 {
                g.glyph(x, y - 1, &["#####", "...#.", "..#..", ".#...", "#####"], zc);
            } else {
                g.glyph(x, y, &["####", "..#.", ".#..", "####"], zc);
            }
        }
        a.push(&g, 360);
    }
    a
}

// ------------------------------------------------------------------------------------------ working

const STEEL: Color = c(0x9aa3b5);
const STEEL_DARK: Color = c(0x6d7487);
const WOOD: Color = c(0x8a5a36);

fn work_hammer() -> Scene {
    // swing: 0 raised, 1 halfway, 2 striking; spark: 0 none, 1 flash, 2 flying, 3 fading
    let seq = [(0, 0, 220), (1, 0, 70), (2, 1, 130), (2, 2, 110), (2, 3, 110), (1, 0, 90), (0, 0, 120)];
    let mut a = Scene::default();
    for (swing, spark, delay) in seq {
        let mut g = Grid::new(c(0x1d1820));
        g.rect(0, 29, GW, 3, c(0x2e2634));
        // anvil with a glowing workpiece
        g.sprite(
            23,
            21,
            &[
                "LLLLLLLLLLLLLLL",
                "..DDDDDDDDDDDD.",
                "....DDDDDDD....",
                ".....DDDDD.....",
                ".....DDDDD.....",
                "...DDDDDDDDD...",
                "..DDDDDDDDDDD..",
                "..DDDDDDDDDDD..",
            ],
            &[('L', STEEL), ('D', STEEL_DARK)],
        );
        g.rect(26, 20, 8, 1, if spark == 1 { c(0xfff2a8) } else { c(0xff9a30) });
        if spark == 1 || spark == 2 {
            g.rect(27, 19, 6, 1, c(0xffc860));
        }

        let (bx, by) = (3, if swing == 2 { 15 } else { 14 });
        let mut p = Pose { eyes: if spark == 1 { Eyes::Closed } else { Eyes::Focus }, look_x: 1, ..Pose::default() };
        if swing == 0 {
            p.arm_r = limb(16, 1, 2, 4);
            g.rect(19, 7, 1, 8, WOOD);
            g.rect(15, 3, 9, 4, STEEL);
            g.rect(15, 6, 9, 1, STEEL_DARK);
        } else if swing == 1 {
            p.arm_r = limb(16, 1, 2, 3);
            g.line(20, 14, 25, 9, WOOD);
            g.rect(24, 5, 5, 5, STEEL);
            g.rect(24, 9, 5, 1, STEEL_DARK);
        } else {
            p.arm_r = limb(16, 1, 2, 3);
            g.rect(21, 17, 6, 1, WOOD);
            g.rect(27, 12, 5, 8, STEEL);
            g.rect(31, 12, 1, 8, STEEL_DARK);
        }
        critter(&mut g, bx, by, &p);

        // sparks around the strike point
        let dirs = [(-3, -2), (-1, -3), (2, -3), (3, -1), (4, -3), (-2, -1)];
        if spark == 1 {
            g.glyph(31, 14, &["#...#", ".#.#.", ".....", ".#.#.", "#...#"], c(0xfff6d0));
        } else if spark >= 2 {
            let k = if spark == 2 { 2 } else { 3 };
            let sc = if spark == 2 { c(0xffe070) } else { c(0xd08030) };
            for (dx, dy) in dirs {
                g.px(33 + dx * k / 2 + if dx > 0 { k } else { 0 }, 18 + dy * k, sc);
            }
        }
        a.push(&g, delay);
    }
    a
}

#[allow(clippy::too_many_arguments)]
fn gear(g: &mut Grid, cx: f64, cy: f64, r_out: f64, r_in: f64, teeth: i32, rot: f64, col: Color, dark: Color) {
    for y in (cy - r_out) as i32 - 1..=(cy + r_out) as i32 + 1 {
        for x in (cx - r_out) as i32 - 1..=(cx + r_out) as i32 + 1 {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let r = dx.hypot(dy);
            if r > r_out {
                continue;
            }
            let tooth = (teeth as f64 * (dy.atan2(dx) - rot)).cos() > 0.1;
            if r > r_in && !tooth {
                continue;
            }
            if r < 1.3 {
                g.px(x, y, INK);
            } else if r < r_in * 0.55 {
                g.px(x, y, dark);
            } else {
                g.px(x, y, col);
            }
        }
    }
}

fn work_gears() -> Scene {
    let mut a = Scene::default();
    let n = 12;
    for f in 0..n {
        let mut g = Grid::new(c(0x161a24));
        g.rect(0, 30, GW, 2, c(0x262c3a));
        let t = f as f64 / n as f64;
        gear(&mut g, 29.5, 13.5, 8.6, 6.4, 8, t * 2.0 * (2.0 * PI / 8.0), c(0xe2b048), c(0xa87828));
        gear(&mut g, 16.5, 6.5, 5.6, 3.8, 6, -t * 2.0 * (2.0 * PI / 6.0) + 0.3, c(0xaab4c8), c(0x6e788c));
        gear(&mut g, 36.5, 26.5, 4.6, 3.0, 5, -t * (2.0 * PI / 5.0) * 2.0, c(0xaab4c8), c(0x6e788c));
        // the critter turns the big gear with a wrench on its axle
        let push = (f / 3) % 2 != 0; // hand low / high
        let by = 15 + if push { -1 } else { 0 };
        let p = Pose {
            eyes: Eyes::Focus,
            look_x: 1,
            arm_l: L_DOWN,
            arm_r: if push { limb(16, 2, 2, 3) } else { limb(16, 4, 2, 3) },
            ..Pose::default()
        };
        critter(&mut g, 3, by, &p);
        let (hx, hy) = (21, by + if push { 3 } else { 5 });
        g.line(hx, hy, 27, 14, STEEL_DARK);
        g.line(hx, hy - 1, 27, 13, STEEL);
        g.sprite(26, 11, &[".SS.S", "SS...", "S....", "SS...", ".SS.S"], &[('S', STEEL)]);
        // sweat
        if push {
            g.rect(2, by - 1, 1, 2, c(0x8cd0ff));
        }
        a.push(&g, 100);
    }
    a
}

fn work_scroll() -> Scene {
    let mut a = Scene::default();
    let (paper, roll, roll_dark, text) = (c(0xf0e0b4), c(0xc8a468), c(0x8e6c3a), c(0x7a5a40));
    // text lines (start, length) scrolling up through the sheet
    let lines = [(0, 12), (0, 7), (2, 9), (0, 11), (0, 5), (3, 8)];
    let n = 12;
    for f in 0..n {
        let mut g = Grid::new(c(0x152020));
        g.rect(0, 30, GW, 2, c(0x1f2e2c));
        let scan = [-1, 0, 1, 0];
        let p = Pose {
            eyes: Eyes::Down,
            look_x: scan[(f % 4) as usize],
            arm_l: NO_LIMB,
            arm_r: NO_LIMB,
            legs: Legs::None,
            ..Pose::default()
        };
        critter(&mut g, 12, 5, &p);
        // sheet
        g.rect(10, 16, 20, 12, paper);
        for y in 16..28 {
            g.px(29, y, c(0xd8c494));
        }
        // 2px line pitch, scrolling 1px a frame; the line being read is orange
        for i in 0..8 {
            let y = 17 + i * 2 - (f % 2);
            if !(17..=26).contains(&y) {
                continue;
            }
            let (start, len) = lines[((i + f / 2) % 6) as usize];
            let reading = y == 21 || y == 22;
            g.rect(12 + start, y, len + 2, 1, if reading { ORANGE } else { text });
        }
        // rollers top and bottom with the hands holding the top one
        for ry in [14, 28] {
            g.rect(9, ry, 22, 2, roll);
            g.rect(9, ry + 1, 22, 1, roll_dark);
            g.rect(7, ry, 2, 2, roll_dark);
            g.rect(31, ry, 2, 2, roll_dark);
        }
        g.rect(9, 13, 3, 3, ORANGE);
        g.rect(28, 13, 3, 3, ORANGE);
        a.push(&g, 200);
    }
    a
}

fn work_treadmill() -> Scene {
    let mut a = Scene::default();
    let n = 8;
    for f in 0..n {
        let mut g = Grid::new(c(0x171a26));
        // speed lines
        for k in 0..3 {
            let y = 14 + k * 4;
            let x0 = ((k * 3 - f * 3) % 8 + 8) % 8 - 2;
            for i in 0..4 {
                g.blend(x0 + i, y, c(0x8fa0c8), 0.25 + i as f64 * 0.12);
            }
        }
        // console on a post
        g.line(33, 27, 35, 14, c(0x4a5064));
        g.line(34, 27, 36, 14, c(0x4a5064));
        g.rect(31, 10, 8, 5, c(0x3a4054));
        g.rect(32, 11, 6, 3, c(0x0e2a1c));
        for i in 0..6 {
            if i <= f % 6 {
                g.px(32 + i, 13 - (i % 2), c(0x5cf08a));
            }
        }
        // belt with moving stripes
        g.rect(4, 27, 30, 2, c(0x2c2f3a));
        for x in 4..34 {
            if (x + f * 2) % 6 < 2 {
                g.px(x, 27, c(0x585e70));
            }
        }
        g.disc(4.5, 28.0, 1.6, c(0x6a7084));
        g.disc(33.5, 28.0, 1.6, c(0x6a7084));
        g.rect(5, 30, 2, 2, c(0x4a5064));
        g.rect(30, 30, 2, 2, c(0x4a5064));

        let odd = f % 2 != 0;
        let by = 11 + if odd { -1 } else { 0 };
        let p = Pose {
            eyes: Eyes::Focus,
            look_x: 1,
            legs: if odd { Legs::RunA } else { Legs::RunB },
            arm_l: if odd { limb(-2, 3, 2, 3) } else { limb(-2, 6, 2, 3) },
            arm_r: if odd { limb(16, 6, 2, 3) } else { limb(16, 3, 2, 3) },
            mouth: Mouth::O,
            ..Pose::default()
        };
        critter(&mut g, 10, by + 1, &p);
        // sweat drop flying off
        let drop = [(11, 11), (9, 9), (7, 9), (5, 11)];
        let (dx, dy) = drop[(f % 4) as usize];
        g.rect(dx, dy, 1, 2, c(0x8cd0ff));
        a.push(&g, 90);
    }
    a
}

fn work_juggle() -> Scene {
    let mut a = Scene::default();
    let n = 12;
    let items: [([&str; 5], Color); 3] = [
        ([".##", ".#.", "##.", ".#.", ".##"], c(0x7ad4f4)), // {
        (["#..", ".#.", "..#", ".#.", "#.."], c(0xa4e47c)), // >
        (["##.", ".#.", ".##", ".#.", "##."], c(0xffd25a)), // }
    ];
    for f in 0..n {
        let mut g = Grid::new(c(0x1a1528));
        g.rect(0, 31, GW, 1, c(0x2a2440));
        let mut pos = [(0, 0); 3];
        let mut tt = [0.0; 3];
        let (mut top_x, mut top_y) = (19, 99);
        for k in 0..3 {
            let t = ((f + k as i32 * 4) % n) as f64 / n as f64;
            tt[k] = t;
            let (x, y) = if t < 0.75 {
                // high arc from the right hand to the left one
                let th = t / 0.75 * PI;
                (19.0 + 10.0 * th.cos(), 13.0 - 11.0 * th.sin())
            } else {
                // quick low pass back to the right hand
                let u = (t - 0.75) / 0.25;
                (9.0 + 20.0 * u, 13.0 - 2.0 * (u * PI).sin())
            };
            pos[k] = (qround(x) - 1, qround(y) - 2);
            if pos[k].1 < top_y {
                (top_x, top_y) = pos[k];
            }
        }
        let left_catch = tt.iter().any(|t| (t - 0.75).abs() < 0.05);
        let right_throw = tt.iter().any(|&t| t < 0.05);
        let p = Pose {
            eyes: Eyes::Up,
            look_x: if top_x < 16 {
                -1
            } else if top_x > 22 {
                1
            } else {
                0
            },
            arm_l: if left_catch { limb(-2, 0, 2, 4) } else { limb(-2, 4, 2, 3) },
            arm_r: if right_throw { limb(16, 0, 2, 4) } else { limb(16, 4, 2, 3) },
            mouth: Mouth::Smile,
            ..Pose::default()
        };
        critter(&mut g, 11, 16, &p);
        for (k, (rows, col)) in items.iter().enumerate() {
            g.glyph(pos[k].0, pos[k].1, rows, *col);
        }
        a.push(&g, 110);
    }
    a
}

fn work_progress() -> Scene {
    let mut a = Scene::default();
    let n = 12;
    let cloud = c(0xe8ecf4);
    for f in 0..n {
        let mut g = Grid::new(c(0x121826));
        g.rect(0, 31, GW, 1, c(0x222a3c));
        let done = f >= 9;
        let p = Pose {
            eyes: if done { Eyes::Happy } else { Eyes::Up },
            look_x: 1,
            arm_l: if done { L_UP } else { L_DOWN },
            arm_r: if done { R_UP } else { limb(16, 7, 2, 2) },
            mouth: if done { Mouth::Smile } else { Mouth::None },
            ..Pose::default()
        };
        critter(&mut g, 3, 16 + if done && f % 2 != 0 { -1 } else { 0 }, &p);
        // thought bubble
        g.px(21, 15, cloud);
        g.rect(23, 12, 2, 2, cloud);
        let puffs = [(25.0, 6.5, 4.0), (31.0, 5.0, 4.5), (36.0, 7.0, 3.6), (29.5, 8.0, 4.0)];
        for (x, y, r) in puffs {
            g.disc(x, y, r, cloud);
        }
        if !done {
            g.rect(23, 5, 14, 4, c(0x3a4058));
            let w = qround(12.0 * (f + 1) as f64 / 9.0);
            g.rect(24, 6, w, 2, ORANGE);
        } else {
            g.glyph(26, 3, &["......##", ".....##.", "##..##..", ".####...", "..##...."], c(0x3cb060));
        }
        a.push(&g, if done { 240 } else { 150 });
    }
    a
}

// ------------------------------------------------------------------------------------------ alerting

fn alert_wave() -> Scene {
    let mut a = Scene::default();
    let n = 8;
    for f in 0..n {
        let mut g = Grid::new(if f % 2 != 0 { c(0x2e2208) } else { c(0x3a2a0a) });
        g.rect(0, 30, GW, 2, c(0x4a3610));
        let left_out = (f / 2) % 2 == 0;
        let (bx, by) = (12, 15 + if f % 2 != 0 { -1 } else { 0 });
        let p = Pose {
            eyes: Eyes::Wide,
            arm_l: if left_out { NO_LIMB } else { limb(-2, -2, 2, 6) },
            arm_r: if left_out { limb(16, -2, 2, 6) } else { NO_LIMB },
            ..Pose::default()
        };
        critter(&mut g, bx, by, &p);
        arm_out(&mut g, bx, by, left_out);
        // motion marks next to the waving hand
        let (hx, dir) = if left_out { (bx - 7, -1) } else { (bx + 22, 1) };
        g.px(hx, by - 2, WHITE);
        g.px(hx + dir, by - 3, WHITE);
        g.px(hx, by + 1, WHITE);
        g.px(hx + dir, by + 2, WHITE);
        // big exclamation mark with rays
        let mark = if f % 4 < 2 { c(0xffd23a) } else { WHITE };
        g.glyph(18, 1, &["####", "####", "####", "####", "####", ".##.", ".##.", "....", ".##.", ".##."], mark);
        if f % 2 == 0 {
            let ray = c(0xffb020);
            g.line(13, 2, 15, 3, ray);
            g.line(26, 2, 24, 3, ray);
            g.line(13, 8, 15, 7, ray);
            g.line(26, 8, 24, 7, ray);
        }
        a.push(&g, 150);
    }
    a
}

fn alert_bell() -> Scene {
    let mut a = Scene::default();
    let angles: [f64; 8] = [0.0, 0.28, 0.45, 0.28, 0.0, -0.28, -0.45, -0.28];
    let (gold, gold_light, gold_dark, clapper) = (c(0xf2c040), c(0xffe890), c(0xc08a24), c(0x5a3e1c));
    let (px0, py0) = (20.0, 1.0);
    for (f, an) in angles.into_iter().enumerate() {
        let strike = f == 2 || f == 6;
        let mut g = Grid::new(if strike { c(0x3e1a12) } else { c(0x2a1410) });
        g.rect(0, 31, GW, 1, c(0x40221a));
        g.rect(8, 0, 24, 2, c(0x5a3a26));
        let (ca, sa) = (an.cos(), an.sin());
        for y in 0..20 {
            for x in 0..GW {
                let dx = x as f64 + 0.5 - px0;
                let dy = y as f64 + 0.5 - py0;
                let u = dx * ca - dy * sa;
                let v = dx * sa + dy * ca;
                let half = if (0.0..2.0).contains(&v) {
                    1.0
                } else if (2.0..4.0).contains(&v) {
                    2.5 + (v - 2.0)
                } else if (4.0..11.0).contains(&v) {
                    4.5 + (v - 4.0) * 0.33
                } else if (11.0..13.0).contains(&v) {
                    7.0 + (v - 11.0) * 0.8
                } else if (13.0..14.2).contains(&v) {
                    8.6
                } else {
                    -1.0
                };
                if half > 0.0 && u.abs() <= half {
                    let col = if v >= 13.0 {
                        gold_dark
                    } else if u < -half + 2.2 && v > 3.0 {
                        gold_light
                    } else if u > half - 2.2 {
                        gold_dark
                    } else {
                        gold
                    };
                    g.px(x, y, col);
                } else if u.hypot(v - 14.8) <= 1.7 {
                    g.px(x, y, clapper);
                }
            }
        }
        // "ding" waves on the side the bell swings to
        let arcs = |g: &mut Grid, side: f64, phase: i32| {
            for r in 0..2 {
                let rad = (12 + phase * 2 + r * 3) as f64;
                let mut t = -0.55;
                while t <= 0.55 {
                    let x = qround(20.0 + side * rad * f64::cos(t));
                    let y = qround(9.0 + rad * f64::sin(t));
                    g.px(x, y, if r == 0 { c(0xffe070) } else { c(0xff9a40) });
                    t += 0.04;
                }
            }
        };
        match f {
            2 => arcs(&mut g, 1.0, 0),
            3 => arcs(&mut g, 1.0, 1),
            6 => arcs(&mut g, -1.0, 0),
            7 => arcs(&mut g, -1.0, 1),
            _ => {}
        }
        let p = Pose {
            eyes: if strike { Eyes::Closed } else { Eyes::Wide },
            arm_l: L_UP,
            arm_r: R_UP,
            mouth: if strike { Mouth::Shout } else { Mouth::None },
            ..Pose::default()
        };
        critter(&mut g, 12, 17, &p);
        a.push(&g, if strike { 160 } else { 110 });
    }
    a
}

fn alert_siren() -> Scene {
    let mut a = Scene::default();
    let n = 8;
    let (red, blue) = (c(0xff3030), c(0x3080ff));
    let (cx, cy) = (20.0, 11.0);
    for f in 0..n {
        let mut g = Grid::new(if f % 2 != 0 { c(0x140e1c) } else { c(0x1c0e14) });
        g.rect(0, 31, GW, 1, c(0x2a2030));
        // two rotating beams
        let base = (f * 2) as f64 * PI / n as f64;
        for y in 0..GH {
            for x in 0..GW {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                let r = dx.hypot(dy);
                if r < 3.0 {
                    continue;
                }
                let th = dy.atan2(dx);
                for b in 0..2 {
                    let d = ieee_remainder(th - (base + b as f64 * PI), 2.0 * PI);
                    if d.abs() < 0.32 {
                        g.blend(x, y, if b != 0 { blue } else { red }, 0.55 * (1.0 - r / 34.0));
                    }
                }
            }
        }
        let odd = f % 2 != 0;
        let p = Pose {
            eyes: Eyes::Wide,
            arm_l: if odd { L_UP } else { L_DOWN },
            arm_r: if odd { R_DOWN } else { R_UP },
            ..Pose::default()
        };
        let jitter = match f % 4 {
            1 => 1,
            3 => -1,
            _ => 0,
        };
        critter(&mut g, 12 + jitter, 16, &p);
        // the light itself on the critter's head
        let sx = 16 + jitter;
        g.rect(sx - 1, 14, 10, 2, c(0x707888));
        let red_on = f % 2 == 0;
        g.sprite(
            sx,
            9,
            &["..LLRR..", ".LLLRRR.", "LLLLRRRR", "LLLLRRRR", "LLLLRRRR"],
            &[('L', if red_on { red } else { c(0x701818) }), ('R', if red_on { c(0x183870) } else { blue })],
        );
        g.rect(sx + (f * 2) % 8, 10, 1, 3, WHITE);
        a.push(&g, 110);
    }
    a
}

fn alert_sign() -> Scene {
    let mut a = Scene::default();
    let n = 8;
    let sway = [0, 0, 1, 1, 0, 0, -1, -1];
    let bob = [0, -1, -1, 0, 0, -1, -1, 0];
    for f in 0..n {
        let fi = f as usize;
        let mut g = Grid::new(c(0x231a35));
        g.rect(0, 31, GW, 1, c(0x33284a));
        // small question marks drifting up on the right
        for k in 0..2 {
            let ph = (f + k * 4) % 8;
            let qc = mix(c(0xc8a0ff), c(0x231a35), ph as f64 / 8.0);
            g.glyph(33 + k * 3, 24 - ph * 2 - k * 3, &["##.", "..#", ".#.", "...", ".#."], qc);
        }
        let p = Pose {
            eyes: if f == 5 { Eyes::Closed } else { Eyes::Open },
            look_x: 1,
            arm_r: limb(16, 1 + bob[fi], 2, 5),
            mouth: Mouth::O,
            ..Pose::default()
        };
        critter(&mut g, 3, 16, &p);
        // sign on a stick held up high
        let (ox, oy) = (13 + sway[fi], 1 + bob[fi]);
        g.line(20, 17 + bob[fi], 20 + sway[fi], oy + 12, WOOD);
        g.rect(ox, oy, 15, 12, c(0x5a5070));
        g.rect(ox + 1, oy + 1, 13, 10, WHITE);
        g.glyph(
            ox + 4,
            oy + 1,
            &[".####.", "##..##", "....##", "...##.", "..##..", "......", "..##..", "..##.."],
            c(0xd9572f),
        );
        a.push(&g, 170);
    }
    a
}

fn alert_knock() -> Scene {
    let mut a = Scene::default();
    // 0 rest, 1 pulled back, 2 knock
    let seq = [0, 1, 2, 1, 2, 0, 0, 0, 0, 0];
    let delays = [200, 90, 140, 90, 160, 200, 200, 120, 200, 200];
    let txt = WHITE;
    for f in 0..10 {
        let s = seq[f];
        let shake = if s == 2 { 1 } else { 0 };
        let mut g = Grid::new(c(0x180f12));
        // huge close-up face
        g.rect(4 + shake, 5, 32, 27, ORANGE);
        g.rect(3 + shake, 7, 34, 25, ORANGE);
        g.rect(5 + shake, 4, 30, 1, ORANGE);
        let blink = f == 7;
        for ex in [10, 25] {
            if blink {
                g.rect(ex - 1 + shake, 16, 6, 2, INK);
            } else {
                g.rect(ex + shake, 12, 4, 7, INK);
                g.rect(ex + 1 + shake, 13, 1, 2, WHITE);
            }
        }
        // worried brows
        g.glyph(9 + shake, 8, &["....##", "..##..", "##...."], INK);
        g.glyph(25 + shake, 8, &["##....", "..##..", "....##"], INK);
        g.rect(18 + shake, 22, 4, 3, INK);
        g.rect(19 + shake, 23, 2, 2, c(0x8a2a20));
        g.rect(6 + shake, 20, 3, 1, BLUSH);
        g.rect(31 + shake, 20, 3, 1, BLUSH);
        // fist knocking on the glass from the inside
        if s == 1 {
            g.sprite(
                29,
                22,
                &[".DDDDDD.", "DOOOOOOD", "DODODODD", "DOOOOOOD", "DOOOOOOD", "DOOOOOOD", ".DOOOOD.", ".DOOOOD."],
                &[('D', ORANGE_DARK), ('O', c(0xe8906c))],
            );
        } else if s == 2 {
            g.sprite(
                25,
                18,
                &[
                    "..DDDDDDDD..",
                    ".DOOOOOOOOD.",
                    "DOOODOODOOOD",
                    "DOODOODOODOD",
                    "DOOOOOOOOOOD",
                    "DOOOOOOOOOOD",
                    "DOOOOOOOOOOD",
                    ".DOOOOOOOOD.",
                    "..DOOOOOOD..",
                    "..DOOOOOOD..",
                    "..DOOOOOOD..",
                    "..DOOOOOOD..",
                ],
                &[('D', ORANGE_DARK), ('O', c(0xf0a07c))],
            );
            // impact marks and "TUK"
            g.line(22, 16, 23, 17, WHITE);
            g.line(24, 14, 25, 16, WHITE);
            g.line(37, 16, 36, 17, WHITE);
            g.line(35, 14, 34, 16, WHITE);
            g.glyph(
                if f < 4 { 8 } else { 21 },
                0,
                &["###.#.#.#.#", ".#..#.#.##.", ".#..###.#..", ".#....#.##.", ".#..##..#.#"],
                txt,
            );
        }
        // reflection on the screen glass
        for i in 0..6 {
            g.blend(1 + i, 6 - i, WHITE, 0.35);
            g.blend(1 + i, 9 - i, WHITE, 0.2);
        }
        a.push(&g, delays[f]);
    }
    a
}

// ------------------------------------------------------------------------------------------ chilling

fn chill_coffee() -> Scene {
    let mut a = Scene::default();
    let n = 12;
    let (mug, mug_shade, coffee) = (c(0xf2ece0), c(0xcfc6b6), c(0x6b3e22));
    for f in 0..n {
        let mut g = Grid::new(c(0x241914));
        // rainy window
        g.rect(2, 2, 11, 11, c(0x5a3e30));
        g.rect(3, 3, 9, 9, c(0x1a2440));
        for (dx, d1) in [(4, 0), (6, 5), (9, 2), (11, 7)] {
            let y = 3 + (d1 + f) % 9;
            g.px(dx, y, c(0x7aa0d0));
            if y > 3 {
                g.blend(dx, y - 1, c(0x7aa0d0), 0.4);
            }
        }
        g.rect(7, 3, 1, 9, c(0x5a3e30));
        g.rect(3, 7, 9, 1, c(0x5a3e30));
        g.rect(1, 13, 13, 1, c(0x7a5644));
        // rug
        g.rect(0, 27, GW, 5, c(0x3a2a22));
        for x in (1..GW).step_by(4) {
            g.px(x, 29, c(0x6a3a2a));
        }

        let sip = (8..=10).contains(&f);
        let p = Pose {
            eyes: if sip { Eyes::Closed } else { Eyes::Relaxed },
            legs: Legs::Short,
            blush: true,
            arm_r: if sip { limb(16, 3, 2, 2) } else { limb(16, 7, 2, 2) },
            ..Pose::default()
        };
        critter(&mut g, 12, 14, &p);
        let my = if sip { 13 } else { 18 };
        g.rect(29, my + 1, 1, 3, mug_shade); // handle in the hand
        g.rect(30, my, 6, 7, mug);
        g.rect(35, my, 1, 7, mug_shade);
        g.rect(30, my + 3, 6, 1, ORANGE);
        g.rect(31, my, 4, 1, coffee);
        // steam: short wisps rising and fading
        if !sip {
            for w in 0..3 {
                let ph = (f + w * 4) % n;
                let yb = my - 2 - ph;
                for y in yb - 3..=yb {
                    if y < 2 {
                        continue;
                    }
                    let x = 31 + (w % 2) * 3 + qround((y as f64 * 0.9 + w as f64).sin() * 1.0);
                    g.blend(x, y, WHITE, 0.9 * (1.0 - ph as f64 / n as f64));
                }
            }
        }
        a.push(&g, if sip { 320 } else { 200 });
    }
    a
}

fn chill_fishing() -> Scene {
    let mut a = Scene::default();
    let n = 12;
    let bob = [0, 0, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0];
    let (water, wave, sun_c) = (c(0x1d3557), c(0x3a6494), c(0xffb060));
    for f in 0..n {
        let fi = f as usize;
        let mut g = Grid::new(c(0x24204a));
        g.vgrad(0, 21, c(0x24204a), c(0x8a4a5a));
        for (sx, sy) in [(4, 3), (14, 2), (22, 5), (37, 2)] {
            g.px(sx, sy, if (f + sx) % 4 != 0 { c(0xd8d0ff) } else { c(0x6a5a90) });
        }
        g.disc(27.0, 21.5, 5.5, sun_c);
        g.rect(0, 21, GW, 11, water);
        // sun glitter on the water
        for r in 0..3 {
            let w = 8 - r * 2 + if (f + r) % 3 == 0 { 1 } else { 0 };
            g.rect(27 - w / 2, 23 + r * 2, w, 1, mix(sun_c, water, 0.3 + r as f64 * 0.2));
        }
        // drifting wavelets
        for k in 0..6 {
            let x = ((k * 7 + f) % 42) - 1;
            let y = 22 + (k * 3) % 9;
            g.rect(x, y, 2, 1, wave);
        }
        let p =
            Pose { eyes: Eyes::Relaxed, arm_r: limb(16, 6, 2, 2), legs: Legs::None, blush: true, ..Pose::default() };
        critter(&mut g, 2, 8, &p);
        // dock with the legs dangling over its edge
        g.rect(1, 23, 1, 8, c(0x3a2416));
        g.rect(19, 23, 1, 8, c(0x3a2416));
        g.rect(0, 20, 21, 2, c(0x9a6a40));
        g.rect(0, 22, 21, 1, c(0x5a3a22));
        for x in [5, 10, 15] {
            g.px(x, 20, c(0x6a4428));
        }
        let swing = (f / 3) % 2;
        g.rect(11, 22, 2, 2 + swing, ORANGE_DARK);
        g.rect(15, 22, 2, 3 - swing, ORANGE_DARK);
        // rod, line, bobber
        g.line(19, 14, 33, 4, c(0xb08050));
        let by = 21 + bob[fi];
        g.line(33, 4, 34, by, c(0xc8d0dc));
        g.rect(34, by, 2, 1, c(0xff3a2a));
        g.rect(34, by + 1, 2, 1, WHITE);
        if bob[fi] == 1 || (f > 0 && bob[fi - 1] == 1) {
            let r = if bob[fi] == 1 { 2 } else { 4 };
            g.rect(33 - r, 23, 2, 1, c(0x8ab0d8));
            g.rect(35 + r, 23, 2, 1, c(0x8ab0d8));
        }
        a.push(&g, 230);
    }
    a
}

fn chill_beach() -> Scene {
    let mut a = Scene::default();
    let n = 8;
    let crab = [0, 1, 2, 3, 4, 3, 2, 1];
    for f in 0..n {
        let mut g = Grid::new(c(0x5cb8e8));
        g.vgrad(0, 16, c(0x4aa8e4), c(0xa8dcf4));
        // sun with alternating rays
        g.disc(34.0, 5.0, 3.6, c(0xfff070));
        let ray = c(0xffe040);
        if f % 2 == 0 {
            for (dx, dy) in [(0, -6), (0, 6), (-6, 0), (6, 0)] {
                g.rect(
                    34 + dx - if dx >= 0 { 0 } else { 1 },
                    5 + dy - if dy < 0 { 1 } else { 0 },
                    if dx == 0 { 1 } else { 2 },
                    if dy == 0 { 1 } else { 2 },
                    ray,
                );
            }
        } else {
            for (dx, dy) in [(-1, -1), (1, -1), (-1, 1), (1, 1)] {
                g.px(34 + dx * 5, 5 + dy * 5, ray);
                g.px(34 + dx * 6, 5 + dy * 6, ray);
            }
        }
        // sea and sand
        g.rect(0, 17, GW, 5, c(0x2a7ec8));
        for x in 0..GW {
            if (x + f) % 8 < 2 {
                g.px(x, 17, WHITE);
            }
            if (x - f + 40) % 10 < 2 {
                g.px(x, 19, c(0x5aa4e0));
            }
        }
        g.rect(0, 22, GW, 10, c(0xf2d496));
        for k in 0..9 {
            g.px((k * 11 + 3) % GW, 24 + (k * 5) % 7, c(0xd8b878));
        }
        // umbrella
        g.rect(8, 7, 1, 21, c(0xeee6d8));
        for y in 2..=8 {
            let half = 9.0 * f64::max(0.0, 1.0 - ((8.5 - y as f64) / 6.5).powf(2.0)).sqrt();
            for x in qround(8.5 - half)..qround(8.5 + half) {
                g.px(x, y, if ((x + 40) / 3) % 2 != 0 { c(0xe04a3a) } else { WHITE });
            }
        }
        // towel
        g.rect(11, 27, 25, 2, c(0x38b0a8));
        for x in (13..36).step_by(4) {
            g.rect(x, 27, 2, 2, WHITE);
        }

        let p = Pose {
            eyes: Eyes::Shades,
            mouth: Mouth::Smile,
            arm_r: limb(16, 7, 2, 2),
            legs: Legs::Short,
            ..Pose::default()
        };
        critter(&mut g, 15, 14, &p);
        // glint sliding over the sunglasses
        if f < 3 {
            g.px(15 + 4 + f * 4, 14 + 5, c(0xd0f0ff));
        }
        // juice with a straw
        g.rect(33, 18, 4, 6, c(0xd8f0f8));
        g.rect(34, 20, 2, 3, c(0xffa020));
        g.line(36, 15, 35, 19, c(0xff4a6a));
        // crab scuttling sideways
        let cx = 2 + crab[f as usize];
        g.sprite(cx, 28, &["R...R", ".RRR.", if f % 2 != 0 { "R.R.R" } else { ".R.R." }], &[('R', c(0xe04030))]);
        g.px(cx + 1, 28, INK);
        a.push(&g, 260);
    }
    a
}

fn chill_cloud() -> Scene {
    let mut a = Scene::default();
    let n = 16;
    let (cloud, cloud_shade) = (c(0xfafcff), c(0xc8d8f0));
    for f in 0..n {
        let mut g = Grid::new(c(0x4a8ad8));
        g.vgrad(0, 31, c(0x4a86d6), c(0xb0d8f6));
        // distant clouds drifting left (pattern repeats every 20px, so the loop is seamless)
        for k in 0..2 {
            for row in 0..2 {
                let x = ((k * 20 + row * 9 - f * 20 / n) % 40 + 40) % 40 - 4;
                let y = 3 + row * 9;
                for dx in [0, 40] {
                    g.rect(x + 2 - dx, y, 3, 1, mix(cloud, c(0x6a9ade), 0.35));
                    g.rect(x - dx, y + 1, 7, 1, mix(cloud, c(0x6a9ade), 0.35));
                }
            }
        }
        let bob = qround(((f * 2) as f64 * PI / n as f64).sin() * 1.2);
        let p = Pose {
            eyes: Eyes::Relaxed,
            mouth: Mouth::Smile,
            blush: true,
            arm_l: limb(-3, 8, 3, 2),
            arm_r: limb(16, 8, 3, 2),
            legs: Legs::None,
            ..Pose::default()
        };
        critter(&mut g, 12, 5 + bob, &p);
        // the big fluffy cloud
        let puffs = [
            (9.0, 24.0, 4.5),
            (16.0, 21.0, 5.5),
            (24.0, 21.0, 5.5),
            (31.0, 24.0, 4.5),
            (20.0, 25.0, 5.0),
            (5.0, 26.0, 3.2),
            (35.0, 26.5, 3.0),
        ];
        let bob = bob as f64;
        for (x, y, r) in puffs {
            g.disc(x, y + bob, r, cloud_shade);
        }
        for (x, y, r) in puffs {
            g.disc(x - 0.5, y + bob - 1.0, r - 0.4, cloud);
        }
        // a note floating up
        let ph = f % 8;
        let note = mix(c(0x3a4a8a), c(0x8ab4ea), ph as f64 / 8.0);
        g.glyph(31 + ph / 3, 11 - ph, &["..##", "..#.", "..#.", "###.", "##.."], note);
        a.push(&g, 220);
    }
    a
}

fn chill_bath() -> Scene {
    let mut a = Scene::default();
    let n = 12;
    let (yellow, beak, tub, tub_shade) = (c(0xffd83a), c(0xff8a20), c(0xeef3f6), c(0xb8c6d0));
    for f in 0..n {
        let mut g = Grid::new(c(0x3e7c8c));
        for y in (0..GH).step_by(6) {
            g.rect(0, y, GW, 1, c(0x356c7a));
        }
        for y in (0..GH).step_by(6) {
            let x0 = if (y / 6) % 2 != 0 { 3 } else { 0 };
            for x in (x0..GW).step_by(6) {
                g.rect(x, y, 1, 6, c(0x356c7a));
            }
        }
        g.rect(0, 29, GW, 3, c(0x2c5866));

        let p = Pose {
            eyes: Eyes::Happy,
            mouth: Mouth::Smile,
            blush: true,
            arm_l: limb(-2, 7, 2, 2),
            arm_r: limb(16, 7, 2, 2),
            legs: Legs::None,
            ..Pose::default()
        };
        critter(&mut g, 12, 5 + if f % 6 < 3 { 0 } else { 1 }, &p);
        // tub
        g.rect(3, 19, 34, 2, WHITE);
        g.rect(4, 21, 32, 5, tub);
        g.rect(5, 26, 30, 1, tub);
        g.rect(7, 27, 26, 1, tub_shade);
        g.rect(4, 25, 32, 1, tub_shade);
        g.rect(7, 28, 2, 2, c(0xe0b040));
        g.rect(31, 28, 2, 2, c(0xe0b040));
        // foam along the rim
        for k in 0..9 {
            let x = 5.0 + k as f64 * 3.7;
            let y = 18.0 + (k % 2) as f64 * 0.6;
            g.disc(x, y, 2.2 + (k % 3) as f64 * 0.4, if k % 2 != 0 { WHITE } else { c(0xe2eef8) });
        }
        // rubber duck bobbing on the foam
        let dy = 11 + if f % 4 < 2 { 0 } else { 1 };
        g.sprite(
            31,
            dy,
            &["..YYY...", "BBYKY...", "..YYY..Y", ".YYYYYYY", ".YYYYYY.", "..YYYY.."],
            &[('Y', yellow), ('B', beak), ('K', INK)],
        );
        // bubbles rising and popping
        for (bx, b1) in [(6, 0), (11, 5), (26, 3), (35, 8)] {
            let ph = (f + b1) % n;
            let y = 15 - ph;
            if ph >= 10 {
                continue;
            }
            let x = bx + (ph / 2) % 2;
            if ph == 9 {
                g.glyph(x - 1, y - 1, &["#.#", "...", "#.#"], c(0xd8f0ff));
            } else {
                g.glyph(x - 1, y - 1, &[".#.", "#.#", ".#."], c(0xd8f0ff));
            }
        }
        a.push(&g, 220);
    }
    a
}

// ------------------------------------------------------------------------------------------ registry

struct Variant {
    state: ClaudeState,
    id: &'static str,
    /// catalog key of the display title
    title: &'static str,
    key_frame: usize,
    make: fn() -> Scene,
}

const fn v(
    state: ClaudeState,
    id: &'static str,
    title: &'static str,
    key_frame: usize,
    make: fn() -> Scene,
) -> Variant {
    Variant { state, id, title, key_frame, make }
}

const VARIANTS: [Variant; 19] = {
    use ClaudeState::{Alerting as A, Chilling as C, Working as W};
    [
        v(W, "classic", "faces.scene.working.classic", 2, working),
        v(W, "hammer", "faces.scene.working.hammer", 2, work_hammer),
        v(W, "gears", "faces.scene.working.gears", 1, work_gears),
        v(W, "scroll", "faces.scene.working.scroll", 0, work_scroll),
        v(W, "treadmill", "faces.scene.working.treadmill", 1, work_treadmill),
        v(W, "juggle", "faces.scene.working.juggle", 1, work_juggle),
        v(W, "progress", "faces.scene.working.progress", 5, work_progress),
        v(A, "classic", "faces.scene.alerting.classic", 2, alerting),
        v(A, "wave", "faces.scene.alerting.wave", 0, alert_wave),
        v(A, "bell", "faces.scene.alerting.bell", 2, alert_bell),
        v(A, "siren", "faces.scene.alerting.siren", 1, alert_siren),
        v(A, "sign", "faces.scene.alerting.sign", 0, alert_sign),
        v(A, "knock", "faces.scene.alerting.knock", 2, alert_knock),
        v(C, "classic", "faces.scene.chilling.classic", 3, chilling),
        v(C, "coffee", "faces.scene.chilling.coffee", 2, chill_coffee),
        v(C, "fishing", "faces.scene.chilling.fishing", 2, chill_fishing),
        v(C, "beach", "faces.scene.chilling.beach", 0, chill_beach),
        v(C, "cloud", "faces.scene.chilling.cloud", 4, chill_cloud),
        v(C, "bath", "faces.scene.chilling.bath", 2, chill_bath),
    ]
};

/// The variant, or the first ("classic") one of the state when the id is unknown.
fn find(state: ClaudeState, variant: &str) -> &'static Variant {
    let mut of_state = VARIANTS.iter().filter(|v| v.state == state);
    let first = of_state.clone().next().expect("every state has variants");
    of_state.find(|v| v.id == variant).unwrap_or(first)
}

/// Variant ids of a state in table order; the first is "classic".
pub fn variants(state: ClaudeState) -> &'static [&'static str] {
    match state {
        ClaudeState::Working => &["classic", "hammer", "gears", "scroll", "treadmill", "juggle", "progress"],
        ClaudeState::Alerting => &["classic", "wave", "bell", "siren", "sign", "knock"],
        ClaudeState::Chilling => &["classic", "coffee", "fishing", "beach", "cloud", "bath"],
    }
}

/// Short display title («Ноутбук», «Кузнец», …) in the interface language; empty for an unknown variant.
pub fn variant_title(state: ClaudeState, variant: &str) -> String {
    let v = find(state, variant);
    if v.id == variant { tr!(v.title).to_string() } else { String::new() }
}

/// The most telling frame, for still previews.
pub fn key_frame(state: ClaudeState, variant: &str) -> usize {
    find(state, variant).key_frame
}

/// Unknown variant → the classic one.
pub fn generate(state: ClaudeState, variant: &str) -> Scene {
    (find(state, variant).make)()
}

// ------------------------------------------------------------------------------------------ caption

const CAPTION_MARGIN: i32 = 4;

fn caption_line(c: &mut Canvas, y: i32, text: &str, color: Color) {
    let max = WIDTH as i32 - 2 * CAPTION_MARGIN;
    let caps = text.to_uppercase();
    if crate::pixelfont::covers(&caps) {
        let line = crate::pixelfont::elided(&caps, 0, max);
        crate::pixelfont::draw(c, CAPTION_MARGIN, y, &line, color, 0, 1);
    } else {
        // like the original: DejaVu Sans Bold 9 px, the text as is, vertically centred in the line
        let font = FontSpec::bold(9.0);
        let rect = r(CAPTION_MARGIN as f32, y as f32, max as f32, crate::pixelfont::HEIGHT as f32);
        c.text(rect, Align::LEFT, &Canvas::elide(text, font, max as f32), font, color);
    }
}

/// Draws the alert caption (§11.5) onto every frame: dark strip of 23 px at the bottom, line 1
/// in the accent colour 20% lighter, line 2 white, uppercased, 5×7 pixel font with "…"
/// eliding at 152 px, DejaVu Sans 9 px bold for characters the pixel font lacks.
pub fn draw_caption(frames: &[Frame], line1: &str, line2: &str) -> Vec<Frame> {
    let line_h = crate::pixelfont::HEIGHT;
    let band = 2 * line_h + 3;
    let top = HEIGHT as i32 - band;
    frames
        .iter()
        .map(|f| {
            let mut c = Canvas::from_frame(f);
            c.fill_rect(0.0, top as f32, WIDTH as f32, band as f32, Color::rgba(0, 0, 0, 200));
            caption_line(&mut c, top + 1, line1, Color::ACCENT.lighter(1.2));
            caption_line(&mut c, top + 1 + line_h, line2, Color::WHITE);
            c.to_frame()
        })
        .collect()
}

// ------------------------------------------------------------------------------------------ export

/// Nearest-neighbour ×2 of a device frame (the size of the exported previews).
fn doubled(f: &Frame) -> Vec<u8> {
    let mut v = Vec::with_capacity(WIDTH * HEIGHT * 12);
    for y in 0..HEIGHT * 2 {
        for x in 0..WIDTH * 2 {
            v.extend_from_slice(&f.pixel(x / 2, y / 2));
        }
    }
    v
}

/// Writes previews of every scene into `dir` (port of FacesPreview.cpp): `<state>-<variant>.gif`
/// at ×2 (320×256) and `sheet.png`, a contact sheet of the key frames with labels.
pub fn export_faces(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let cols = ClaudeState::ALL.iter().map(|&s| variants(s).len()).max().unwrap_or(0) as i32;
    const CELL_W: i32 = WIDTH as i32;
    const CELL_H: i32 = HEIGHT as i32;
    const LABEL: i32 = 36;
    const GAP: i32 = 12;
    const HEAD: i32 = 34;
    let rows = ClaudeState::ALL.len() as i32;
    let (sw, sh) = (GAP + cols * (CELL_W + GAP), HEAD + rows * (CELL_H + LABEL + GAP));
    let mut sheet = Canvas::new(sw as u32, sh as u32);
    sheet.fill(Color::hex(0x222024));
    let small = FontSpec::sans(11.0);
    let big = FontSpec::bold(14.0);
    sheet.text(
        r(GAP as f32, 0.0, (sw - 2 * GAP) as f32, HEAD as f32),
        Align::LEFT,
        tr!("faces.sheet.heading"),
        big,
        Color::ACCENT,
    );

    for (row, &state) in ClaudeState::ALL.iter().enumerate() {
        for (col, &id) in variants(state).iter().enumerate() {
            let scene = generate(state, id);
            let frames: Vec<Vec<u8>> = scene.frames.iter().map(doubled).collect();
            let gif = crate::gif::encode_rgb(WIDTH as u16 * 2, HEIGHT as u16 * 2, &frames, &scene.delays);
            std::fs::write(dir.join(format!("{}-{id}.gif", state.id())), gif)?;

            let x = (GAP + col as i32 * (CELL_W + GAP)) as f32;
            let y = (HEAD + row as i32 * (CELL_H + LABEL + GAP)) as f32;
            let key = key_frame(state, id).min(scene.frames.len().saturating_sub(1));
            sheet.draw_canvas(&Canvas::from_frame(&scene.frames[key]), x, y, 1.0);
            let label = trn!("faces.sheet.label", scene.frames.len() as i64, state = state.id(), id = id);
            sheet.text(
                r(x, y + CELL_H as f32 + 2.0, CELL_W as f32, 14.0),
                Align::LEFT,
                &label,
                small,
                Color::hex(0x9a949c),
            );
            let title_rect = r(x, y + CELL_H as f32 + 16.0, CELL_W as f32, 18.0);
            sheet.text(title_rect, Align::LEFT, &variant_title(state, id), big, Color::hex(0xf2eee8));
            if col == 0 {
                // the default variant
                sheet.text(title_rect, Align::RIGHT, tr!("faces.sheet.default"), small, Color::ACCENT);
            }
        }
    }
    let img = image::DynamicImage::ImageRgba8(sheet.to_image()).to_rgb8();
    img.save(dir.join("sheet.png")).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;

    /// (state, id, frames, delays) of the table in §11.3.
    fn table() -> Vec<(ClaudeState, &'static str, Vec<u32>)> {
        use ClaudeState::*;
        vec![
            (Working, "classic", vec![140; 8]),
            (Working, "hammer", vec![220, 70, 130, 110, 110, 90, 120]),
            (Working, "gears", vec![100; 12]),
            (Working, "scroll", vec![200; 12]),
            (Working, "treadmill", vec![90; 8]),
            (Working, "juggle", vec![110; 12]),
            (Working, "progress", [vec![150; 9], vec![240; 3]].concat()),
            (Alerting, "classic", vec![170; 6]),
            (Alerting, "wave", vec![150; 8]),
            (Alerting, "bell", vec![110, 110, 160, 110, 110, 110, 160, 110]),
            (Alerting, "siren", vec![110; 8]),
            (Alerting, "sign", vec![170; 8]),
            (Alerting, "knock", vec![200, 90, 140, 90, 160, 200, 200, 120, 200, 200]),
            (Chilling, "classic", vec![360; 8]),
            (Chilling, "coffee", [vec![200; 8], vec![320; 3], vec![200]].concat()),
            (Chilling, "fishing", vec![230; 12]),
            (Chilling, "beach", vec![260; 8]),
            (Chilling, "cloud", vec![220; 16]),
            (Chilling, "bath", vec![220; 12]),
        ]
    }

    #[test]
    fn scenes_follow_the_table() {
        let keys = [2, 2, 1, 0, 1, 1, 5, 2, 0, 2, 1, 0, 2, 3, 2, 2, 0, 4, 2];
        let table = table();
        assert_eq!(table.len(), 19);
        for (i, (state, id, delays)) in table.into_iter().enumerate() {
            let s = generate(state, id);
            assert_eq!(s.delays, delays, "{state:?}/{id}");
            assert_eq!(s.frames.len(), delays.len(), "{state:?}/{id}");
            assert!(s.frames.iter().all(|f| f.rgb().len() == WIDTH * HEIGHT * 3));
            assert_eq!(key_frame(state, id), keys[i], "{state:?}/{id}");
            assert!(!variant_title(state, id).is_empty());
            assert!(variants(state).contains(&id));
            // deterministic, and actually animated
            assert_eq!(generate(state, id).frames, s.frames);
            assert!(s.frames.iter().any(|f| *f != s.frames[0]), "{state:?}/{id} is static");
        }
        assert_eq!(variant_title(ClaudeState::Working, "hammer"), "Кузнец");
        assert_eq!(variant_title(ClaudeState::Working, "nope"), "");
        assert_eq!(generate(ClaudeState::Alerting, "nope").frames, generate(ClaudeState::Alerting, "classic").frames);
        let classic = generate(ClaudeState::Working, "classic");
        assert_eq!(classic.frames[0].pixel(0, 0), [0x15, 0x15, 0x24]);
    }

    #[test]
    fn matches_reference_gifs() {
        let dir = Path::new("/home/spike/projects/divoom/docs/faces");
        if !dir.is_dir() {
            eprintln!("skipped: {} not found", dir.display());
            return;
        }
        let mut mismatched = Vec::new();
        for (state, id, _) in table() {
            let path = dir.join(format!("{}-{id}.gif", state.id()));
            let file = std::fs::File::open(&path).expect("reference gif");
            let reference =
                GifDecoder::new(std::io::BufReader::new(file)).unwrap().into_frames().collect_frames().unwrap();
            let scene = generate(state, id);
            assert_eq!(reference.len(), scene.frames.len(), "{}", path.display());
            for (i, (r, f)) in reference.iter().zip(&scene.frames).enumerate() {
                let (ms, _) = r.delay().numer_denom_ms();
                assert_eq!(ms, ((scene.delays[i] + 5) / 10).max(2) * 10, "{}: delay of frame {i}", path.display());
                let buf = r.buffer();
                assert_eq!((buf.width(), buf.height()), (WIDTH as u32 * 2, HEIGHT as u32 * 2));
                let ours = doubled(f);
                let diff = buf.pixels().zip(ours.as_chunks::<3>().0).filter(|(p, q)| p.0[..3] != q[..]).count();
                if diff > 0 {
                    mismatched.push(format!("{}-{id} frame {i}: {diff} px", state.id()));
                }
            }
        }
        assert!(mismatched.is_empty(), "differs from the reference:\n{}", mismatched.join("\n"));
    }

    #[test]
    fn caption_band_and_text() {
        let frames = generate(ClaudeState::Alerting, "classic").frames;
        let out = draw_caption(&frames, "Проверка", "Разрешить Bash?");
        assert_eq!(out.len(), frames.len());
        let f = &out[0];
        // scene untouched above the band
        assert_eq!(&f.rgb()[..104 * WIDTH * 3], &frames[0].rgb()[..104 * WIDTH * 3]);
        // band darkened, text lit: accent on line 1, white on line 2
        let band: Vec<[u8; 3]> =
            (105..128).flat_map(|y| (0..WIDTH).map(move |x| (x, y))).map(|(x, y)| f.pixel(x, y)).collect();
        assert!(band.iter().all(|p| p.iter().all(|&v| v < 0x60) || *p == [255, 255, 255] || p[0] > 0xe0));
        let accent = Color::ACCENT.lighter(1.2);
        assert!((106..116).any(|y| (0..WIDTH).any(|x| f.pixel(x, y) == [accent.r, accent.g, accent.b])));
        assert!((116..126).any(|y| (0..WIDTH).any(|x| f.pixel(x, y) == [255, 255, 255])));
        assert_eq!(f.pixel(3, 110), f.pixel(2, 110)); // 4 px left margin
        // fallback font for characters missing in the pixel font
        let out = draw_caption(&frames, "проект", "日本");
        assert_eq!(out.len(), frames.len());
    }

    #[test]
    fn exports_previews() {
        let dir = std::env::temp_dir().join(format!("minitoo-faces-{}", std::process::id()));
        export_faces(&dir).unwrap();
        let gifs = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "gif"))
            .count();
        assert_eq!(gifs, 19);
        let sheet = image::open(dir.join("sheet.png")).unwrap();
        assert_eq!((sheet.width(), sheet.height()), (1216, 562));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
