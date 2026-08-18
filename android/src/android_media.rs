//! Android storage helpers for the compose-bar image attach flow.
//!
//! Opens the **system document / photo picker** (`ACTION_OPEN_DOCUMENT` /
//! `GET_CONTENT` / `ACTION_PICK_IMAGES`) via JNI, then recovers the result
//! Intent from a helper fragment / `ActivityThread` pending results.
//! OAuth deep links use `uk.nandi.sleek.SleekActivity` (NativeActivity subclass
//! injected as APK `classes.dex`) so `onNewIntent` can capture `freeq://auth`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::Context;
use winit::platform::android::activity::{AndroidApp, WindowManagerFlags};

use crate::clipboard::PickAttachResult;

static ANDROID_APP: OnceLock<AndroidApp> = OnceLock::new();

/// Request code for Activity-path `startActivityForResult` (scavenger fallback).
const PICK_IMAGE_REQ: i32 = 0x51_EE_61;
/// Fragment path uses a 16-bit code (`Fragment.startActivityForResult` masks
/// with `0xffff`). Must match `PickFragment.REQ` in the embedded dex.
#[allow(dead_code)]
const PICK_FRAGMENT_REQ: i32 = 0x1_EE_6;

/// Stash the `AndroidApp` from `android_main` so path/permission helpers can use it.
///
/// Also enables edge-to-edge layout flags so the GL surface fills the physical
/// display and `content_rect` reports real status/nav insets.
pub fn set_android_app(app: AndroidApp) {
    // LAYOUT_IN_SCREEN: draw under system bars.
    // LAYOUT_INSET_DECOR: content_rect reports the safe area under those bars.
    app.set_window_flags(
        WindowManagerFlags::LAYOUT_IN_SCREEN | WindowManagerFlags::LAYOUT_INSET_DECOR,
        WindowManagerFlags::empty(),
    );
    let _ = ANDROID_APP.set(app);
}

fn android_app() -> Option<&'static AndroidApp> {
    ANDROID_APP.get()
}

/// Public handle for other Android modules (camera capture, etc.).
pub fn android_app_handle() -> Option<&'static AndroidApp> {
    android_app()
}

/// App-private directory for `prefs.json` / `session.json` (survives restarts).
///
/// The `dirs` crate does not support Android, so auth storage must use the
/// NativeActivity internal files dir instead of `dirs::config_dir()`.
pub fn app_storage_dir() -> Option<PathBuf> {
    let app = android_app()?;
    let base = app
        .internal_data_path()
        .or_else(|| app.external_data_path())?;
    Some(base.join("sleek"))
}

/// Load an **application** class (APK `classes.dex`) by binary name.
///
/// `Env::find_class` from a native-created thread (tokio / media worker,
/// egui) uses the **system** [`ClassLoader`](https://developer.android.com/training/articles/perf-jni#faq_FindClass),
/// which cannot see types like `uk.nandi.sleek.CameraCapture`. Always resolve
/// app classes through the Activity's loader instead.
///
/// `binary_name` is the Java binary name, e.g. `"uk.nandi.sleek.CameraCapture"`
/// (dots, not slashes).
pub fn load_app_class<'a>(
    env: &mut jni::Env<'a>,
    activity: &jni::objects::JObject<'_>,
    binary_name: &str,
) -> Result<jni::objects::JClass<'a>, String> {
    use jni::objects::{JClass, JValue};
    use jni::{jni_sig, jni_str};

    let loader = env
        .call_method(
            activity,
            jni_str!("getClassLoader"),
            jni_sig!(() -> java.lang.ClassLoader),
            &[],
        )
        .map_err(|e| format!("getClassLoader: {e}"))?
        .l()
        .map_err(|e| format!("getClassLoader: {e}"))?;
    if loader.is_null() {
        return Err("Activity.getClassLoader returned null".into());
    }

    let class_name = env
        .new_string(binary_name)
        .map_err(|e| format!("new_string({binary_name}): {e}"))?;
    let loaded = env
        .call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!((java.lang.String) -> java.lang.Class),
            &[JValue::Object(class_name.as_ref())],
        )
        .map_err(|e| format!("loadClass({binary_name}): {e}"))?
        .l()
        .map_err(|e| format!("loadClass({binary_name}): {e}"))?;
    if loaded.is_null() {
        return Err(format!("loadClass({binary_name}) returned null"));
    }
    env.cast_local::<JClass>(loaded)
        .map_err(|e| format!("cast {binary_name}: {e}"))
}

/// True when CAMERA is already granted (no dialog).
pub fn has_camera_permission() -> bool {
    check_self_permission("android.permission.CAMERA").unwrap_or(false)
}

/// True when RECORD_AUDIO is already granted (no dialog).
pub fn has_record_audio_permission() -> bool {
    check_self_permission("android.permission.RECORD_AUDIO").unwrap_or(false)
}

/// Outcome of [`ensure_av_call_permissions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvCallPermissions {
    pub mic: bool,
    pub camera: bool,
}

/// Request mic (+ optional camera) in a **single** system dialog.
///
/// Calling `requestPermissions` twice in a row replaces the first dialog, so
/// RECORD_AUDIO + CAMERA must be batched. Returns the current grant state
/// (dialog may still be open — check again after the user responds).
pub fn ensure_av_call_permissions(want_camera: bool) -> AvCallPermissions {
    let mut need: Vec<&str> = vec!["android.permission.RECORD_AUDIO"];
    if want_camera {
        need.push("android.permission.CAMERA");
    }
    match request_all_permissions(&need, 0x5_1EE_9) {
        Ok(true) => {
            log::debug!("android AV permissions already granted (camera={want_camera})");
        }
        Ok(false) => {
            log::info!(
                "android AV permissions requested (awaiting user; want_camera={want_camera})"
            );
        }
        Err(e) => {
            log::warn!("android AV permissions: {e}");
        }
    }
    AvCallPermissions {
        mic: has_record_audio_permission(),
        camera: has_camera_permission(),
    }
}

/// Ensure camera access for AV video publish; prompt if needed.
///
/// Returns `true` when already granted. `false` means the system dialog was
/// shown or the permission was denied — caller may still dial audio-only.
pub fn ensure_camera_permission() -> bool {
    match request_all_permissions(&["android.permission.CAMERA"], 0x5_1EE_8) {
        Ok(true) => {
            log::debug!("android CAMERA granted");
            true
        }
        Ok(false) => {
            log::info!("android CAMERA requested (awaiting user)");
            false
        }
        Err(e) => {
            log::warn!("android CAMERA permission: {e}");
            false
        }
    }
}

/// System navigation interaction mode (3-button vs gestures).
///
/// Matches Android's internal `config_navBarInteractionMode` / Secure
/// `navigation_mode` encoding. Used to refine chrome fallbacks when
/// `content_rect` under- or over-reports the bottom inset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMode {
    /// Back / Home / Recents bar.
    ThreeButton,
    /// Home pill + Back (older Android).
    TwoButton,
    /// Edge-swipe gestures + thin home indicator.
    Gestures,
    /// Detection failed — treat measured insets as authoritative.
    Unknown,
}

impl NavMode {
    pub fn is_button_nav(self) -> bool {
        matches!(self, Self::ThreeButton | Self::TwoButton)
    }

    pub fn is_gesture_nav(self) -> bool {
        matches!(self, Self::Gestures)
    }

    fn from_android(code: i32) -> Self {
        match code {
            0 => Self::ThreeButton,
            1 => Self::TwoButton,
            2 => Self::Gestures,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ThreeButton => "three_button",
            Self::TwoButton => "two_button",
            Self::Gestures => "gestures",
            Self::Unknown => "unknown",
        }
    }
}

/// Cache for JNI nav-mode probes (avoid attach every frame).
struct NavModeCache {
    mode: NavMode,
    /// `Instant` of last successful JNI probe.
    at: Instant,
}

static NAV_MODE_CACHE: Mutex<Option<NavModeCache>> = Mutex::new(None);

/// Re-probe interval — mode only changes in Settings.
const NAV_MODE_TTL: Duration = Duration::from_secs(2);

