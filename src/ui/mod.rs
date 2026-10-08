//! The window (§16): navigation, the current page and the drawn MiniToo. Reads snapshots
//! from the core and sends commands; everything else here is purely local UI state.

pub mod crop_box;
mod device_panel;
mod nav;
mod pages;
pub mod pixel;
pub mod textures;
pub mod theme;
pub mod widgets;

use crate::api::{Command, CoreHandle, Snapshot, Theme};
use egui::{Vec2, vec2};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use textures::Textures;
use theme::{Palette, pal, set_pal};

pub struct UiOptions {
    pub start_hidden: bool,
    pub debug: bool,
    pub screenshot_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiExit {
    Hidden,
    Quit,
}

pub const PAGE_COUNT: usize = 6;
pub const WINDOW_SIZE: Vec2 = vec2(1340.0, 860.0);
pub const MIN_SIZE: Vec2 = vec2(1080.0, 700.0);
const NAV_WIDE: f32 = 224.0;
const NAV_NARROW: f32 = 80.0;
const PANEL_WIDTH: f32 = 348.0;

/// What a page gets to draw itself.
pub struct Cx<'a> {
    pub snap: &'a Snapshot,
    pub core: &'a CoreHandle,
    pub tex: &'a mut Textures,
    pub debug: bool,
    /// files are being dragged over the window
    pub drag_hover: bool,
    /// a page asks to switch the theme (applied locally at once)
    pub theme_request: Option<Theme>,
}

impl Cx<'_> {
    pub fn send(&self, cmd: Command) {
        self.core.send(cmd);
    }
}

