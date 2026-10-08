//! Screen capture (§7): xdg-desktop-portal ScreenCast + PipeWire on Linux.
//! PLACEHOLDER — replaced by the real implementation.

use std::sync::Arc;

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

pub struct Capture {}

impl Capture {
    /// Starts the portal session (shows the picker unless `restore_token` is valid) and the
    /// PipeWire stream. Events arrive on `on_event` from any thread.
    pub fn start(_rt: &tokio::runtime::Handle, _restore_token: Option<String>, on_event: CaptureSink) -> Capture {
        on_event(CaptureEvent::Stopped(Some("захват экрана недоступен".into())));
        Capture {}
    }

    pub fn stop(&self) {}

    /// Latest frame (straight RGBA, full source resolution) and a counter that grows with
    /// every new frame.
    pub fn latest(&self) -> Option<(u64, Arc<image::RgbaImage>)> {
        None
    }
}

/// Portal backend units of the user whose process maps deleted libraries (stale after a
/// system update), e.g. `plasma-xdg-desktop-portal-kde.service`.
pub fn stale_portal_units() -> Vec<String> {
    Vec::new()
}

/// `systemctl --user restart <units>`
pub async fn restart_units(_units: Vec<String>) -> Result<(), String> {
    Err("недоступно".into())
}