/// Detect 3-button vs gesture navigation via `SleekActivity.navigationMode()`.
///
/// Results are cached for [`NAV_MODE_TTL`]. Returns [`NavMode::Unknown`] when
/// JNI is unavailable (cold start, missing dex).
pub fn navigation_mode() -> NavMode {
    if let Ok(guard) = NAV_MODE_CACHE.lock() {
        if let Some(c) = guard.as_ref() {
            if c.at.elapsed() < NAV_MODE_TTL {
                return c.mode;
            }
        }
    }

    let mode = query_navigation_mode_jni().unwrap_or(NavMode::Unknown);

    if let Ok(mut guard) = NAV_MODE_CACHE.lock() {
        let prev = guard.as_ref().map(|c| c.mode);
        *guard = Some(NavModeCache {
            mode,
            at: Instant::now(),
        });
        if prev != Some(mode) {
            log::info!("android nav mode: {}", mode.as_str());
        }
    }

    mode
}

fn query_navigation_mode_jni() -> Option<NavMode> {
    let app = android_app()?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return None;
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return None;
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    let result = vm.attach_current_thread(|env| -> jni::errors::Result<i32> {
        // SAFETY: activity global ref owned by the runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };

        // 1) SleekActivity helper (full heuristics: Secure + config + insets).
        if let Ok(cls) = env.find_class(jni_str!("uk/nandi/sleek/SleekActivity")) {
            if let Ok(v) = env.call_static_method(
                &cls,
                jni_str!("navigationModeOf"),
                jni_sig!((android.app.Activity) -> jint),
                &[JValue::Object(&activity)],
            ) {
                if let Ok(code) = v.i() {
                    if code >= 0 {
                        return Ok(code);
                    }
                }
            }
            // Instance method fallback (subclass dex injected).
            if let Ok(v) = env.call_method(
                &activity,
                jni_str!("navigationMode"),
                jni_sig!(() -> jint),
                &[],
            ) {
                if let Ok(code) = v.i() {
                    if code >= 0 {
                        return Ok(code);
                    }
                }
            }
        }

        // 2) Direct Settings.Secure.navigation_mode — works even when the
        // injected SleekActivity dex is stale/missing the helper methods
        // (that was returning Unknown on Waydroid while `adb shell settings
        // get secure navigation_mode` already showed 0 = three-button).
        if let Ok(resolver) = env
            .call_method(
                &activity,
                jni_str!("getContentResolver"),
                jni_sig!(() -> android.content.ContentResolver),
                &[],
            )
            .and_then(|v| v.l())
        {
            if !resolver.is_null() {
                if let Ok(settings_secure) =
                    env.find_class(jni_str!("android/provider/Settings$Secure"))
                {
                    if let Ok(name) = env.new_string("navigation_mode") {
                        if let Ok(v) = env.call_static_method(
                            &settings_secure,
                            jni_str!("getInt"),
                            jni_sig!(
                                (android.content.ContentResolver, java.lang.String, jint) -> jint
                            ),
                            &[
                                JValue::Object(&resolver),
                                JValue::Object(name.as_ref()),
                                JValue::Int(-1),
                            ],
                        ) {
                            if let Ok(code) = v.i() {
                                if code >= 0 {
                                    return Ok(code);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(-1)
    });

    match result {
        Ok(code) => Some(NavMode::from_android(code)),
        Err(e) => {
            log::debug!("android nav mode JNI: {e}");
            None
        }
    }
}

/// Measured status/nav insets in **egui points**.
///
/// **Primary source:** JNI WindowInsets (`statusBars` + cutout +
/// `navigationBars`). NativeActivity `content_rect` is secondary — it often
/// reports `top=0` under edge-to-edge.
///
/// **Rules (hard-won):**
/// 1. **Measured bottom is never shrunk** by a nav-mode guess. Clamping a real
///    48 dp 3-button bar down to a “gesture” pad was the regression that drew
///    app chrome over Back/Home/Recents when the detector lied.
/// 2. **Bottom height classifies the bar** when mode is wrong/unknown:
///    ≥40 pt ⇒ button nav, 12–32 pt ⇒ gesture handle.
/// 3. Nav mode only **floors** missing measurements (never caps good ones).
/// 4. **Top is never zero** while edge-to-edge — floor so the clock clears.
pub fn measured_system_chrome(ctx: &Context) -> Option<vidya::SystemChrome> {
    let ppp = ctx.pixels_per_point().max(0.01);
    let screen = ctx.screen_rect();
    if screen.height() <= 0.0 {
        return None;
    }
    let max_inset = screen.height() * 0.25;

    // 1) WindowInsets via JNI (authoritative under edge-to-edge).
    let (jni_top_px, jni_bottom_px) = system_bar_insets_px().unwrap_or((0, 0));

    // 2) content_rect as secondary (sometimes has nav when WindowInsets lag).
    let (cr_top_px, cr_bottom_px) = content_rect_insets_px(ctx);

    // Top: take the larger clearance so the clock always wins.
    let top_px = jni_top_px.max(cr_top_px).max(0);
    // Bottom: prefer WindowInsets when present. content_rect can over-report
    // (IME/layout glitches) and used to force a 3-button-sized pad under
    // gesture nav when we took max(jni, cr).
    let bottom_px = if jni_bottom_px > 0 {
        jni_bottom_px
    } else {
        cr_bottom_px.max(0)
    };

    let mut top = (top_px as f32 / ppp).clamp(0.0, max_inset);
    let mut bottom = (bottom_px as f32 / ppp).clamp(0.0, max_inset);

    // Edge-to-edge surface: never allow a zero top inset. content_rect and even
    // early-frame WindowInsets can report 0 while the GL surface still draws
    // under the status bar — floor so the title / header clear the clock.
    const STATUS_TOP_MIN: f32 = 32.0;
    if top < STATUS_TOP_MIN {
        top = STATUS_TOP_MIN.min(max_inset);
    }

    // Bottom: **trust measured WindowInsets when present** (in points).
    // Never shrink a real inset because a nav-mode guess said "gestures" —
    // that was the regression that painted over Back/Home/Recents.
    // Floors apply only when measurement is missing (0 px).
    const GESTURE_BOTTOM_MIN: f32 = 16.0;
    const BUTTON_BOTTOM_MIN: f32 = 48.0;
    let nav = navigation_mode();
    if bottom_px <= 0 {
        if nav.is_button_nav() {
            bottom = BUTTON_BOTTOM_MIN.min(max_inset);
        } else if nav.is_gesture_nav() {
            bottom = GESTURE_BOTTOM_MIN.min(max_inset);
        } else {
            // Unknown + no measurement: prefer button pad (covering system
            // buttons is worse than a few empty points under gestures).
            bottom = BUTTON_BOTTOM_MIN.min(max_inset);
        }
    }
    // else: keep measured `bottom` from WindowInsets / content_rect as-is.

    // One-shot-ish log when values/mode change (cache key via static).
    {
        static LAST: Mutex<Option<(u32, u32, NavMode)>> = Mutex::new(None);
        let key = (
            (top * 10.0) as u32,
            (bottom * 10.0) as u32,
            nav,
        );
        if let Ok(mut g) = LAST.lock() {
            if g.as_ref() != Some(&key) {
                log::info!(
                    "android chrome: top={top:.1} bottom={bottom:.1} nav={} \
                     jni_px=({jni_top_px},{jni_bottom_px}) cr_px=({cr_top_px},{cr_bottom_px})",
                    nav.as_str()
                );
                *g = Some(key);
            }
        }
    }

    Some(vidya::SystemChrome { top, bottom })
}

/// Physical-pixel insets from `AndroidApp::content_rect` (may be zero/wrong).
fn content_rect_insets_px(ctx: &Context) -> (i32, i32) {
    let Some(app) = android_app() else {
        return (0, 0);
    };
    let cr = app.content_rect();
    if cr.right <= cr.left || cr.bottom <= cr.top {
        return (0, 0);
    }
    let ppp = ctx.pixels_per_point().max(0.01);
    let surface_h = (ctx.screen_rect().height() * ppp).round() as i32;
    if surface_h <= 0 {
        return (0, 0);
    }
    let content_h = cr.bottom - cr.top;
    if content_h < surface_h / 3 {
        return (0, 0);
    }
    let top = cr.top.max(0);
    let bottom = (surface_h - cr.bottom).max(0);
    (top, bottom)
}

/// JNI: status + nav bar insets in physical pixels.
///
/// Tries `SleekActivity` helpers first, then reads `WindowInsets` directly from
/// the decor view (does not depend on injected-dex methods — those were
/// returning 0/`Unknown` on device while `VRI` already had real insets).
fn system_bar_insets_px() -> Option<(i32, i32)> {
    let app = android_app()?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return None;
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return None;
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    let result = vm.attach_current_thread(|env| -> jni::errors::Result<(i32, i32)> {
        // SAFETY: activity global ref owned by the runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };

        // 1) SleekActivity helpers (dimen fallbacks + cutout max).
        if let Ok(cls) = env.find_class(jni_str!("uk/nandi/sleek/SleekActivity")) {
            let top = env
                .call_static_method(
                    &cls,
                    jni_str!("statusBarInsetPx"),
                    jni_sig!((android.app.Activity) -> jint),
                    &[JValue::Object(&activity)],
                )
                .ok()
                .and_then(|v| v.i().ok())
                .unwrap_or(0)
                .max(0);
            let bottom = env
                .call_static_method(
                    &cls,
                    jni_str!("navigationBarInsetPx"),
                    jni_sig!((android.app.Activity) -> jint),
                    &[JValue::Object(&activity)],
                )
                .ok()
                .and_then(|v| v.i().ok())
                .unwrap_or(0)
                .max(0);
            if top > 0 || bottom > 0 {
                return Ok((top, bottom));
            }
        }

        // 2) Direct WindowInsets from decor (framework API — no custom dex).
        let window = env
            .call_method(
                &activity,
                jni_str!("getWindow"),
                jni_sig!(() -> android.view.Window),
                &[],
            )?
            .l()?;
        if !window.is_null() {
            let decor = env
                .call_method(
                    &window,
                    jni_str!("getDecorView"),
                    jni_sig!(() -> android.view.View),
                    &[],
                )?
                .l()?;
            if !decor.is_null() {
                let root = env
                    .call_method(
                        &decor,
                        jni_str!("getRootWindowInsets"),
                        jni_sig!(() -> android.view.WindowInsets),
                        &[],
                    )?
                    .l()?;
                if !root.is_null() {
                    let mut top = 0i32;
                    let mut bottom = 0i32;

                    // Typed insets (API 30+): WindowInsets.Type.statusBars() etc.
                    if let Ok(type_cls) =
                        env.find_class(jni_str!("android/view/WindowInsets$Type"))
                    {
                        let status_t = env
                            .call_static_method(
                                &type_cls,
                                jni_str!("statusBars"),
                                jni_sig!(() -> jint),
                                &[],
                            )
                            .ok()
                            .and_then(|v| v.i().ok());
                        let nav_t = env
                            .call_static_method(
                                &type_cls,
                                jni_str!("navigationBars"),
                                jni_sig!(() -> jint),
                                &[],
                            )
                            .ok()
                            .and_then(|v| v.i().ok());
                        let cutout_t = env
                            .call_static_method(
                                &type_cls,
                                jni_str!("displayCutout"),
                                jni_sig!(() -> jint),
                                &[],
                            )
                            .ok()
                            .and_then(|v| v.i().ok());
                        if let (Some(st), Some(nt), Some(ct)) = (status_t, nav_t, cutout_t) {
                            let status_top = window_insets_top(env, &root, st);
                            let cutout_top = window_insets_top(env, &root, ct);
                            let nav_bottom = window_insets_bottom(env, &root, nt);
                            top = status_top.max(cutout_top);
                            bottom = nav_bottom;
                        }
                    }

                    if top <= 0 {
                        top = env
                            .call_method(
                                &root,
                                jni_str!("getSystemWindowInsetTop"),
                                jni_sig!(() -> jint),
                                &[],
                            )
                            .ok()
                            .and_then(|v| v.i().ok())
                            .unwrap_or(0)
                            .max(0);
                    }
                    if bottom <= 0 {
                        bottom = env
                            .call_method(
                                &root,
                                jni_str!("getSystemWindowInsetBottom"),
                                jni_sig!(() -> jint),
                                &[],
                            )
                            .ok()
                            .and_then(|v| v.i().ok())
                            .unwrap_or(0)
                            .max(0);
                    }

                    if top > 0 || bottom > 0 {
                        return Ok((top, bottom));
                    }
                }
            }
        }

        Ok((0, 0))
    });

    match result {
        Ok(pair) => Some(pair),
        Err(e) => {
            log::debug!("android system bar insets JNI: {e}");
            None
        }
    }
}

fn window_insets_top(env: &mut jni::Env<'_>, root: &jni::objects::JObject<'_>, type_mask: i32) -> i32 {
    use jni::objects::JValue;
    use jni::{jni_sig, jni_str};
    let Ok(insets) = env
        .call_method(
            root,
            jni_str!("getInsets"),
            jni_sig!((jint) -> android.graphics.Insets),
            &[JValue::Int(type_mask)],
        )
        .and_then(|v| v.l())
    else {
        return 0;
    };
    if insets.is_null() {
        return 0;
    }
    env.get_field(&insets, jni_str!("top"), jni_sig!(jint))
        .ok()
        .and_then(|v| v.i().ok())
        .unwrap_or(0)
        .max(0)
}

fn window_insets_bottom(
    env: &mut jni::Env<'_>,
    root: &jni::objects::JObject<'_>,
    type_mask: i32,
) -> i32 {
    use jni::objects::JValue;
    use jni::{jni_sig, jni_str};
    let Ok(insets) = env
        .call_method(
            root,
            jni_str!("getInsets"),
            jni_sig!((jint) -> android.graphics.Insets),
            &[JValue::Int(type_mask)],
        )
        .and_then(|v| v.l())
    else {
        return 0;
    };
    if insets.is_null() {
        return 0;
    }
    env.get_field(&insets, jni_str!("bottom"), jni_sig!(jint))
        .ok()
        .and_then(|v| v.i().ok())
        .unwrap_or(0)
        .max(0)
}

/// Shared media directories (used only as fallback path hints / logging).
#[allow(dead_code)]
pub fn media_root_candidates() -> Vec<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/storage/emulated/0/DCIM",
        "/storage/emulated/0/Pictures",
        "/storage/emulated/0/Download",
        "/sdcard/DCIM",
        "/sdcard/Pictures",
        "/sdcard/Download",
        "/storage/emulated/0",
        "/sdcard",
    ];
    let mut out = Vec::new();
    for c in CANDIDATES {
        let p = PathBuf::from(c);
        if p.is_dir() && !out.iter().any(|e: &PathBuf| e == &p) {
            out.push(p);
        }
    }
    if let Some(app) = android_app() {
        if let Some(ext) = app.external_data_path() {
            if ext.is_dir() && !out.iter().any(|e| e == &ext) {
                out.push(ext);
            }
        }
        if let Some(int) = app.internal_data_path() {
            if int.is_dir() && !out.iter().any(|e| e == &int) {
                out.push(int);
            }
        }
    }
    out
}

#[allow(dead_code)]
pub fn default_picker_dir() -> PathBuf {
    for p in media_root_candidates() {
        if dir_readable(&p) {
            return p;
        }
    }
    PathBuf::from("/storage/emulated/0/DCIM")
}

fn dir_readable(path: &Path) -> bool {
    path.is_dir() && std::fs::read_dir(path).is_ok()
}

/// Ensure we have photo-read permission; prompt the system dialog if needed.
pub fn ensure_read_images_permission() {
    match request_permissions(
        &[
            "android.permission.READ_MEDIA_IMAGES",
            "android.permission.READ_EXTERNAL_STORAGE",
        ],
        0x5_1EE_6,
    ) {
        Ok(true) => log::debug!("android media read permission granted"),
        Ok(false) => log::info!("android media read permission requested (awaiting user)"),
        Err(e) => log::warn!("android media permission: {e}"),
    }
}

/// Ensure mic access for native MoQ calls; prompt if needed.
///
/// Returns `true` when already granted. A `false` means the system dialog was
/// shown — caller may dial anyway and retry after the user accepts, or wait.
pub fn ensure_record_audio_permission() -> bool {
    match request_all_permissions(&["android.permission.RECORD_AUDIO"], 0x5_1EE_7) {
        Ok(true) => {
            log::debug!("android RECORD_AUDIO granted");
            true
        }
        Ok(false) => {
            log::info!("android RECORD_AUDIO requested (awaiting user)");
            false
        }
        Err(e) => {
            log::warn!("android RECORD_AUDIO permission: {e}");
            false
        }
    }
}

/// `Ok(true)` when `permission` is already granted.
fn check_self_permission(permission: &str) -> Result<bool, String> {
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;

    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm comes from the live AndroidApp for this process.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    let result = vm
        .attach_current_thread(|env| -> jni::errors::Result<bool> {
            // SAFETY: activity is a global ref owned by the Android runtime.
            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
            let jperm = env.new_string(permission)?;
            let res = env
                .call_method(
                    &activity,
                    jni_str!("checkSelfPermission"),
                    jni_sig!((java.lang.String) -> jint),
                    &[JValue::Object(jperm.as_ref())],
                )?
                .i()?;
            // PackageManager.PERMISSION_GRANTED == 0
            Ok(res == 0)
        })
        .map_err(|e| format!("{e}"))?;

    Ok(result)
}

/// `Ok(true)` = at least one listed permission already granted (or none needed).
/// `Ok(false)` = dialog shown / not granted yet.
///
/// For photo-read we treat *any* of the alternatives as sufficient (API 33+
/// uses `READ_MEDIA_IMAGES`; older uses `READ_EXTERNAL_STORAGE`).
fn request_permissions(permissions: &[&str], request_code: i32) -> Result<bool, String> {
    for perm in permissions {
        if check_self_permission(perm)? {
            return Ok(true);
        }
    }
    if permissions.is_empty() {
        return Ok(true);
    }
    // None granted — show dialog for all alternatives (user accepts either).
    request_permissions_dialog(permissions, request_code)
}

/// `Ok(true)` = **all** listed permissions already granted.
/// `Ok(false)` = dialog shown for the missing ones / not granted yet.
///
/// Use for AV (mic + camera): a second `requestPermissions` call replaces the
/// first system dialog, so missing grants must be batched.
fn request_all_permissions(permissions: &[&str], request_code: i32) -> Result<bool, String> {
    let mut need: Vec<&str> = Vec::new();
    for perm in permissions {
        if !check_self_permission(perm)? {
            need.push(*perm);
        }
    }
    if need.is_empty() {
        return Ok(true);
    }
    request_permissions_dialog(&need, request_code)?;
    Ok(false)
}

fn request_permissions_dialog(permissions: &[&str], request_code: i32) -> Result<bool, String> {
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;

    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm comes from the live AndroidApp for this process.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        // SAFETY: activity is a global ref owned by the Android runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };

        let empty = env.new_string("")?;
        let empty_obj: &JObject = empty.as_ref();
        let arr = env.new_object_array(
            permissions.len() as jni::sys::jsize,
            jni_str!("java/lang/String"),
            empty_obj,
        )?;
        for (i, perm) in permissions.iter().enumerate() {
            let jperm = env.new_string(*perm)?;
            let jperm_obj: &JObject = jperm.as_ref();
            arr.set_element(env, i, jperm_obj)?;
        }

        let arr_obj: &JObject = arr.as_ref();
        env.call_method(
            &activity,
            jni_str!("requestPermissions"),
            jni_sig!(([java.lang.String], jint)),
            &[JValue::Object(arr_obj), JValue::Int(request_code)],
        )?;
        Ok(())
    })
    .map_err(|e| format!("{e}"))?;

    Ok(false)
}

// ── OS image picker ─────────────────────────────────────────────────────────

/// Guards against concurrent picker sessions.
static PICK_BUSY: Mutex<bool> = Mutex::new(false);

/// Keeps the InMemoryDexClassLoader that owns `PickFragment` alive for the
/// process lifetime (ART may unload classes if the loader is dropped).
static PICK_DEX_LOADER: Mutex<Option<jni::refs::Global<jni::objects::JObject<'static>>>> =
    Mutex::new(None);

/// Embedded helper Fragment (`android/java/.../PickFragment.java` → dex).
/// NativeActivity has no `onActivityResult`; the framework routes fragment
/// results via `dispatchActivityResult(who, …)` instead.
const PICK_FRAGMENT_DEX: &[u8] = include_bytes!("assets/pick_fragment.dex");
const PICK_FRAGMENT_CLASS: &str = "uk.nandi.sleek.PickFragment";

/// Outcome of one poll over fragment statics / pending-results fallback.
enum RawPick {
    /// No matching result yet (picker still open, or result not delivered).
    Waiting,
    /// User selected an image; raw file bytes.
    Image(Vec<u8>),
    /// User cancelled / dismissed / empty selection.
    Cancelled,
    /// Result present but unusable.
    Failed(String),
}

/// Open the system image/video picker and block until the user chooses or cancels.
///
/// Designed to run on a **background thread** (never the egui UI thread).
pub fn pick_media_file() -> PickAttachResult {
    ensure_read_images_permission();

    {
        let mut busy = PICK_BUSY
            .lock()
            .map_err(|_| "File picker lock poisoned".to_string())?;
        if *busy {
            return Err("A file picker is already open".into());
        }
        *busy = true;
    }

    let finish = |outcome: PickAttachResult| -> PickAttachResult {
        let _ = PICK_BUSY.lock().map(|mut g| *g = false);
        outcome
    };

    // Prefer Fragment.startActivityForResult: NativeActivity drops Activity
    // results, but fragment-scoped request codes are re-delivered to the helper.
    let mut used_fragment = false;
    match launch_system_image_picker_via_fragment() {
        Ok(()) => {
            used_fragment = true;
            log::debug!("android pick: launched via PickFragment");
        }
        Err(e) => {
            log::warn!("android pick: fragment launch failed ({e}); trying Activity path");
            if let Err(e2) = launch_system_image_picker_on_activity() {
                return finish(Err(format!("{e2} (fragment: {e})")));
            }
        }
    }

    let launched_at = Instant::now();
    let deadline = launched_at + Duration::from_secs(180);
    let mut saw_paused = false;
    let mut resumed_at: Option<Instant> = None;

    loop {
        // Primary: helper Fragment statics (set from onActivityResult).
        match poll_fragment_pick_result() {
            Ok(RawPick::Image(bytes)) => {
                return finish(crate::clipboard::load_attach_from_vec(bytes, None).map(Some));
            }
            Ok(RawPick::Cancelled) => {
                return finish(Ok(None));
            }
            Ok(RawPick::Failed(e)) => {
                return finish(Err(e));
            }
            Ok(RawPick::Waiting) => {}
            Err(e) => log::debug!("android pick fragment poll: {e}"),
        }

        // Fallback: scavenge ActivityThread.pendingResults (racey on modern
        // Android — only useful if the fragment path could not be installed).
        if !used_fragment {
            match scavenge_pick_result() {
                Ok(RawPick::Image(bytes)) => {
                    return finish(crate::clipboard::load_attach_from_vec(bytes, None).map(Some));
                }
                Ok(RawPick::Cancelled) => {
                    return finish(Ok(None));
                }
                Ok(RawPick::Failed(e)) => {
                    return finish(Err(e));
                }
                Ok(RawPick::Waiting) => {}
                Err(e) => log::debug!("android pick scavenge: {e}"),
            }
        }

        // Lifecycle fallback when the Activity path is used (or the fragment
        // never got a callback). Fragment cancel/OK always sets `done`, so this
        // is mainly for the scavenger fallback — keep a longer grace period so
        // a slow URI read is not mistaken for cancel.
        if !used_fragment {
            match activity_is_resumed() {
                Some(false) if launched_at.elapsed() > Duration::from_millis(400) => {
                    saw_paused = true;
                    resumed_at = None;
                }
                Some(true) if saw_paused => {
                    let since = resumed_at.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_millis(1500) {
                        log::debug!("android pick: resumed without result — treating as cancel");
                        return finish(Ok(None));
                    }
                }
                _ => {}
            }
        }

        if Instant::now() > deadline {
            return finish(Err("Image picker timed out".into()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn with_activity_env<T>(
    f: impl FnOnce(&mut jni::Env<'_>, jni::sys::jobject) -> Result<T, String>,
) -> Result<T, String> {
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }

    use jni::JavaVM;

    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    // jobject is a raw pointer (Copy); pass by value so the attach closure does
    // not borrow a short-lived local (which forced a bogus `'static` bound).
    let mut out: Option<Result<T, String>> = None;
    let jni_result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        out = Some(f(env, activity_ptr));
        Ok(())
    });
    jni_result.map_err(|e| format!("{e}"))?;
    out.unwrap_or_else(|| Err("JNI attach produced no result".into()))
}

/// SAFETY: `activity_ptr` must be the live NativeActivity global ref from `AndroidApp`.
macro_rules! activity_from_raw {
    ($env:expr, $activity_ptr:expr) => {{
        use jni::objects::JObject;
        use jni::refs::Global;
        // SAFETY: caller upholds the activity pointer invariant.
        unsafe { $env.as_cast_raw::<Global<JObject>>(&$activity_ptr) }.map_err(|e| format!("{e}"))
    }};
}

fn build_picker_intent<'a>(
    env: &mut jni::Env<'a>,
    _activity: &jni::objects::JObject<'_>,
) -> Result<jni::objects::JObject<'a>, String> {
    // Prefer OPEN_DOCUMENT with image + video MIME types. The modern photo
    // picker (`PICK_IMAGES`) is images-only and would hide `.mp4` / `.webm`.
    build_open_document_intent(env).map_err(|e| format!("{e}"))
}

/// Launch the picker through `PickFragment` so the result is not dropped by
/// NativeActivity's empty `onActivityResult`.
fn launch_system_image_picker_via_fragment() -> Result<(), String> {
    with_activity_env(|env, activity_ptr| {
        use jni::objects::JValue;
        use jni::{jni_sig, jni_str};

        let activity = activity_from_raw!(env, activity_ptr)?;
        let intent = build_picker_intent(env, activity.as_ref())?;
        let pick_cls = load_pick_fragment_class(env, activity.as_ref())?;
        env.call_static_method(
            &pick_cls,
            jni_str!("startPick"),
            jni_sig!((android.app.Activity, android.content.Intent)),
            &[
                JValue::Object(activity.as_ref()),
                JValue::Object(intent.as_ref()),
            ],
        )
        .map_err(|e| format!("PickFragment.startPick: {e}"))?;
        Ok(())
    })
}

/// Legacy: `Activity.startActivityForResult` + pendingResults scavenger.
fn launch_system_image_picker_on_activity() -> Result<(), String> {
    with_activity_env(|env, activity_ptr| {
        use jni::objects::JValue;
        use jni::{jni_sig, jni_str};

        let activity = activity_from_raw!(env, activity_ptr)?;
        let intent = build_picker_intent(env, activity.as_ref())?;
        env.call_method(
            activity.as_ref(),
            jni_str!("startActivityForResult"),
            jni_sig!((android.content.Intent, jint)),
            &[JValue::Object(intent.as_ref()), JValue::Int(PICK_IMAGE_REQ)],
        )
        .map_err(|e| format!("startActivityForResult: {e}"))?;
        Ok(())
    })
    .map_err(|e| format!("Failed to open system image picker: {e}"))
}

/// Load `PickFragment` from the embedded dex (once per process) and return it.
fn load_pick_fragment_class<'a>(
    env: &mut jni::Env<'a>,
    activity: &jni::objects::JObject<'_>,
) -> Result<jni::objects::JClass<'a>, String> {
    use jni::objects::{JClass, JValue};
    use jni::{jni_sig, jni_str};

    {
        let mut slot = PICK_DEX_LOADER
            .lock()
            .map_err(|_| "Pick dex loader lock poisoned".to_string())?;
        if slot.is_none() {
            let parent = env
                .call_method(
                    activity,
                    jni_str!("getClassLoader"),
                    jni_sig!(() -> java.lang.ClassLoader),
                    &[],
                )
                .map_err(|e| format!("getClassLoader: {e}"))?
                .l()
                .map_err(|e| format!("{e}"))?;

            let byte_arr = env
                .byte_array_from_slice(PICK_FRAGMENT_DEX)
                .map_err(|e| format!("dex bytes: {e}"))?;
            let bb_cls = env
                .find_class(jni_str!("java/nio/ByteBuffer"))
                .map_err(|e| format!("{e}"))?;
            let buffer = env
                .call_static_method(
                    &bb_cls,
                    jni_str!("wrap"),
                    jni_sig!(([jbyte]) -> java.nio.ByteBuffer),
                    &[JValue::Object(byte_arr.as_ref())],
                )
                .map_err(|e| format!("ByteBuffer.wrap: {e}"))?
                .l()
                .map_err(|e| format!("{e}"))?;

            let loader_cls = env
                .find_class(jni_str!("dalvik/system/InMemoryDexClassLoader"))
                .map_err(|e| format!("InMemoryDexClassLoader: {e}"))?;
            let loader = env
                .new_object(
                    &loader_cls,
                    jni_sig!((java.nio.ByteBuffer, java.lang.ClassLoader)),
                    &[
                        JValue::Object(buffer.as_ref()),
                        JValue::Object(parent.as_ref()),
                    ],
                )
                .map_err(|e| format!("new InMemoryDexClassLoader: {e}"))?;

            let global = env
                .new_global_ref(&loader)
                .map_err(|e| format!("global dex loader: {e}"))?;
            *slot = Some(global);
            log::info!(
                "android pick: loaded PickFragment dex ({} bytes)",
                PICK_FRAGMENT_DEX.len()
            );
        }
    }

    let loader_global = PICK_DEX_LOADER
        .lock()
        .map_err(|_| "Pick dex loader lock poisoned".to_string())?;
    let loader = loader_global
        .as_ref()
        .ok_or_else(|| "Pick dex loader missing after init".to_string())?;

    let class_name = env
        .new_string(PICK_FRAGMENT_CLASS)
        .map_err(|e| format!("{e}"))?;
    let loaded = env
        .call_method(
            loader.as_ref(),
            jni_str!("loadClass"),
            jni_sig!((java.lang.String) -> java.lang.Class),
            &[JValue::Object(class_name.as_ref())],
        )
        .map_err(|e| format!("loadClass: {e}"))?
        .l()
        .map_err(|e| format!("{e}"))?;
    if loaded.is_null() {
        return Err("loadClass returned null".into());
    }

    env.cast_local::<JClass>(loaded)
        .map_err(|e| format!("cast PickFragment class: {e}"))
}

/// Read `PickFragment` static result fields (set from `onActivityResult`).
fn poll_fragment_pick_result() -> Result<RawPick, String> {
    with_activity_env(|env, activity_ptr| {
        use jni::objects::JString;
        use jni::{jni_sig, jni_str};

        // Class may not be loaded yet (launch failed before dex load).
        let loader_loaded = PICK_DEX_LOADER
            .lock()
            .map(|g| g.is_some())
            .unwrap_or(false);
        if !loader_loaded {
            return Ok(RawPick::Waiting);
        }

        let activity = activity_from_raw!(env, activity_ptr)?;
        let cls = load_pick_fragment_class(env, activity.as_ref())?;

        let done = env
            .get_static_field(&cls, jni_str!("done"), jni_sig!(jboolean))
            .map_err(|e| format!("done field: {e}"))?
            .z()
            .map_err(|e| format!("{e}"))?;
        if !done {
            return Ok(RawPick::Waiting);
        }

        // Optional error string set by startPick on failure.
        if let Ok(err_v) = env.get_static_field(&cls, jni_str!("error"), jni_sig!(java.lang.String))
        {
            if let Ok(err_obj) = err_v.l() {
                if !err_obj.is_null() {
                    let jstr = env
                        .cast_local::<JString>(err_obj)
                        .map_err(|e| format!("cast error string: {e}"))?;
                    let s = format!("{jstr}");
                    reset_pick_fragment_statics(env, &cls);
                    if !s.is_empty() {
                        return Ok(RawPick::Failed(s));
                    }
                }
            }
        }

        let result_code = env
            .get_static_field(&cls, jni_str!("resultCode"), jni_sig!(jint))
            .map_err(|e| format!("resultCode: {e}"))?
            .i()
            .map_err(|e| format!("{e}"))?;
        // RESULT_OK = -1, RESULT_CANCELED = 0
        if result_code == 0 {
            reset_pick_fragment_statics(env, &cls);
            return Ok(RawPick::Cancelled);
        }
        if result_code != -1 {
            reset_pick_fragment_statics(env, &cls);
            return Ok(RawPick::Cancelled);
        }

        let data = env
            .get_static_field(&cls, jni_str!("data"), jni_sig!(android.content.Intent))
            .map_err(|e| format!("data field: {e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if data.is_null() {
            reset_pick_fragment_statics(env, &cls);
            return Ok(RawPick::Cancelled);
        }

        let outcome = match read_image_bytes_from_result_intent(env, &data) {
            Ok(Some(bytes)) => RawPick::Image(bytes),
            Ok(None) => RawPick::Cancelled,
            Err(e) => RawPick::Failed(e),
        };
        reset_pick_fragment_statics(env, &cls);
        Ok(outcome)
    })
}

fn reset_pick_fragment_statics(env: &mut jni::Env<'_>, cls: &jni::objects::JClass<'_>) {
    use jni::{jni_sig, jni_str};

    let _ = env.call_static_method(cls, jni_str!("reset"), jni_sig!(()), &[]);
}

fn build_open_document_intent<'a>(
    env: &mut jni::Env<'a>,
) -> jni::errors::Result<jni::objects::JObject<'a>> {
    use jni::objects::{JObject, JValue};
    use jni::{jni_sig, jni_str};

    // Intent.ACTION_OPEN_DOCUMENT
    let action = env.new_string("android.intent.action.OPEN_DOCUMENT")?;
    let intent_cls = env.find_class(jni_str!("android/content/Intent"))?;
    let intent = env.new_object(
        &intent_cls,
        jni_sig!((java.lang.String)),
        &[JValue::Object(action.as_ref())],
    )?;

    // CATEGORY_OPENABLE
    let cat = env.new_string("android.intent.category.OPENABLE")?;
    env.call_method(
        &intent,
        jni_str!("addCategory"),
        jni_sig!((java.lang.String) -> android.content.Intent),
        &[JValue::Object(cat.as_ref())],
    )?;

    let mime = env.new_string("*/*")?;
    env.call_method(
        &intent,
        jni_str!("setType"),
        jni_sig!((java.lang.String) -> android.content.Intent),
        &[JValue::Object(mime.as_ref())],
    )?;

    // EXTRA_MIME_TYPES: images + freeq-allowed videos (mp4/webm/mov).
    let string_cls = env.find_class(jni_str!("java/lang/String"))?;
    let arr = env.new_object_array(4, &string_cls, jni::objects::JObject::null())?;
    let m0 = env.new_string("image/*")?;
    let m1 = env.new_string("video/mp4")?;
    let m2 = env.new_string("video/webm")?;
    let m3 = env.new_string("video/quicktime")?;
    // Disambiguate `JString: AsRef<_>` (jni has multiple impls) like requestPermissions.
    let m0_obj: &JObject = m0.as_ref();
    let m1_obj: &JObject = m1.as_ref();
    let m2_obj: &JObject = m2.as_ref();
    let m3_obj: &JObject = m3.as_ref();
    env.set_object_array_element(&arr, 0, m0_obj)?;
    env.set_object_array_element(&arr, 1, m1_obj)?;
    env.set_object_array_element(&arr, 2, m2_obj)?;
    env.set_object_array_element(&arr, 3, m3_obj)?;
    let extra = env.new_string("android.intent.extra.MIME_TYPES")?;
    env.call_method(
        &intent,
        jni_str!("putExtra"),
        jni_sig!((java.lang.String, [java.lang.String]) -> android.content.Intent),
        &[
            JValue::Object(extra.as_ref()),
            JValue::Object(arr.as_ref()),
        ],
    )?;

    Ok(intent)
}

/// Whether our NativeActivity is currently resumed (`mResumed` / lifecycle).
///
/// `None` = could not determine (caller should not use resume-cancel).
fn activity_is_resumed() -> Option<bool> {
    let app = android_app()?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return None;
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return None;
    }

    use jni::objects::JObject;
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: live AndroidApp VM.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    let result = vm.attach_current_thread(|env| -> jni::errors::Result<Option<bool>> {
        // SAFETY: activity global ref owned by the runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };

        // Prefer public `isResumed()` when present (API 24+ / some OEMs).
        if let Ok(v) = env.call_method(
            &activity,
            jni_str!("isResumed"),
            jni_sig!(() -> jboolean),
            &[],
        ) {
            if let Ok(b) = v.z() {
                return Ok(Some(b));
            }
        }

        // Fallback: reflect Activity.mResumed (boolean).
        if let Ok(v) = env.get_field(&activity, jni_str!("mResumed"), jni_sig!(jboolean)) {
            if let Ok(b) = v.z() {
                return Ok(Some(b));
            }
        }

        // Last resort: window focus — true when we are the interactive app again.
        if let Ok(v) = env.call_method(
            &activity,
            jni_str!("hasWindowFocus"),
            jni_sig!(() -> jboolean),
            &[],
        ) {
            if let Ok(b) = v.z() {
                return Ok(Some(b));
            }
        }

        Ok(None)
    });

    result.ok().flatten()
}

