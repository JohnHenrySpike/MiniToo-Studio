//! The window. Temporary stand-in until the egui front-end lands: waits until "show" is
//! requested again or the app quits.

use minitoo::api::CoreHandle;

pub struct Options {
    pub debug: bool,
    pub screenshot_dir: Option<std::path::PathBuf>,
}

pub enum Exit {
    Hidden,
    Quit,
}

pub fn run(core: CoreHandle, _opts: &Options) -> Exit {
    loop {
        if core.snapshot().quit {
            return Exit::Quit;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
