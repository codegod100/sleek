//! Sleek — freeq mobile client (desktop + Android NativeActivity).

mod app;
mod auth;
mod av;
mod av_media;
mod bsky;
mod clipboard;
mod net;
mod preview;
mod reconnect;
mod slash;
mod state;
mod ui;

/// V4L2 MMAP capture with spec-correct dqbuf (v4l2loopback/OBS workaround).
#[cfg(all(not(target_os = "android"), target_os = "linux"))]
mod v4l2cam;

#[cfg(target_os = "android")]
mod android_media;
#[cfg(target_os = "android")]
mod android_camera;
#[cfg(any(target_os = "android", test))]
mod nv12_orient;

pub use app::run_desktop;

#[cfg(target_os = "android")]
pub use app::run_android;

/// Default log filter for desktop (`env_logger` / `RUST_LOG`) and Android
/// (`android_logger` filter).
///
/// Global level stays `info` so sleek modules keep normal info/warn/error.
/// `moq_lite` is capped at `error` because `moq_lite::Error::from_transport`
/// always `tracing::warn!`s on closed transports — leave/hangup/idle close
/// otherwise spam one WARN per live stream. Real moq failures still surface
/// at error. On desktop, set `RUST_LOG` to override (e.g. `RUST_LOG=info` or
/// `RUST_LOG=moq_lite=warn,info`).
pub fn default_log_filter() -> &'static str {
    "info,moq_lite=error"
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_filter(
                android_logger::FilterBuilder::new()
                    .parse(default_log_filter())
                    .build(),
            ),
    );
    // Clone for eframe; keep a handle for storage paths + runtime permissions.
    android_media::set_android_app(android_app.clone());
    log::info!("sleek android_main start");
    match run_android(android_app) {
        Ok(()) => log::info!("sleek run_android returned Ok"),
        Err(e) => log::error!("sleek run_android error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::default_log_filter;

    #[test]
    fn default_log_filter_silences_moq_lite_warn() {
        let f = default_log_filter();
        // Global app level remains informative.
        assert!(
            f.split(',').any(|p| p.trim() == "info" || p.trim().starts_with("info,")),
            "expected default global info level in {f:?}"
        );
        assert!(
            f.contains("info"),
            "expected info default in filter {f:?}"
        );
        // moq_lite transport-close WARNs are suppressed (error-and-above only).
        assert!(
            f.contains("moq_lite=error"),
            "expected moq_lite=error clause in filter {f:?}"
        );
    }

    #[test]
    fn default_log_filter_is_env_filter_parseable() {
        // env_logger / android_logger both use env_filter-style directives.
        let f = default_log_filter();
        for part in f.split(',') {
            let part = part.trim();
            assert!(!part.is_empty(), "empty filter directive in {f:?}");
            if let Some((target, level)) = part.split_once('=') {
                assert!(!target.is_empty(), "empty target in {part:?}");
                assert!(
                    matches!(
                        level,
                        "error" | "warn" | "info" | "debug" | "trace" | "off"
                    ),
                    "unexpected level in {part:?}"
                );
            } else {
                assert!(
                    matches!(
                        part,
                        "error" | "warn" | "info" | "debug" | "trace" | "off"
                    ),
                    "unexpected bare level in {part:?}"
                );
            }
        }
    }

    /// Document that RUST_LOG override is possible: our default is only used
    /// when the env var is unset (`default_filter_or`). A user-set RUST_LOG
    /// replaces the default entirely (including re-enabling moq_lite WARN).
    #[test]
    fn rust_log_override_semantics_documented() {
        // Pure contract of env_logger::Env::default_filter_or:
        // unset → use our string; set → use RUST_LOG as-is.
        let default = default_log_filter();
        assert!(default.contains("moq_lite=error"));
        // A user who wants the old noise can set RUST_LOG=info (no moq cap).
        let user_override = "info";
        assert!(
            !user_override.contains("moq_lite=error"),
            "user override without moq_lite=error re-enables transport WARNs"
        );
    }
}
