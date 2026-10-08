//! Shared PipeWire helpers (Linux): one-time init, a main loop on its own thread that can be
//! stopped from any other thread, and POD serialisation.

#![cfg(target_os = "linux")]

use pipewire as pw;
use pw::spa;
use std::any::Any;
use std::sync::Once;
use std::thread::JoinHandle;

/// `pw_init` once per process.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(pw::init);
}

/// Whatever must stay alive while the loop runs (core, stream, listeners…).
pub type Guard = Box<dyn Any>;

/// A PipeWire main loop running on a dedicated thread.
///
/// `stop` (or dropping the value) asks the loop to quit and joins the thread; the objects
/// returned by the setup closure are destroyed on that thread, before the loop itself.
pub struct PwThread {
    quit: pw::channel::Sender<()>,
    handle: Option<JoinHandle<()>>,
}

impl PwThread {
    /// Starts the thread. `setup` runs on it with the new main loop and returns the objects to
    /// keep alive, or an error text that goes to `on_error` (the thread then ends).
    pub fn spawn<S, E>(name: &str, setup: S, on_error: E) -> std::io::Result<PwThread>
    where
        S: FnOnce(&pw::main_loop::MainLoopRc) -> Result<Guard, String> + Send + 'static,
        E: FnOnce(String) + Send + 'static,
    {
        let (quit, rx) = pw::channel::channel::<()>();
        let handle = std::thread::Builder::new().name(name.to_string()).spawn(move || {
            init();
            let mainloop = match pw::main_loop::MainLoopRc::new(None) {
                Ok(l) => l,
                Err(e) => {
                    on_error(format!("PipeWire: {e}"));
                    return;
                }
            };
            let weak = mainloop.downgrade();
            let _rx = rx.attach(mainloop.loop_(), move |()| {
                if let Some(l) = weak.upgrade() {
                    l.quit();
                }
            });
            match setup(&mainloop) {
                Ok(guard) => {
                    mainloop.run();
                    drop(guard);
                }
                Err(e) => on_error(e),
            }
        })?;
        Ok(PwThread { quit, handle: Some(handle) })
    }

    /// Asks the loop to quit and waits for the thread (a few milliseconds).
    pub fn stop(&mut self) {
        let _ = self.quit.send(());
        if let Some(h) = self.handle.take() {
            if h.thread().id() != std::thread::current().id() {
                let _ = h.join();
            }
        }
    }
}

impl Drop for PwThread {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Serialises a POD value (EnumFormat, Buffers…) to bytes for `Pod::from_bytes`.
pub fn pod_bytes(value: spa::pod::Value) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &value)
        .map(|(c, _)| c.into_inner())
        .unwrap_or_default()
}
