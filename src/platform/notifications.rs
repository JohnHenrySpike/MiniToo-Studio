//! Desktop notifications (§10): `BecomeMonitor` on a private session-bus connection.
//! PLACEHOLDER — replaced by the real implementation.

use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopNotification {
    pub app: String,
    pub summary: String,
    pub body: String,
    /// `app_icon`, or the `desktop-entry` hint when it is empty
    pub icon: String,
}

pub type NotifySink = Arc<dyn Fn(Result<DesktopNotification, String>) + Send + Sync>;

/// Starts monitoring `org.freedesktop.Notifications.Notify` calls. Errors (also later ones)
/// arrive as `Err(text)`. Monitoring stops when the returned guard is dropped.
pub fn spawn_monitor(_rt: &tokio::runtime::Handle, on: NotifySink) -> MonitorGuard {
    on(Err("мониторинг уведомлений недоступен на этой платформе".into()));
    MonitorGuard {}
}

pub struct MonitorGuard {}