fn shot_size() -> Option<Vec2> {
    let s = std::env::var("MINITOO_SHOT_SIZE").ok()?;
    let (w, h) = s.split_once(['x', 'X'])?;
    Some(vec2(w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn install_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontFamily};
    let mut defs = egui::FontDefinitions::default();
    let fonts = [
        ("dejavu", crate::fonts::SANS_TTF),
        ("dejavu-bold", crate::fonts::SANS_BOLD_TTF),
        ("dejavu-mono", crate::fonts::MONO_TTF),
        ("dejavu-mono-bold", crate::fonts::MONO_BOLD_TTF),
    ];
    for (name, bytes) in fonts {
        defs.font_data.insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    }
    let fallback: Vec<String> = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &[&str]| -> Vec<String> { first.iter().map(|s| s.to_string()).chain(fallback.iter().cloned()).collect() };
    defs.families.insert(FontFamily::Proportional, with(&["dejavu"]));
    defs.families.insert(FontFamily::Monospace, with(&["dejavu-mono", "dejavu"]));
    defs.families.insert(FontFamily::Name("bold".into()), with(&["dejavu-bold", "dejavu"]));
    defs.families.insert(FontFamily::Name("mono-bold".into()), with(&["dejavu-mono-bold", "dejavu-bold"]));
    ctx.set_fonts(defs);
}

/// Runs the window on the current thread until it is closed (see [`UiExit`]).
pub fn run(core: CoreHandle, opts: &UiOptions) -> UiExit {
    // started hidden: no window until something asks for it (a hidden window cannot be
    // created on every platform, Wayland included)
    if opts.start_hidden && opts.screenshot_dir.is_none() {
        let serial = core.snapshot().show_window_serial;
        loop {
            let snap = core.snapshot();
            if snap.quit {
                return UiExit::Quit;
            }
            if snap.show_window_serial != serial {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let exit = Arc::new(Mutex::new(UiExit::Quit));
    let size = if opts.screenshot_dir.is_some() { shot_size().unwrap_or(WINDOW_SIZE) } else { WINDOW_SIZE };
    let min = if opts.screenshot_dir.is_some() { vec2(size.x.min(MIN_SIZE.x), size.y.min(MIN_SIZE.y)) } else { MIN_SIZE };
    let viewport = egui::ViewportBuilder::default()
        .with_title("MiniToo Studio")
        .with_app_id("minitoo-studio")
        .with_inner_size(size)
        .with_min_inner_size(min);
    let native = eframe::NativeOptions { viewport, run_and_return: true, centered: true, ..Default::default() };
    let app_exit = exit.clone();
    let debug = opts.debug;
    let shot = opts.screenshot_dir.clone();
    let result = eframe::run_native(
        "minitoo-studio",
        native,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            pixel::reset();
            cc.egui_ctx.set_theme(egui::Theme::Light);
            let ctx = cc.egui_ctx.clone();
            core.set_repaint(Box::new(move || ctx.request_repaint()));
            Ok(Box::new(StudioApp::new(core, debug, shot, app_exit)))
        }),
    );
    if let Err(e) = result {
        log::error!("window: {e}");
    }
    *exit.lock().unwrap()
}

struct Shot {
    dir: PathBuf,
    step: usize,
    frames: u32,
    since: Instant,
    requested: bool,
}

struct StudioApp {
    core: CoreHandle,
    debug: bool,
    page: usize,
    seen_page: usize,
    theme: Theme,
    seen_theme: Theme,
    styled: Option<Theme>,
    show_serial: u64,
    tex: Textures,
    preview_modes: bool,
    preview_claude: bool,
    pages: pages::Pages,
    panel: device_panel::State,
    shot: Option<Shot>,
    exit: Arc<Mutex<UiExit>>,
    closing: bool,
}

impl StudioApp {
    fn new(core: CoreHandle, debug: bool, shot: Option<PathBuf>, exit: Arc<Mutex<UiExit>>) -> Self {
        let snap = core.snapshot();
        StudioApp {
            debug: debug || snap.debug,
            page: snap.page.min(PAGE_COUNT - 1),
            seen_page: snap.page,
            theme: snap.theme,
            seen_theme: snap.theme,
            styled: None,
            show_serial: snap.show_window_serial,
            tex: Textures::default(),
            preview_modes: false,
            preview_claude: false,
            pages: pages::Pages::default(),
            panel: device_panel::State::default(),
            shot: shot.map(|dir| Shot { dir, step: 0, frames: 0, since: Instant::now(), requested: false }),
            exit,
            closing: false,
            core,
        }
    }

    fn set_page(&mut self, page: usize) {
        if page != self.page {
            self.page = page;
            self.core.send(Command::SetPage(page));
        }
    }

    fn set_theme(&mut self, theme: Theme) {
        if theme != self.theme {
            self.theme = theme;
            self.core.send(Command::SetTheme(theme));
        }
    }

    fn sync(&mut self, snap: &Snapshot) {
        if snap.page != self.seen_page {
            self.seen_page = snap.page;
            self.page = snap.page.min(PAGE_COUNT - 1);
        }
        if snap.theme != self.seen_theme {
            self.seen_theme = snap.theme;
            self.theme = snap.theme;
        }
        self.debug |= snap.debug;
    }

    fn update_previews(&mut self, visible: bool) {
        let modes = visible && self.page == 2;
        let claude = visible && self.page == 3;
        if modes != self.preview_modes {
            self.preview_modes = modes;
            self.core.send(Command::PreviewModes(modes));
        }
        if claude != self.preview_claude {
            self.preview_claude = claude;
            self.core.send(Command::PreviewClaude(claude));
        }
    }

    fn finish(&mut self, ctx: &egui::Context, how: UiExit) {
        *self.exit.lock().unwrap() = how;
        self.update_previews(false);
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(shot) = self.shot.as_mut() else { return };
        let total = PAGE_COUNT * 2;
        // a finished capture arrives as an event
        let images: Vec<(usize, Arc<egui::ColorImage>)> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        user_data.data.as_ref().and_then(|d| d.downcast_ref::<usize>()).map(|s| (*s, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (step, image) in images {
            let name = if step < PAGE_COUNT { format!("page{step}.png") } else { format!("dark{}.png", step - PAGE_COUNT) };
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
            if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                if let Err(e) = img.save(shot.dir.join(&name)) {
                    log::error!("screenshot {name}: {e}");
                }
            }
            if step == shot.step {
                shot.step += 1;
                shot.frames = 0;
                shot.requested = false;
                shot.since = Instant::now();
            }
        }
        if shot.step >= total {
            self.finish(ctx, UiExit::Quit);
            return;
        }
        let page = shot.step % PAGE_COUNT;
        let theme = if shot.step < PAGE_COUNT { Theme::Beige } else { Theme::Dark };
        let step = shot.step;
        let (frames, since, requested) = (shot.frames, shot.since, shot.requested);
        if let Some(s) = self.shot.as_mut() {
            s.frames += 1;
        }
        self.set_page(page);
        self.set_theme(theme);
        if !requested && frames >= 12 && since.elapsed() > Duration::from_millis(700) {
            if let Some(s) = self.shot.as_mut() {
                s.requested = true;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(step)));
        }
        ctx.request_repaint_after(Duration::from_millis(30));
    }
}

impl eframe::App for StudioApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        pal().bg.to_normalized_gamma_f32()
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let snap = self.core.snapshot();
        self.sync(&snap);
        if snap.quit && !self.closing {
            self.finish(ctx, UiExit::Quit);
            return;
        }
        if snap.show_window_serial != self.show_serial {
            self.show_serial = snap.show_window_serial;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.closing {
            let how = if snap.settings.close_to_tray && self.shot.is_none() { UiExit::Hidden } else { UiExit::Quit };
            *self.exit.lock().unwrap() = how;
            self.update_previews(false);
            self.closing = true;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.screenshot_step(&ctx);
        let snap = self.core.snapshot();
        let p = Palette::of(self.theme);
        set_pal(p);
        if self.styled != Some(self.theme) {
            theme::apply_style(&ctx, &p);
            self.styled = Some(self.theme);
        }

        let (dropped, hovering) = ctx.input(|i| (i.raw.dropped_files.clone(), !i.raw.hovered_files.is_empty()));
        let paths: Vec<PathBuf> = dropped.into_iter().map(|f| f.path().to_path_buf()).collect();
        if !paths.is_empty() {
            self.core.send(Command::AddPaths(paths));
            if self.page != 0 {
                self.set_page(0);
            }
        }

        let width = ctx.content_rect().width();
        let narrow = width < 1220.0;
        let nav_w = if narrow { NAV_NARROW } else { NAV_WIDE };

        egui::Panel::left("nav")
            .exact_size(nav_w)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(p.shell))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(r.right() - 1.0, r.top()), r.max), 0, p.shell_lo);
                let out = nav::show(ui, &snap, self.page, narrow, self.theme);
                if let Some(page) = out.page {
                    self.set_page(page);
                }
                if out.toggle_theme {
                    self.set_theme(if self.theme == Theme::Dark { Theme::Beige } else { Theme::Dark });
                }
            });

        egui::Panel::right("device")
            .exact_size(PANEL_WIDTH)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(p.shell))
            .show(ui, |ui| {
                let r = ui.max_rect();
                ui.painter().rect_filled(egui::Rect::from_min_max(r.min, egui::pos2(r.left() + 1.0, r.bottom())), 0, p.shell_lo);
                let mut cx = Cx { snap: &snap, core: &self.core, tex: &mut self.tex, debug: self.debug, drag_hover: hovering, theme_request: None };
                device_panel::show(ui, &mut cx, &mut self.panel);
            });

        let mut theme_request = None;
        egui::CentralPanel::no_frame().frame(egui::Frame::NONE.fill(p.bg)).show(ui, |ui| {
            let mut cx = Cx { snap: &snap, core: &self.core, tex: &mut self.tex, debug: self.debug, drag_hover: hovering, theme_request: None };
            self.pages.show(ui, &mut cx, self.page);
            theme_request = cx.theme_request;
        });
        if let Some(t) = theme_request {
            self.set_theme(t);
        }

        self.update_previews(!self.closing);
        self.tex.end_pass();
    }
}
