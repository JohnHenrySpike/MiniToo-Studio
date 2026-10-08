//! Desktop integrations. Linux (KDE/freedesktop) is the primary target; every module compiles on
//! other platforms and reports "unavailable" there unless it has a portable implementation.

pub mod bluez;
pub mod capture;
pub mod icon_theme;
pub mod notifications;
pub mod pipewire_util;
pub mod screensaver;
pub mod tray;