/// Inspect `ActivityThread` pending results for `PICK_IMAGE_REQ` and consume a
/// match if present. Always returns a definitive outcome when a match is found
/// (never silently drops a cancel / empty selection).
fn scavenge_pick_result() -> Result<RawPick, String> {
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: live AndroidApp VM.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    // Carry the outcome out of the attach closure.
    let mut out: RawPick = RawPick::Waiting;

    let jni_result = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        // SAFETY: activity global ref owned by the runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };

        // ActivityThread.currentActivityThread()
        let at_cls = env.find_class(jni_str!("android/app/ActivityThread"))?;
        let at = env
            .call_static_method(
                &at_cls,
                jni_str!("currentActivityThread"),
                jni_sig!(() -> android.app.ActivityThread),
                &[],
            )?
            .l()?;
        if at.is_null() {
            return Ok(());
        }

        // mActivities: ArrayMap<IBinder, ActivityClientRecord>
        let m_activities = match env.get_field(
            &at,
            jni_str!("mActivities"),
            jni_sig!(android.util.ArrayMap),
        ) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        let m_activities = m_activities.l()?;
        if m_activities.is_null() {
            return Ok(());
        }

        let size = env
            .call_method(&m_activities, jni_str!("size"), jni_sig!(() -> jint), &[])?
            .i()?;

        for i in 0..size {
            let record = env
                .call_method(
                    &m_activities,
                    jni_str!("valueAt"),
                    jni_sig!((jint) -> java.lang.Object),
                    &[JValue::Int(i)],
                )?
                .l()?;
            if record.is_null() {
                continue;
            }

            let act_field = env.get_field(
                &record,
                jni_str!("activity"),
                jni_sig!(android.app.Activity),
            );
            let Ok(act_field) = act_field else { continue };
            let rec_activity = act_field.l()?;
            if rec_activity.is_null() {
                continue;
            }
            let same = env.is_same_object(&rec_activity, activity.as_ref())?;
            if !same {
                continue;
            }

            // pendingResults: List<ResultInfo>
            let pending = env.get_field(
                &record,
                jni_str!("pendingResults"),
                jni_sig!(java.util.List),
            );
            let Ok(pending) = pending else { continue };
            let pending = pending.l()?;
            if pending.is_null() {
                continue;
            }

            let list_size = env
                .call_method(&pending, jni_str!("size"), jni_sig!(() -> jint), &[])?
                .i()?;

            for j in (0..list_size).rev() {
                let info = env
                    .call_method(
                        &pending,
                        jni_str!("get"),
                        jni_sig!((jint) -> java.lang.Object),
                        &[JValue::Int(j)],
                    )?
                    .l()?;
                if info.is_null() {
                    continue;
                }

                let req = env
                    .get_field(&info, jni_str!("mRequestCode"), jni_sig!(jint))?
                    .i()?;
                if req != PICK_IMAGE_REQ {
                    continue;
                }
                let result_code = env
                    .get_field(&info, jni_str!("mResultCode"), jni_sig!(jint))?
                    .i()?;
                let data = env
                    .get_field(
                        &info,
                        jni_str!("mData"),
                        jni_sig!(android.content.Intent),
                    )?
                    .l()?;

                // Consume so the framework doesn't re-deliver oddly.
                let _ = env.call_method(
                    &pending,
                    jni_str!("remove"),
                    jni_sig!((jint) -> java.lang.Object),
                    &[JValue::Int(j)],
                );

                // RESULT_OK = -1, RESULT_CANCELED = 0. Always emit an outcome —
                // silent drop here is what left the compose bar stuck on cancel.
                if result_code == 0 {
                    out = RawPick::Cancelled;
                    return Ok(());
                }
                if result_code != -1 {
                    out = RawPick::Cancelled;
                    return Ok(());
                }
                if data.is_null() {
                    out = RawPick::Cancelled;
                    return Ok(());
                }
                match read_image_bytes_from_result_intent(env, &data) {
                    Ok(Some(bytes)) => out = RawPick::Image(bytes),
                    Ok(None) => out = RawPick::Cancelled,
                    Err(e) => out = RawPick::Failed(e),
                }
                return Ok(());
            }
        }
        Ok(())
    });

    jni_result.map_err(|e| format!("{e}"))?;
    Ok(out)
}

