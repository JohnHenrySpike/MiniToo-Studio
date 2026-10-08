//! Tray icon (§16.13). Linux: StatusNotifierItem via `ksni` (KDE Plasma and other SNI hosts).
//! Elsewhere: the `tray-icon` crate.

use crate::canvas::Canvas;
use crate::color::Color;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct TrayState {
    /// colour of the little screen in the icon
    pub screen: Color,
    /// «MiniToo Studio\nКолонка: …\nРежим: …\nClaude: …»
    pub tooltip: String,
    pub claude_mode: bool,
    pub streaming: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    ToggleClaude,
    StopStream,
    Quit,
}

const MENU_SHOW: &str = "Показать окно";
const MENU_CLAUDE: &str = "Режим Claude";
const MENU_STOP: &str = "Остановить трансляцию экрана";
const MENU_QUIT: &str = "Выход";

/// Sizes rendered for the tray; the host picks the closest one.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const ICON_SIZES: [u32; 5] = [16, 22, 32, 48, 64];

/// The drawn icon: dark rounded body `#2b2b33`, a "screen" in `screen` colour, two pixel eyes.
/// Straight RGBA on a transparent background.
pub fn icon_rgba(screen: Color, size: u32) -> image::RgbaImage {
    let size = size.max(1);
    let s = size as f32;
    let mut c = Canvas::new(size, size);
    let m = s * 0.06;
    let (bx, by, bw, bh) = (m, s * 0.14, s - 2.0 * m, s * 0.72);
    c.fill_round_rect(bx, by, bw, bh, s * 0.14, Color::hex(0x2b2b33));
    let inset = s * 0.1;
    let (sx, sy, sw, sh) = (bx + inset, by + inset, bw - 2.0 * inset, bh - 2.0 * inset);
    c.fill_round_rect(sx, sy, sw, sh, s * 0.05, screen.with_alpha(255));
    // eyes snapped to whole pixels so they stay crisp at 16–22 px
    let e = (s * 0.09).max(1.0);
    let (cx, cy) = (sx + sw / 2.0, sy + sh / 2.0);
    let (ew, eh) = (e.round().max(1.0), (e * 1.6).round().max(1.0));
    let ey = (cy - e).round();
    c.aa = false;
    for ex in [cx - s * 0.17, cx + s * 0.08] {
        c.fill_rect(ex.round(), ey, ew, eh, Color::hex(0x1a1412));
    }
    c.to_image()
}

/// First line → title, the rest → description.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn split_tooltip(tooltip: &str) -> (String, String) {
    match tooltip.split_once('\n') {
        Some((title, rest)) => (title.to_string(), rest.to_string()),
        None => (tooltip.to_string(), String::new()),
    }
}

/// RGBA → ARGB32 in network byte order, as StatusNotifierItem wants it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn argb32(img: &image::RgbaImage) -> Vec<u8> {
    img.as_raw().as_chunks::<4>().0.iter().flat_map(|&[r, g, b, a]| [a, r, g, b]).collect()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};
    use ksni::TrayMethods;
    use std::time::Duration;
    use tokio::sync::watch;

    struct Sni {
        state: TrayState,
        icons: Vec<ksni::Icon>,
        on: Arc<dyn Fn(TrayAction) + Send + Sync>,
    }

    fn icons(screen: Color) -> Vec<ksni::Icon> {
        ICON_SIZES
            .iter()
            .map(|&s| ksni::Icon { width: s as i32, height: s as i32, data: argb32(&icon_rgba(screen, s)) })
            .collect()
    }

    impl Sni {
        fn set(&mut self, state: TrayState) {
            if state.screen != self.state.screen {
                self.icons = icons(state.screen);
            }
            self.state = state;
        }
    }

    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            "minitoo-studio".into()
        }

        fn title(&self) -> String {
            "MiniToo Studio".into()
        }

        fn category(&self) -> ksni::Category {
            ksni::Category::Hardware
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icons.clone()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            let (title, description) = split_tooltip(&self.state.tooltip);
            ksni::ToolTip { title, description, ..Default::default() }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.on)(TrayAction::ShowWindow);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let item = |label: &str, icon: &str, enabled: bool, action: TrayAction| -> MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    icon_name: icon.into(),
                    enabled,
                    activate: Box::new(move |t: &mut Self| (t.on)(action)),
                    ..Default::default()
                }
                .into()
            };
            vec![
                item(MENU_SHOW, "window", true, TrayAction::ShowWindow),
                CheckmarkItem {
                    label: MENU_CLAUDE.into(),
                    checked: self.state.claude_mode,
                    activate: Box::new(|t: &mut Self| (t.on)(TrayAction::ToggleClaude)),
                    ..Default::default()
                }
                .into(),
                item(MENU_STOP, "media-playback-stop", self.state.streaming, TrayAction::StopStream),
                MenuItem::Separator,
                item(MENU_QUIT, "application-exit", true, TrayAction::Quit),
            ]
        }
    }

    pub struct Tray {
        tx: watch::Sender<TrayState>,
    }

    impl Tray {
        pub fn spawn(rt: &tokio::runtime::Handle, initial: TrayState, on: Arc<dyn Fn(TrayAction) + Send + Sync>) -> Option<Tray> {
            let (tx, mut rx) = watch::channel(initial.clone());
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let tray = Sni { icons: icons(initial.screen), state: initial, on };
            rt.spawn(async move {
                let handle = match tray.spawn().await {
                    Ok(h) => {
                        let _ = ready_tx.send(true);
                        h
                    }
                    Err(e) => {
                        log::info!("трей недоступен: {e}");
                        let _ = ready_tx.send(false);
                        return;
                    }
                };
                while rx.changed().await.is_ok() {
                    let state = rx.borrow_and_update().clone();
                    if handle.update(|t| t.set(state)).await.is_none() {
                        return;
                    }
                }
                handle.shutdown().await;
            });
            ready_rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false).then_some(Tray { tx })
        }

        pub fn update(&self, state: TrayState) {
            self.tx.send_if_modified(|cur| {
                let changed = *cur != state;
                *cur = state;
                changed
            });
        }
    }
}

