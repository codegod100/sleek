//! Shared Relm4 entrypoint used by the desktop binary and GTK Android cdylib.

#[path = "relm4.rs"]
mod app;

pub fn run() {
    app::run();
}

#[cfg(target_os = "android")]
mod android;