fn read_image_bytes_from_result_intent(
    env: &mut jni::Env<'_>,
    data: &jni::objects::JObject<'_>,
) -> Result<Option<Vec<u8>>, String> {
    use jni::objects::JValue;
    use jni::{jni_sig, jni_str};

    // Intent.getData() -> Uri
    let uri = env
        .call_method(
            data,
            jni_str!("getData"),
            jni_sig!(() -> android.net.Uri),
            &[],
        )
        .map_err(|e| format!("{e}"))?
        .l()
        .map_err(|e| format!("{e}"))?;
    if uri.is_null() {
        // Some pickers put the URI in clip data.
        let clip = env
            .call_method(
                data,
                jni_str!("getClipData"),
                jni_sig!(() -> android.content.ClipData),
                &[],
            )
            .ok()
            .and_then(|v| v.l().ok());
        if let Some(clip) = clip {
            if !clip.is_null() {
                let count = env
                    .call_method(&clip, jni_str!("getItemCount"), jni_sig!(() -> jint), &[])
                    .map_err(|e| format!("{e}"))?
                    .i()
                    .map_err(|e| format!("{e}"))?;
                if count > 0 {
                    let item = env
                        .call_method(
                            &clip,
                            jni_str!("getItemAt"),
                            jni_sig!((jint) -> android.content.ClipData::Item),
                            &[JValue::Int(0)],
                        )
                        .map_err(|e| format!("{e}"))?
                        .l()
                        .map_err(|e| format!("{e}"))?;
                    if !item.is_null() {
                        let uri2 = env
                            .call_method(
                                &item,
                                jni_str!("getUri"),
                                jni_sig!(() -> android.net.Uri),
                                &[],
                            )
                            .map_err(|e| format!("{e}"))?
                            .l()
                            .map_err(|e| format!("{e}"))?;
                        if !uri2.is_null() {
                            return read_bytes_from_uri(env, &uri2).map(Some);
                        }
                    }
                }
            }
        }
        return Ok(None);
    }
    read_bytes_from_uri(env, &uri).map(Some)
}

