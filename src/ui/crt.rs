//! CRT look for the screen of the drawn MiniToo: a fragment shader over the raw 160×128 frame.
//! Each row is a round beam (bright ones swell, dark ones leave scanlines between them), a faint
//! aperture grille, a glow around bright pixels, a slightly bulging glass and a vignette.
//!
//! Drawn by an `egui_glow` paint callback. The shader needs GLSL 1.40 / ES 3.00; where it
//! cannot be built [`available`] turns false and the screen falls back to the plain picture.

use eframe::egui_glow;
use eframe::glow::{self, HasContext as _};
use egui::{PaintCallback, Painter, Rect, TextureId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const VERTEX: &str = r#"
out vec2 v_uv;
void main() {
    vec2 p = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    v_uv = vec2(p.x, 1.0 - p.y);
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
uniform sampler2D u_tex;
// the viewport in physical pixels
uniform vec2 u_out;
in vec2 v_uv;
out vec4 f_color;

const vec2 SRC = vec2(160.0, 128.0);
// bulge of the glass: 0 is flat
const float CURVE = 0.035;
// beam width (sigma, in rows) of dark and of bright pixels
const float BEAM_DARK = 0.26;
const float BEAM_BRIGHT = 0.36;
const float MASK = 0.10;
const float GLOW = 0.16;
const float BOOST = 1.3;

vec3 lin(vec3 c) { return pow(c, vec3(2.2)); }

vec3 texel(vec2 i) {
    i = clamp(i, vec2(0.0), SRC - 1.0);
    return lin(texture(u_tex, (i + 0.5) / SRC).rgb);
}

// screen position (0..1) → position on the tube (0..1), outside it at the rounded corners
vec2 warp(vec2 uv) {
    vec2 c = uv * 2.0 - 1.0;
    c *= (1.0 + CURVE * dot(c, c)) / (1.0 + CURVE);
    return c * 0.5 + 0.5;
}

float beam(float d, vec3 col) {
    float s = mix(BEAM_DARK, BEAM_BRIGHT, sqrt(max(col.r, max(col.g, col.b))));
    return exp(-0.5 * (d / s) * (d / s));
}

// linear colour of the tube at `uv`; `sharp` is physical pixels per device pixel
vec3 tube(vec2 uv, float sharp, float scan) {
    vec2 p = uv * SRC - 0.5;
    vec2 i = floor(p);
    vec2 f = p - i;
    // horizontally: sharp edges, softened a little like a beam
    float fx = clamp((f.x - 0.5) * sharp * 0.75 + 0.5, 0.0, 1.0);
    vec3 r0 = mix(texel(i), texel(i + vec2(1.0, 0.0)), fx);
    vec3 r1 = mix(texel(i + vec2(0.0, 1.0)), texel(i + vec2(1.0, 1.0)), fx);
    vec3 beams = r0 * beam(f.y, r0) + r1 * beam(1.0 - f.y, r1);
    float fy = clamp((f.y - 0.5) * sharp + 0.5, 0.0, 1.0);
    vec3 even = mix(r0, r1, fy);
    return mix(even, beams * BOOST, scan);
}

void main() {
    vec2 uv = warp(v_uv);
    float sharp = u_out.y / SRC.y;
    // scanlines finer than ~2 physical pixels only shimmer: fade them out on small screens
    float scan = smoothstep(1.4, 2.6, sharp);

    // 2×2 samples per physical pixel against moiré
    vec3 col = vec3(0.0);
    for (int k = 0; k < 4; k++) {
        vec2 o = (vec2(float(k & 1), float(k >> 1)) - 0.5) * 0.5 / u_out;
        col += tube(warp(v_uv + o), sharp, scan);
    }
    col *= 0.25;

    // glow around bright pixels
    vec2 c = floor(uv * SRC);
    vec3 halo = vec3(0.0);
    for (int y = -2; y <= 2; y++) {
        for (int x = -2; x <= 2; x++) {
            float w = exp(-0.35 * float(x * x + y * y));
            halo += texel(c + vec2(float(x), float(y))) * w;
        }
    }
    col += halo * (GLOW / 7.3);

    // aperture grille, in physical pixels
    float m = mod(floor(gl_FragCoord.x), 3.0);
    vec3 mask = vec3(m == 0.0 ? 1.0 : 1.0 - MASK, m == 1.0 ? 1.0 : 1.0 - MASK, m == 2.0 ? 1.0 : 1.0 - MASK);
    col *= mask * (1.0 + MASK * 0.6);

    // vignette and the glow of the phosphor itself
    vec2 v = uv * (1.0 - uv);
    col *= pow(clamp(v.x * v.y * 16.0, 0.0, 1.0), 0.12);
    col += vec3(0.0006, 0.0008, 0.0008);

    vec3 g = pow(clamp(col, 0.0, 1.0), vec3(1.0 / 2.2));
    // faint reflection of the room on the glass
    g += 0.025 * (1.0 - smoothstep(0.0, 0.75, length((v_uv - vec2(0.18, 0.08)) * vec2(1.0, 1.6))));

    // antialiased rounded edge of the tube (one physical pixel)
    vec2 e = min(uv, 1.0 - uv) * u_out;
    float a = clamp(min(e.x, e.y) + 0.5, 0.0, 1.0);
    f_color = vec4(g * a, a);
}
"#;

struct Program {
    /// the GL context it belongs to (a re-created window brings a new one)
    gl: usize,
    program: glow::Program,
    vao: glow::VertexArray,
    u_tex: Option<glow::UniformLocation>,
    u_out: Option<glow::UniformLocation>,
}

static PROGRAM: Mutex<Option<Program>> = Mutex::new(None);
static BROKEN: AtomicBool = AtomicBool::new(false);

/// False once the shader failed to build: draw the plain picture instead.
pub fn available() -> bool {
    !BROKEN.load(Ordering::Relaxed)
}

/// Paints the raw frame texture `tex` (160×128) as a CRT into `rect`.
pub fn paint(painter: &Painter, rect: Rect, tex: TextureId) {
    let cb = egui_glow::CallbackFn::new(move |info, p| {
        let Some(texture) = p.texture(tex) else { return };
        let gl = p.gl();
        let mut guard = PROGRAM.lock().unwrap_or_else(|e| e.into_inner());
        let key = Arc::as_ptr(gl) as usize;
        if guard.as_ref().is_none_or(|pr| pr.gl != key) {
            match unsafe { build(gl) } {
                Ok(pr) => *guard = Some(Program { gl: key, ..pr }),
                Err(e) => {
                    log::warn!("CRT shader: {e}; showing the plain screen");
                    BROKEN.store(true, Ordering::Relaxed);
                    *guard = None;
                    return;
                }
            }
        }
        let Some(pr) = guard.as_ref() else { return };
        let vp = info.viewport_in_pixels();
        unsafe {
            gl.use_program(Some(pr.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.uniform_1_i32(pr.u_tex.as_ref(), 0);
            gl.uniform_2_f32(pr.u_out.as_ref(), vp.width_px as f32, vp.height_px as f32);
            gl.bind_vertex_array(Some(pr.vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
        }
    });
    painter.add(PaintCallback { rect, callback: Arc::new(cb) });
}

unsafe fn build(gl: &glow::Context) -> Result<Program, String> {
    let version = egui_glow::ShaderVersion::get(gl);
    if !version.is_new_shader_interface() {
        return Err(format!("needs GLSL 1.40 or ES 3.00, have {version:?}"));
    }
    let header = format!("{}{}", version.version_declaration(), if version.is_embedded() { "precision highp float;\nprecision highp int;\n" } else { "" });
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
            let sh = gl.create_shader(kind)?;
            gl.shader_source(sh, &format!("{header}{src}"));
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                let log = gl.get_shader_info_log(sh);
                gl.delete_shader(sh);
                gl.delete_program(program);
                return Err(log);
            }
            gl.attach_shader(program, sh);
            shaders.push(sh);
        }
        gl.link_program(program);
        for sh in shaders {
            gl.detach_shader(program, sh);
            gl.delete_shader(sh);
        }
        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(log);
        }
        let vao = gl.create_vertex_array()?;
        Ok(Program {
            gl: 0,
            u_tex: gl.get_uniform_location(program, "u_tex"),
            u_out: gl.get_uniform_location(program, "u_out"),
            program,
            vao,
        })
    }
}