/// Other platforms use `tray-icon`. Its objects are not `Send` and need the platform event loop:
/// `Tray::spawn` and `Tray::update` must be called on the UI (main) thread after the event loop
/// has started (eframe runs one, which is enough on Windows and macOS). The icon lives in a
/// thread-local of that thread; calls from other threads do nothing.
#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    struct Items {
        icon: TrayIcon,
        claude: CheckMenuItem,
        stop: MenuItem,
        state: TrayState,
    }

    thread_local! {
        static TRAY: RefCell<Option<Items>> = const { RefCell::new(None) };
    }

    fn icon(screen: Color) -> Option<tray_icon::Icon> {
        let img = icon_rgba(screen, 32);
        tray_icon::Icon::from_rgba(img.into_raw(), 32, 32).ok()
    }

    pub struct Tray {}

    impl Tray {
        pub fn spawn(_rt: &tokio::runtime::Handle, initial: TrayState, on: Arc<dyn Fn(TrayAction) + Send + Sync>) -> Option<Tray> {
            let show = MenuItem::new(MENU_SHOW, true, None);
            let claude = CheckMenuItem::new(MENU_CLAUDE, true, initial.claude_mode, None);
            let stop = MenuItem::new(MENU_STOP, initial.streaming, None);
            let quit = MenuItem::new(MENU_QUIT, true, None);
            let menu = Menu::new();
            menu.append_items(&[&show, &claude, &stop, &PredefinedMenuItem::separator(), &quit]).ok()?;
            let ids = [
                (show.id().clone(), TrayAction::ShowWindow),
                (claude.id().clone(), TrayAction::ToggleClaude),
                (stop.id().clone(), TrayAction::StopStream),
                (quit.id().clone(), TrayAction::Quit),
            ];
            let mut builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(&initial.tooltip).with_menu_on_left_click(false);
            if let Some(i) = icon(initial.screen) {
                builder = builder.with_icon(i);
            }
            let tray = match builder.build() {
                Ok(t) => t,
                Err(e) => {
                    log::info!("трей недоступен: {e}");
                    return None;
                }
            };
            let on_menu = on.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if let Some((_, a)) = ids.iter().find(|(id, _)| *id == e.id) {
                    on_menu(*a);
                }
            }));
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    on(TrayAction::ShowWindow);
                }
            }));
            TRAY.with(|t| *t.borrow_mut() = Some(Items { icon: tray, claude, stop, state: initial }));
            Some(Tray {})
        }

        pub fn update(&self, state: TrayState) {
            TRAY.with(|t| {
                let mut t = t.borrow_mut();
                let Some(items) = t.as_mut() else { return };
                if items.state.screen != state.screen {
                    let _ = items.icon.set_icon(icon(state.screen));
                }
                if items.state.tooltip != state.tooltip {
                    let _ = items.icon.set_tooltip(Some(&state.tooltip));
                }
                items.claude.set_checked(state.claude_mode);
                items.stop.set_enabled(state.streaming);
                items.state = state;
            });
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            TRAY.with(|t| t.borrow_mut().take());
            MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
            TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
        }
    }
}

pub use imp::Tray;

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
        img.get_pixel(x, y).0
    }

    #[test]
    fn icon_layout() {
        let screen = Color::hex(0xd97757);
        for size in [16, 22, 32, 48, 64, 128] {
            let img = icon_rgba(screen, size);
            assert_eq!(img.dimensions(), (size, size));
            // transparent corners and top band
            assert_eq!(px(&img, 0, 0)[3], 0, "size {size}");
            assert_eq!(px(&img, size / 2, 0)[3], 0, "size {size}");
            // body between the outer edge and the screen (16 px has no fully covered band)
            let body_pixels = img.pixels().filter(|p| p.0 == [0x2b, 0x2b, 0x33, 255]).count();
            assert!(body_pixels >= size as usize, "size {size}");
            if size >= 32 {
                assert_eq!(px(&img, size / 2, (size as f32 * 0.19) as u32), [0x2b, 0x2b, 0x33, 255], "size {size}");
            }
            // screen colour near the screen's bottom-left
            let s = px(&img, (size as f32 * 0.25) as u32, (size as f32 * 0.70) as u32);
            assert_eq!(s, [0xd9, 0x77, 0x57, 255], "size {size}");
            // two dark eyes on the screen's middle row
            let dark = (0..size).filter(|&x| px(&img, x, size / 2) == [0x1a, 0x14, 0x12, 255]).count();
            assert!(dark >= 2, "size {size}");
        }
    }

    #[test]
    fn icon_follows_colour() {
        let a = icon_rgba(Color::hex(0xe04030), 32);
        let b = icon_rgba(Color::hex(0x5070b0), 32);
        assert_ne!(a, b);
        assert_eq!(px(&a, 8, 22), [0xe0, 0x40, 0x30, 255]);
    }

    #[test]
    fn argb_and_tooltip() {
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 4]));
        assert_eq!(argb32(&img), vec![4, 1, 2, 3]);
        assert_eq!(
            split_tooltip("MiniToo Studio\nКолонка: подключена\nРежим: часы"),
            ("MiniToo Studio".to_string(), "Колонка: подключена\nРежим: часы".to_string())
        );
        assert_eq!(split_tooltip("x"), ("x".to_string(), String::new()));
    }
}