fn read_bytes_from_uri(
    env: &mut jni::Env<'_>,
    uri: &jni::objects::JObject<'_>,
) -> Result<Vec<u8>, String> {
    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str};

    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }
    // SAFETY: activity global ref owned by the runtime.
    let activity = unsafe {
        env.as_cast_raw::<Global<JObject>>(&activity_ptr)
            .map_err(|e| format!("{e}"))?
    };

    let resolver = env
        .call_method(
            &activity,
            jni_str!("getContentResolver"),
            jni_sig!(() -> android.content.ContentResolver),
            &[],
        )
        .map_err(|e| format!("{e}"))?
        .l()
        .map_err(|e| format!("{e}"))?;

    let stream = env
        .call_method(
            &resolver,
            jni_str!("openInputStream"),
            jni_sig!((android.net.Uri) -> java.io.InputStream),
            &[JValue::Object(uri)],
        )
        .map_err(|e| format!("openInputStream: {e}"))?
        .l()
        .map_err(|e| format!("{e}"))?;
    if stream.is_null() {
        return Err("Could not open selected image".into());
    }

    // Read all bytes via ByteArrayOutputStream.
    let baos_cls = env
        .find_class(jni_str!("java/io/ByteArrayOutputStream"))
        .map_err(|e| format!("{e}"))?;
    let baos = env
        .new_object(&baos_cls, jni_sig!(()), &[])
        .map_err(|e| format!("{e}"))?;

    let buf = env
        .new_byte_array(8192)
        .map_err(|e| format!("{e}"))?;
    loop {
        let n = env
            .call_method(
                &stream,
                jni_str!("read"),
                jni_sig!(([jbyte]) -> jint),
                &[JValue::Object(buf.as_ref())],
            )
            .map_err(|e| format!("read: {e}"))?
            .i()
            .map_err(|e| format!("{e}"))?;
        if n <= 0 {
            break;
        }
        if n as usize > 40 * 1024 * 1024 {
            let _ = env.call_method(&stream, jni_str!("close"), jni_sig!(()), &[]);
            return Err("Image file is too large (max 40MB)".into());
        }
        env.call_method(
            &baos,
            jni_str!("write"),
            jni_sig!(([jbyte], jint, jint)),
            &[JValue::Object(buf.as_ref()), JValue::Int(0), JValue::Int(n)],
        )
        .map_err(|e| format!("write: {e}"))?;
        // Guard total size.
        let size = env
            .call_method(&baos, jni_str!("size"), jni_sig!(() -> jint), &[])
            .map_err(|e| format!("{e}"))?
            .i()
            .map_err(|e| format!("{e}"))?;
        if size > 40 * 1024 * 1024 {
            let _ = env.call_method(&stream, jni_str!("close"), jni_sig!(()), &[]);
            return Err("Image file is too large (max 40MB)".into());
        }
    }
    let _ = env.call_method(&stream, jni_str!("close"), jni_sig!(()), &[]);

    let arr = env
        .call_method(&baos, jni_str!("toByteArray"), jni_sig!(() -> [jbyte]), &[])
        .map_err(|e| format!("{e}"))?
        .l()
        .map_err(|e| format!("{e}"))?;
    if arr.is_null() {
        return Err("Empty image data".into());
    }

    // Convert jbyte array to Vec<u8>.
    let jarr = env
        .cast_local::<jni::objects::JByteArray>(arr)
        .map_err(|e| format!("toByteArray cast: {e}"))?;
    let bytes = env
        .convert_byte_array(&jarr)
        .map_err(|e| format!("{e}"))?;
    Ok(bytes)
}

