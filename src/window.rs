//! The window: the egui front-end in `minitoo::ui`.

use minitoo::api::CoreHandle;
use minitoo::ui::{self, UiExit, UiOptions};

pub struct Options {
    pub debug: bool,
    pub screenshot_dir: Option<std::path::PathBuf>,
}

pub enum Exit {
    Hidden,
    Quit,
}

pub fn run(core: CoreHandle, opts: &Options) -> Exit {
    let ui_opts = UiOptions { start_hidden: false, debug: opts.debug, screenshot_dir: opts.screenshot_dir.clone() };
    match ui::run(core, &ui_opts) {
        UiExit::Hidden => Exit::Hidden,
        UiExit::Quit => Exit::Quit,
    }
}
