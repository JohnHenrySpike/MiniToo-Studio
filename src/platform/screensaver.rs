//! Screen lock (§12): `org.freedesktop.ScreenSaver.ActiveChanged(bool)` on the session bus.

use std::sync::Arc;

/// Calls `on(active)` for every ActiveChanged signal (sent to anyone or to us directly).
/// Never calls `Lock`.
pub fn spawn_watch(rt: &tokio::runtime::Handle, on: Arc<dyn Fn(bool) + Send + Sync>) {
    #[cfg(target_os = "linux")]
    rt.spawn(async move {
        if let Err(e) = linux::watch(on).await {
            log::warn!("ScreenSaver.ActiveChanged: {e}");
        }
    });
    #[cfg(not(target_os = "linux"))]
    let _ = (rt, on);
}

/// Paths the signal is accepted on. KDE exports the service on both, so the same change may
/// arrive twice in a row; `Dedup` drops such an echo.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const PATHS: [&str; 2] = ["/ScreenSaver", "/org/freedesktop/ScreenSaver"];

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Default)]
struct Dedup {
    last: Option<(bool, String, std::time::Instant)>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Dedup {
    /// `false` if this is the same value just seen on the other path.
    fn accept(&mut self, active: bool, path: &str, now: std::time::Instant) -> bool {
        let echo = matches!(&self.last, Some((a, p, t))
            if *a == active && p != path && now.duration_since(*t) < std::time::Duration::from_secs(2));
        self.last = Some((active, path.to_string(), now));
        !echo
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{Dedup, PATHS};
    use futures_util::StreamExt;
    use std::sync::Arc;
    use zbus::message::Type;
    use zbus::{MatchRule, MessageStream};

    pub async fn watch(on: Arc<dyn Fn(bool) + Send + Sync>) -> zbus::Result<()> {
        let conn = zbus::Connection::session().await?;
        if let Some(name) = conn.unique_name() {
            log::info!("{}", tr!("platform.screensaver.listening", name = name));
        }
        // no path/sender in the rule: signals sent directly to our unique name match it too
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .interface("org.freedesktop.ScreenSaver")?
            .member("ActiveChanged")?
            .build();
        let mut stream = MessageStream::for_match_rule(rule, &conn, Some(16)).await?;
        let mut dedup = Dedup::default();
        while let Some(msg) = stream.next().await {
            let msg = msg?;
            let header = msg.header();
            let Some(path) = header.path().map(|p| p.as_str().to_string()) else { continue };
            if !PATHS.contains(&path.as_str()) {
                continue;
            }
            let Ok(active) = msg.body().deserialize::<bool>() else { continue };
            if dedup.accept(active, &path, std::time::Instant::now()) {
                on(active);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn drops_echo_from_second_path() {
        let mut d = Dedup::default();
        let t = Instant::now();
        assert!(d.accept(true, PATHS[0], t));
        assert!(!d.accept(true, PATHS[1], t + Duration::from_millis(5)));
        assert!(d.accept(false, PATHS[1], t + Duration::from_secs(10)));
        assert!(d.accept(false, PATHS[1], t + Duration::from_secs(11)));
        assert!(d.accept(false, PATHS[0], t + Duration::from_secs(20)));
    }
}