/// Open an `http(s)://` URL in the system browser via `Intent.ACTION_VIEW`.
///
/// The desktop `open` crate has no Android backend (it shells out to
/// `xdg-open`), so login and other external-link flows must go through JNI.
pub fn open_url(url: &str) -> Result<(), String> {
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        // SAFETY: activity global ref owned by the runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };

        let url_j = env.new_string(url)?;
        let uri_cls = env.find_class(jni_str!("android/net/Uri"))?;
        let uri = env
            .call_static_method(
                &uri_cls,
                jni_str!("parse"),
                jni_sig!((java.lang.String) -> android.net.Uri),
                &[JValue::Object(url_j.as_ref())],
            )?
            .l()?;

        let action = env.new_string("android.intent.action.VIEW")?;
        let intent_cls = env.find_class(jni_str!("android/content/Intent"))?;
        let intent = env.new_object(
            &intent_cls,
            jni_sig!((java.lang.String, android.net.Uri)),
            &[JValue::Object(action.as_ref()), JValue::Object(uri.as_ref())],
        )?;

        // External browser often runs in another task; NEW_TASK is safe from
        // Activity and required if the context is ever non-Activity.
        // Intent.FLAG_ACTIVITY_NEW_TASK = 0x10000000
        env.call_method(
            &intent,
            jni_str!("addFlags"),
            jni_sig!((jint) -> android.content.Intent),
            &[JValue::Int(0x1000_0000)],
        )?;

        env.call_method(
            &activity,
            jni_str!("startActivity"),
            jni_sig!((android.content.Intent)),
            &[JValue::Object(intent.as_ref())],
        )?;
        Ok(())
    })
    .map_err(|e| format!("Failed to open URL: {e}"))
}

