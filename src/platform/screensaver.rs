//! Screen lock (§12): `org.freedesktop.ScreenSaver.ActiveChanged(bool)` on the session bus.
//! PLACEHOLDER — replaced by the real implementation.

use std::sync::Arc;

/// Calls `on(active)` for every ActiveChanged signal (sent to anyone or to us directly).
/// Never calls `Lock`.
pub fn spawn_watch(_rt: &tokio::runtime::Handle, _on: Arc<dyn Fn(bool) + Send + Sync>) {}
