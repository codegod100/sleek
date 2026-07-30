//! Sleek — freeq mobile client (desktop + Android NativeActivity).

mod app;
mod auth;
mod av;
#[cfg(not(target_os = "android"))]
mod av_media;
mod clipboard;
mod net;
mod preview;
mod state;
mod ui;

#[cfg(target_os = "android")]
mod android_media;

pub use app::run_desktop;

#[cfg(target_os = "android")]
pub use app::run_android;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    // Clone for eframe; keep a handle for storage paths + runtime permissions.
    android_media::set_android_app(android_app.clone());
    log::info!("sleek android_main start");
    match run_android(android_app) {
        Ok(()) => log::info!("sleek run_android returned Ok"),
        Err(e) => log::error!("sleek run_android error: {e}"),
    }
}