/// Take a pending `freeq://…` deep link (OAuth callback), if any.
///
/// Prefers `SleekActivity.takePendingDeepLink()` (set from `onCreate` /
/// `onNewIntent`). Falls back to `Activity.getIntent().getDataString()` for
/// cold-start races, then clears the intent data so we do not re-apply.
pub fn take_pending_deep_link() -> Option<String> {
    let app = android_app()?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return None;
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return None;
    }

    use jni::objects::{JObject, JString, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    let result = vm.attach_current_thread(|env| -> jni::errors::Result<Option<String>> {
        // 1) SleekActivity static (warm onNewIntent + cold onCreate).
        if let Ok(cls) = env.find_class(jni_str!("uk/nandi/sleek/SleekActivity")) {
            if let Ok(v) = env.call_static_method(
                &cls,
                jni_str!("takePendingDeepLink"),
                jni_sig!(() -> java.lang.String),
                &[],
            ) {
                if let Ok(obj) = v.l() {
                    if !obj.is_null() {
                        if let Ok(js) = env.cast_local::<JString>(obj) {
                            let s = format!("{js}");
                            if s.starts_with("freeq://") {
                                return Ok(Some(s));
                            }
                        }
                    }
                }
            }
        }

        // 2) Fallback: current Activity intent data (cold start without static).
        // SAFETY: activity global ref owned by the runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
        let intent = env
            .call_method(
                &activity,
                jni_str!("getIntent"),
                jni_sig!(() -> android.content.Intent),
                &[],
            )?
            .l()?;
        if intent.is_null() {
            return Ok(None);
        }
        let data = env
            .call_method(
                &intent,
                jni_str!("getDataString"),
                jni_sig!(() -> java.lang.String),
                &[],
            )?
            .l()?;
        if data.is_null() {
            return Ok(None);
        }
        let js = match env.cast_local::<JString>(data) {
            Ok(js) => js,
            Err(_) => return Ok(None),
        };
        let s = format!("{js}");
        if !s.starts_with("freeq://") {
            return Ok(None);
        }

        // Clear intent data so a later resume does not re-apply tokens.
        // Pass a null Uri (JNI null jobject).
        let null_uri = JObject::null();
        let _ = env.call_method(
            &intent,
            jni_str!("setData"),
            jni_sig!((android.net.Uri)),
            &[JValue::Object(null_uri.as_ref())],
        );

        Ok(Some(s))
    });

    match result {
        Ok(v) => v,
        Err(e) => {
            log::debug!("take_pending_deep_link: {e}");
            None
        }
    }
}

