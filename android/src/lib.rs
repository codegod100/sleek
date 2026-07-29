//! Sleek — freeq mobile client (desktop + Android NativeActivity).

mod app;
mod net;
mod state;
mod ui;

pub use app::run_desktop;

#[cfg(target_os = "android")]
pub use app::run_android;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::info!("sleek android_main start");
    match run_android(android_app) {
        Ok(()) => log::info!("sleek run_android returned Ok"),
        Err(e) => log::error!("sleek run_android error: {e}"),
    }
}
