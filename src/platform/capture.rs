//! Screen capture (§7).
//!
//! * Linux: xdg-desktop-portal ScreenCast (`CreateSession` → `SelectSources` → `Start` →
//!   `OpenPipeWireRemote`, via ashpd) and a PipeWire video stream on its own thread.
//! * Elsewhere: the primary monitor polled with `xcap` on a thread.
//!
//! Only the newest frame is kept ([`Capture::latest`]); the consumer (screen streamer) picks it
//! up at its own pace.

use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Debug)]
pub enum CaptureEvent {
    /// the stream runs; `restore_token` should be saved as `screen/restoreToken`
    Started { width: u32, height: u32, restore_token: Option<String> },
    /// the source size changed while capturing
    Resized { width: u32, height: u32 },
    /// capture ended; `Some(error)` if it failed (dialog cancelled, portal error, stream gone)
    Stopped(Option<String>),
}

pub type CaptureSink = Arc<dyn Fn(CaptureEvent) + Send + Sync>;

/// Frames are taken from the source at most this often.
pub const MAX_FPS: u64 = 15;

// ---------------------------------------------------------------------- pixel formats

/// Memory layouts accepted from PipeWire (byte order in memory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Bgrx,
    Bgra,
    Rgbx,
    Rgba,
}

/// Copies a `width`×`height` frame whose rows are `stride` bytes apart into straight RGBA.
/// `None` if the buffer is too small for the given geometry.
pub fn convert_frame(src: &[u8], width: u32, height: u32, stride: usize, fmt: PixelFormat) -> Option<image::RgbaImage> {
    let (w, h) = (width as usize, height as usize);
    let row = w.checked_mul(4)?;
    if w == 0 || h == 0 || stride < row || src.len() < stride * (h - 1) + row {
        return None;
    }
    let mut out = Vec::with_capacity(row * h);
    for y in 0..h {
        let line = &src[y * stride..y * stride + row];
        match fmt {
            PixelFormat::Rgba => out.extend_from_slice(line),
            PixelFormat::Rgbx => {
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            PixelFormat::Bgra => {
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
            PixelFormat::Bgrx => {
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
        }
    }
    image::RgbaImage::from_raw(width, height, out)
}

// ---------------------------------------------------------------------- shared state

struct Shared {
    on_event: CaptureSink,
    /// `Stopped` was reported (or the capture was dropped): nothing more is reported
    finished: AtomicBool,
    counter: AtomicU64,
    latest: Mutex<Option<(u64, Arc<image::RgbaImage>)>>,
    #[cfg(target_os = "linux")]
    cancel: tokio::sync::watch::Sender<bool>,
    #[cfg(target_os = "linux")]
    pw: Mutex<Option<super::pipewire_util::PwThread>>,
    #[cfg(not(target_os = "linux"))]
    quit: AtomicBool,
}

impl Shared {
    fn emit(&self, ev: CaptureEvent) {
        if !self.finished.load(Ordering::SeqCst) {
            (self.on_event)(ev);
        }
    }

    /// Reports `Stopped(err)` once and tears the capture down.
    fn finish(&self, err: Option<String>) {
        if !self.finished.swap(true, Ordering::SeqCst) {
            if let Some(e) = &err {
                log::warn!("[minitoo] screen capture: {e}");
            }
            (self.on_event)(CaptureEvent::Stopped(err));
        }
        self.shutdown();
    }

    fn shutdown(&self) {
        #[cfg(target_os = "linux")]
        {
            self.cancel.send_replace(true);
            let t = self.pw.lock().take();
            drop(t); // joins the PipeWire thread (unless called from it)
        }
        #[cfg(not(target_os = "linux"))]
        self.quit.store(true, Ordering::SeqCst);
    }

    fn push(&self, img: image::RgbaImage) {
        let n = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        *self.latest.lock() = Some((n, Arc::new(img)));
    }

    fn stopped(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
}

pub struct Capture {
    shared: Arc<Shared>,
}

impl Capture {
    /// Starts the portal session (shows the picker unless `restore_token` is valid) and the
    /// PipeWire stream. Events arrive on `on_event` from any thread.
    pub fn start(rt: &tokio::runtime::Handle, restore_token: Option<String>, on_event: CaptureSink) -> Capture {
        #[cfg(target_os = "linux")]
        {
            let (cancel, rx) = tokio::sync::watch::channel(false);
            let shared = Arc::new(Shared {
                on_event,
                finished: AtomicBool::new(false),
                counter: AtomicU64::new(0),
                latest: Mutex::new(None),
                cancel,
                pw: Mutex::new(None),
            });
            let token = restore_token.filter(|t| !t.is_empty());
            rt.spawn(linux::portal_session(shared.clone(), token, rx));
            Capture { shared }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (rt, restore_token);
            let shared = Arc::new(Shared {
                on_event,
                finished: AtomicBool::new(false),
                counter: AtomicU64::new(0),
                latest: Mutex::new(None),
                quit: AtomicBool::new(false),
            });
            other::start(shared.clone());
            Capture { shared }
        }
    }

    /// Ends the capture; reports `Stopped(None)` unless it already ended.
    pub fn stop(&self) {
        self.shared.finish(None);
    }

    /// Latest frame (straight RGBA, full source resolution) and a counter that grows with
    /// every new frame.
    pub fn latest(&self) -> Option<(u64, Arc<image::RgbaImage>)> {
        self.shared.latest.lock().clone()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // silent: the owner is gone
        self.shared.finished.store(true, Ordering::SeqCst);
        self.shared.shutdown();
    }
}

// ---------------------------------------------------------------------- Linux: portal + PipeWire

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::platform::pipewire_util::{self, PwThread};
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::{PersistMode, ResponseError};
    use futures_util::StreamExt;
    use pipewire as pw;
    use pw::spa;
    use std::future::Future;
    use std::os::fd::OwnedFd;
    use std::time::{Duration, Instant};
    use tokio::sync::watch;

    /// Runs `f` unless the capture is cancelled first.
    async fn or_cancel<F: Future>(cancel: &mut watch::Receiver<bool>, f: F) -> Option<F::Output> {
        tokio::select! {
            r = f => Some(r),
            _ = cancel.wait_for(|c| *c) => None,
        }
    }

    fn portal_error(step: &str, e: ashpd::Error) -> String {
        match e {
            ashpd::Error::Response(ResponseError::Cancelled) => "выбор источника отменён".into(),
            ashpd::Error::Response(ResponseError::Other) => "портал вернул ошибку".into(),
            e => format!("{step}: {e}"),
        }
    }

    pub(super) async fn portal_session(shared: Arc<Shared>, token: Option<String>, mut cancel: watch::Receiver<bool>) {
        let proxy = match or_cancel(&mut cancel, Screencast::new()).await {
            None => return,
            Some(Ok(p)) => p,
            Some(Err(e)) => return shared.finish(Some(format!("xdg-desktop-portal ScreenCast недоступен: {e}"))),
        };
        let session = match or_cancel(&mut cancel, proxy.create_session(Default::default())).await {
            None => return,
            Some(Ok(s)) => s,
            Some(Err(e)) => return shared.finish(Some(portal_error("CreateSession", e))),
        };

        let result = async {
            let cursor = proxy.available_cursor_modes().await.unwrap_or_default();
            let mut opts = SelectSourcesOptions::default()
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::ExplicitlyRevoked);
            if cursor.contains(CursorMode::Embedded) {
                opts = opts.set_cursor_mode(CursorMode::Embedded);
            }
            if let Some(t) = token.as_deref() {
                opts = opts.set_restore_token(t);
            }
            proxy
                .select_sources(&session, opts)
                .await
                .and_then(|r| r.response())
                .map_err(|e| portal_error("SelectSources", e))?;
            let streams = proxy
                .start(&session, None, Default::default())
                .await
                .and_then(|r| r.response())
                .map_err(|e| portal_error("Start", e))?;
            let new_token = streams.restore_token().map(str::to_string);
            let node = streams.streams().first().map(|s| s.pipe_wire_node_id()).ok_or("портал не вернул поток")?;
            let fd = proxy
                .open_pipe_wire_remote(&session, Default::default())
                .await
                .map_err(|e| format!("OpenPipeWireRemote: {e}"))?;
            Ok::<_, String>((node, fd, new_token))
        };

        match or_cancel(&mut cancel, result).await {
            None => {}
            Some(Err(e)) => shared.finish(Some(e)),
            Some(Ok((node, fd, new_token))) => {
                let sh = shared.clone();
                let sh_err = shared.clone();
                let spawned = PwThread::spawn(
                    "minitoo-capture",
                    move |ml| stream_setup(ml, fd, node, new_token, sh),
                    move |e| sh_err.finish(Some(e)),
                );
                match spawned {
                    Err(_) => shared.finish(Some("не удалось запустить поток PipeWire".into())),
                    Ok(t) => {
                        *shared.pw.lock() = Some(t);
                        if *cancel.borrow() {
                            let t = shared.pw.lock().take();
                            let _ = tokio::task::spawn_blocking(move || drop(t)).await;
                        } else {
                            // run until stopped or until the portal closes the session
                            let mut closed = session.receive_closed().await.ok();
                            tokio::select! {
                                _ = cancel.wait_for(|c| *c) => {}
                                Some(_) = async { match closed.as_mut() { Some(s) => s.next().await, None => std::future::pending().await } } => {
                                    shared.finish(Some("сеанс захвата закрыт".into()));
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), session.close()).await;
        // make sure the PipeWire thread is gone even if `stop` was never called
        let t = shared.pw.lock().take();
        if t.is_some() {
            let _ = tokio::task::spawn_blocking(move || drop(t)).await;
        }
    }

    struct StreamData {
        shared: Arc<Shared>,
        loop_: pw::main_loop::MainLoopWeak,
        format: Option<PixelFormat>,
        width: u32,
        height: u32,
        started: bool,
        token: Option<String>,
        last: Option<Instant>,
    }

    impl StreamData {
        fn fail(&self, msg: String) {
            if self.shared.stopped() {
                return; // we are closing it ourselves
            }
            self.shared.finish(Some(msg));
            if let Some(l) = self.loop_.upgrade() {
                l.quit();
            }
        }
    }

    fn pixel_format(f: spa::param::video::VideoFormat) -> Option<PixelFormat> {
        use spa::param::video::VideoFormat as V;
        match f {
            V::BGRx => Some(PixelFormat::Bgrx),
            V::BGRA => Some(PixelFormat::Bgra),
            V::RGBx => Some(PixelFormat::Rgbx),
            V::RGBA => Some(PixelFormat::Rgba),
            _ => None,
        }
    }

    fn enum_format_pod() -> Vec<u8> {
        use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
        use spa::param::video::VideoFormat;
        use spa::utils::{Fraction, Rectangle};
        let obj = spa::pod::object!(
            spa::utils::SpaTypes::ObjectParamFormat,
            spa::param::ParamType::EnumFormat,
            spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
            spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
            spa::pod::property!(
                FormatProperties::VideoFormat,
                Choice,
                Enum,
                Id,
                VideoFormat::BGRx,
                VideoFormat::BGRx,
                VideoFormat::BGRA,
                VideoFormat::RGBx,
                VideoFormat::RGBA
            ),
            spa::pod::property!(
                FormatProperties::VideoSize,
                Choice,
                Range,
                Rectangle,
                Rectangle { width: 1920, height: 1080 },
                Rectangle { width: 1, height: 1 },
                Rectangle { width: 16384, height: 16384 }
            ),
            spa::pod::property!(
                FormatProperties::VideoFramerate,
                Choice,
                Range,
                Fraction,
                Fraction { num: 30, denom: 1 },
                Fraction { num: 0, denom: 1 },
                Fraction { num: 240, denom: 1 }
            ),
        );
        pipewire_util::pod_bytes(spa::pod::Value::Object(obj))
    }

    /// CPU-mappable buffers only (no DMA-BUF).
    fn buffers_pod() -> Vec<u8> {
        use spa::pod::{ChoiceValue, Object, Property, PropertyFlags, Value};
        use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
        let mask = (1i32 << spa::sys::SPA_DATA_MemPtr) | (1i32 << spa::sys::SPA_DATA_MemFd);
        let obj = Object {
            type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
            id: spa::param::ParamType::Buffers.as_raw(),
            properties: vec![Property {
                key: spa::sys::SPA_PARAM_BUFFERS_dataType,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags { default: mask, flags: vec![mask] },
                ))),
            }],
        };
        pipewire_util::pod_bytes(Value::Object(obj))
    }

    fn stream_setup(
        mainloop: &pw::main_loop::MainLoopRc,
        fd: OwnedFd,
        node: u32,
        token: Option<String>,
        shared: Arc<Shared>,
    ) -> Result<pipewire_util::Guard, String> {
        let context = pw::context::ContextRc::new(mainloop, None).map_err(|_| "не удалось запустить поток PipeWire".to_string())?;
        let core = context.connect_fd_rc(fd, None).map_err(|_| "не удалось подключиться к PipeWire".to_string())?;
        let stream = pw::stream::StreamRc::new(
            core,
            "minitoo-screen",
            pw::properties::properties! {
                *pw::keys::MEDIA_TYPE => "Video",
                *pw::keys::MEDIA_CATEGORY => "Capture",
                *pw::keys::MEDIA_ROLE => "Screen",
            },
        )
        .map_err(|e| format!("PipeWire: {e}"))?;

        let data = StreamData {
            shared,
            loop_: mainloop.downgrade(),
            format: None,
            width: 0,
            height: 0,
            started: false,
            token,
            last: None,
        };
        let min_interval = Duration::from_millis(1000 / MAX_FPS);
        let listener = stream
            .add_local_listener_with_user_data(data)
            .state_changed(|_, d, _old, new| match new {
                pw::stream::StreamState::Error(e) => {
                    d.fail(if e.is_empty() { "поток PipeWire закрыт".into() } else { e })
                }
                pw::stream::StreamState::Unconnected => d.fail("поток PipeWire закрыт".into()),
                _ => {}
            })
            .param_changed(|stream, d, id, param| {
                let Some(param) = param else { return };
                if id != spa::param::ParamType::Format.as_raw() {
                    return;
                }
                let mut info = spa::param::video::VideoInfoRaw::new();
                if info.parse(param).is_err() {
                    return;
                }
                d.format = pixel_format(info.format());
                let (w, h) = (info.size().width, info.size().height);
                if !d.started {
                    d.started = true;
                    d.shared.emit(CaptureEvent::Started { width: w, height: h, restore_token: d.token.take() });
                } else if (w, h) != (d.width, d.height) {
                    d.shared.emit(CaptureEvent::Resized { width: w, height: h });
                }
                d.width = w;
                d.height = h;
                let bytes = buffers_pod();
                if let Some(pod) = spa::pod::Pod::from_bytes(&bytes) {
                    let _ = stream.update_params(&mut [pod]);
                }
            })
            .process(move |stream, d| {
                // keep only the newest buffer; the others go straight back
                let mut newest = None;
                while let Some(b) = stream.dequeue_buffer() {
                    newest = Some(b);
                }
                let Some(mut buf) = newest else { return };
                if d.last.is_some_and(|t| t.elapsed() < min_interval) {
                    return;
                }
                let (Some(fmt), w, h) = (d.format, d.width, d.height) else { return };
                let datas = buf.datas_mut();
                let Some(first) = datas.first_mut() else { return };
                let (offset, size, stride) = {
                    let c = first.chunk();
                    (c.offset() as usize, c.size() as usize, c.stride())
                };
                if size == 0 {
                    return;
                }
                let stride = if stride > 0 { stride as usize } else { w as usize * 4 };
                let Some(bytes) = first.data() else { return };
                let Some(src) = bytes.get(offset.min(bytes.len())..) else { return };
                if let Some(img) = convert_frame(src, w, h, stride, fmt) {
                    d.last = Some(Instant::now());
                    d.shared.push(img);
                }
            })
            .register()
            .map_err(|e| format!("PipeWire: {e}"))?;

        let bytes = enum_format_pod();
        let pod = spa::pod::Pod::from_bytes(&bytes).ok_or("PipeWire: формат")?;
        stream
            .connect(
                spa::utils::Direction::Input,
                Some(node),
                pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
                &mut [pod],
            )
            .map_err(|e| format!("pw_stream_connect: {e}"))?;
        // the listener goes first, then the stream (and with it core and context)
        Ok(Box::new((listener, stream, context)))
    }
}

// ---------------------------------------------------------------------- elsewhere: xcap

#[cfg(not(target_os = "linux"))]
mod other {
    use super::*;
    use std::time::{Duration, Instant};

    pub(super) fn start(shared: Arc<Shared>) {
        let sh = shared.clone();
        let spawned = std::thread::Builder::new().name("minitoo-capture".into()).spawn(move || run(sh));
        if spawned.is_err() {
            shared.finish(Some("не удалось запустить захват экрана".into()));
        }
    }

    fn primary() -> Result<xcap::Monitor, String> {
        let all = xcap::Monitor::all().map_err(|e| e.to_string())?;
        let idx = all.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
        all.into_iter().nth(idx).ok_or_else(|| "нет мониторов".to_string())
    }

    fn run(shared: Arc<Shared>) {
        let monitor = match primary() {
            Ok(m) => m,
            Err(e) => return shared.finish(Some(format!("захват экрана недоступен: {e}"))),
        };
        let interval = Duration::from_millis(1000 / MAX_FPS);
        let mut size: Option<(u32, u32)> = None;
        let mut failures = 0;
        while !shared.quit.load(Ordering::SeqCst) {
            let t0 = Instant::now();
            match monitor.capture_image() {
                Ok(img) => {
                    failures = 0;
                    let s = (img.width(), img.height());
                    match size {
                        None => shared.emit(CaptureEvent::Started { width: s.0, height: s.1, restore_token: None }),
                        Some(old) if old != s => shared.emit(CaptureEvent::Resized { width: s.0, height: s.1 }),
                        _ => {}
                    }
                    size = Some(s);
                    shared.push(img);
                }
                Err(e) => {
                    failures += 1;
                    if size.is_none() || failures > 20 {
                        return shared.finish(Some(format!("захват экрана не удался: {e}")));
                    }
                }
            }
            if let Some(rest) = interval.checked_sub(t0.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
}

// ---------------------------------------------------------------------- stale portal backends

/// systemd user unit of a portal backend executable.
fn portal_unit(exe_name: &str) -> Option<&'static str> {
    Some(match exe_name {
        "xdg-desktop-portal-kde" => "plasma-xdg-desktop-portal-kde.service",
        "xdg-desktop-portal-gnome" => "xdg-desktop-portal-gnome.service",
        "xdg-desktop-portal-gtk" => "xdg-desktop-portal-gtk.service",
        "xdg-desktop-portal-wlr" => "xdg-desktop-portal-wlr.service",
        "xdg-desktop-portal-hyprland" => "xdg-desktop-portal-hyprland.service",
        "xdg-desktop-portal" => "xdg-desktop-portal.service",
        _ => return None,
    })
}

/// True if a `/proc/<pid>/maps` text maps a shared library that was replaced on disk
/// (`… /usr/lib/libfoo.so.6.2 (deleted)`).
fn maps_have_deleted_lib(maps: &str) -> bool {
    maps.lines().any(|line| {
        let Some(path) = line.trim_end().strip_suffix("(deleted)") else { return false };
        let path = path.trim_end();
        // strip trailing ".<digits>" groups, then expect ".so"
        let mut p = path;
        loop {
            match p.rfind('.') {
                Some(i) if i + 1 < p.len() && p[i + 1..].bytes().all(|b| b.is_ascii_digit()) => p = &p[..i],
                _ => break,
            }
        }
        p.ends_with(".so")
    })
}

/// Portal backend units of the user whose process maps deleted libraries (stale after a
/// system update), e.g. `plasma-xdg-desktop-portal-kde.service`.
pub fn stale_portal_units() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = unsafe { libc::getuid() };
        let mut stale: Vec<String> = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc") else { return stale };
        for entry in dir.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().filter(|s| s.bytes().all(|b| b.is_ascii_digit())) else { continue };
            let base = std::path::Path::new("/proc").join(pid);
            if std::fs::metadata(&base).map(|m| m.uid()).ok() != Some(uid) {
                continue;
            }
            let Ok(exe) = std::fs::read_link(base.join("exe")) else { continue };
            let exe = exe.to_string_lossy();
            // a replaced binary itself reads "…/xdg-desktop-portal-kde (deleted)"
            let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
            let file = exe.rsplit('/').next().unwrap_or(exe);
            let Some(unit) = portal_unit(file) else { continue };
            let Ok(maps) = std::fs::read_to_string(base.join("maps")) else { continue };
            if maps_have_deleted_lib(&maps) && !stale.iter().any(|u| u == unit) {
                stale.push(unit.to_string());
            }
        }
        stale
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (portal_unit, maps_have_deleted_lib);
        Vec::new()
    }
}

/// `systemctl --user restart <units>`
pub async fn restart_units(units: Vec<String>) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if units.is_empty() {
            return Ok(());
        }
        let out = tokio::process::Command::new("systemctl")
            .arg("--user")
            .arg("restart")
            .args(&units)
            .output()
            .await
            .map_err(|e| format!("systemctl: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() { format!("systemctl: код {}", out.status.code().unwrap_or(-1)) } else { err })
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = units;
        Err("недоступно".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_formats_with_stride() {
        // 2×2, stride 12 (4 bytes of padding per row)
        let mut src = vec![0u8; 24];
        let px = [[10, 20, 30, 40], [50, 60, 70, 80], [90, 100, 110, 120], [130, 140, 150, 160]];
        for (i, p) in px.iter().enumerate() {
            let (x, y) = (i % 2, i / 2);
            src[y * 12 + x * 4..y * 12 + x * 4 + 4].copy_from_slice(p);
        }
        src[8..12].copy_from_slice(&[255, 255, 255, 255]); // padding must be ignored
        let bgrx = convert_frame(&src, 2, 2, 12, PixelFormat::Bgrx).unwrap();
        assert_eq!(bgrx.get_pixel(0, 0).0, [30, 20, 10, 255]);
        assert_eq!(bgrx.get_pixel(1, 1).0, [150, 140, 130, 255]);
        let bgra = convert_frame(&src, 2, 2, 12, PixelFormat::Bgra).unwrap();
        assert_eq!(bgra.get_pixel(1, 0).0, [70, 60, 50, 80]);
        let rgbx = convert_frame(&src, 2, 2, 12, PixelFormat::Rgbx).unwrap();
        assert_eq!(rgbx.get_pixel(0, 1).0, [90, 100, 110, 255]);
        let rgba = convert_frame(&src, 2, 2, 12, PixelFormat::Rgba).unwrap();
        assert_eq!(rgba.get_pixel(1, 1).0, [130, 140, 150, 160]);
        // the last row needs no padding
        assert!(convert_frame(&src[..20], 2, 2, 12, PixelFormat::Rgba).is_some());
        assert!(convert_frame(&src[..19], 2, 2, 12, PixelFormat::Rgba).is_none());
        assert!(convert_frame(&src, 2, 2, 7, PixelFormat::Rgba).is_none());
        assert!(convert_frame(&src, 0, 2, 12, PixelFormat::Rgba).is_none());
    }

    #[test]
    fn detects_deleted_libraries() {
        let ok = "7f00-7f01 r-xp 00000000 08:01 123 /usr/lib/libQt6Core.so.6.8.0\n\
                  7f02-7f03 rw-s 00000000 00:01 9 /memfd:pipewire-memfd (deleted)\n";
        assert!(!maps_have_deleted_lib(ok));
        let stale = "7f00-7f01 r-xp 00000000 08:01 123 /usr/lib/libQt6Core.so.6.8.0 (deleted)\n";
        assert!(maps_have_deleted_lib(stale));
        assert!(maps_have_deleted_lib("7f-7f r-xp 0 0:0 1 /usr/lib/libfoo.so (deleted)"));
        assert!(!maps_have_deleted_lib("7f-7f r-xp 0 0:0 1 /usr/share/icons/cache.dat (deleted)"));
        assert_eq!(portal_unit("xdg-desktop-portal-kde"), Some("plasma-xdg-desktop-portal-kde.service"));
        assert_eq!(portal_unit("xdg-desktop-portal-wlr"), Some("xdg-desktop-portal-wlr.service"));
        assert_eq!(portal_unit("bash"), None);
        // must not panic, whatever runs here
        let _ = stale_portal_units();
    }
}