/// Read plain text from the Android system clipboard (for long-press paste in egui).
pub fn clipboard_get_text() -> Option<String> {
    with_activity_jni(|env, activity| {
        use jni::objects::{JString, JValue};
        use jni::{jni_sig, jni_str};

        let svc = env.new_string("clipboard").map_err(|e| format!("{e}"))?;
        let mgr = env
            .call_method(
                activity,
                jni_str!("getSystemService"),
                jni_sig!((java.lang.String) -> java.lang.Object),
                &[JValue::Object(svc.as_ref())],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if mgr.is_null() {
            return Ok(None);
        }

        let has = env
            .call_method(
                &mgr,
                jni_str!("hasPrimaryClip"),
                jni_sig!(() -> boolean),
                &[],
            )
            .map_err(|e| format!("{e}"))?
            .z()
            .map_err(|e| format!("{e}"))?;
        if !has {
            return Ok(None);
        }

        let clip = env
            .call_method(
                &mgr,
                jni_str!("getPrimaryClip"),
                jni_sig!(() -> android.content.ClipData),
                &[],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if clip.is_null() {
            return Ok(None);
        }

        let count = env
            .call_method(&clip, jni_str!("getItemCount"), jni_sig!(() -> jint), &[])
            .map_err(|e| format!("{e}"))?
            .i()
            .map_err(|e| format!("{e}"))?;
        if count <= 0 {
            return Ok(None);
        }

        let item = env
            .call_method(
                &clip,
                jni_str!("getItemAt"),
                jni_sig!((jint) -> android.content.ClipData::Item),
                &[JValue::Int(0)],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if item.is_null() {
            return Ok(None);
        }

        let seq = env
            .call_method(
                &item,
                jni_str!("coerceToText"),
                jni_sig!((android.content.Context) -> java.lang.CharSequence),
                &[JValue::Object(activity)],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if seq.is_null() {
            return Ok(None);
        }

        let jstr = env
            .call_method(
                &seq,
                jni_str!("toString"),
                jni_sig!(() -> java.lang.String),
                &[],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if jstr.is_null() {
            return Ok(None);
        }

        let js = env
            .cast_local::<JString>(jstr)
            .map_err(|e| format!("{e}"))?;
        let text = format!("{js}");
        if text.is_empty() {
            Ok(None)
        } else {
            Ok(Some(text))
        }
    })
    .ok()
    .flatten()
}

/// Write plain text to the Android system clipboard (copy/cut from egui).
pub fn clipboard_set_text(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    with_activity_jni(|env, activity| {
        use jni::objects::JValue;
        use jni::{jni_sig, jni_str};

        let svc = env.new_string("clipboard").map_err(|e| format!("{e}"))?;
        let mgr = env
            .call_method(
                activity,
                jni_str!("getSystemService"),
                jni_sig!((java.lang.String) -> java.lang.Object),
                &[JValue::Object(svc.as_ref())],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if mgr.is_null() {
            return Err("ClipboardManager missing".into());
        }

        let clip_data = env
            .find_class(jni_str!("android/content/ClipData"))
            .map_err(|e| format!("{e}"))?;
        let label = env.new_string("text").map_err(|e| format!("{e}"))?;
        let payload = env.new_string(text).map_err(|e| format!("{e}"))?;
        let clip = env
            .call_static_method(
                &clip_data,
                jni_str!("newPlainText"),
                jni_sig!(
                    (java.lang.CharSequence, java.lang.CharSequence) -> android.content.ClipData
                ),
                &[
                    JValue::Object(label.as_ref()),
                    JValue::Object(payload.as_ref()),
                ],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if clip.is_null() {
            return Err("ClipData.newPlainText returned null".into());
        }

        env.call_method(
            &mgr,
            jni_str!("setPrimaryClip"),
            jni_sig!((android.content.ClipData)),
            &[JValue::Object(&clip)],
        )
        .map_err(|e| format!("{e}"))?;
        Ok(())
    })
    .is_ok()
}

/// Run a JNI closure with the stored Activity (egui / worker threads).
fn with_activity_jni<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&mut jni::Env<'_>, &jni::objects::JObject<'_>) -> Result<T, String>,
{
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }

    use jni::objects::JObject;
    use jni::refs::Global;
    use jni::JavaVM;

    // SAFETY: vm comes from the live AndroidApp for this process.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };
    vm.attach_current_thread(|env| -> jni::errors::Result<Result<T, String>> {
        // SAFETY: activity is a global ref owned by the Android runtime.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
        Ok(f(env, &activity))
    })
    .map_err(|e| format!("{e}"))?
}

/// Finish the host Activity (leave the app). Used when Android back / Esc is
/// pressed at the root shell (tabs or connect) with nothing left to dismiss.
pub fn finish_activity() {
    match with_activity_jni(|env, activity| {
        use jni::{jni_sig, jni_str};
        env.call_method(activity, jni_str!("finish"), jni_sig!(()), &[])
            .map_err(|e| format!("Activity.finish: {e}"))?;
        Ok(())
    }) {
        Ok(()) => log::info!("android finish_activity: leaving"),
        Err(e) => log::warn!("android finish_activity: {e}"),
    }
}
