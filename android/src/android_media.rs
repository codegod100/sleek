//! Android storage helpers for the compose-bar image attach flow.
//!
//! Opens the **system document / photo picker** (`ACTION_OPEN_DOCUMENT` /
//! `GET_CONTENT` / `ACTION_PICK_IMAGES`) via JNI, then recovers the result
//! Intent from `ActivityThread` pending results so we do not need a custom
//! Java `Activity` subclass (cargo-apk ships plain `NativeActivity`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use winit::platform::android::activity::AndroidApp;

use crate::clipboard::PickImageResult;

static ANDROID_APP: OnceLock<AndroidApp> = OnceLock::new();

/// Request code for Activity-path `startActivityForResult` (scavenger fallback).
const PICK_IMAGE_REQ: i32 = 0x51_EE_61;
/// Fragment path uses a 16-bit code (`Fragment.startActivityForResult` masks
/// with `0xffff`). Must match `PickFragment.REQ` in the embedded dex.
#[allow(dead_code)]
const PICK_FRAGMENT_REQ: i32 = 0x1_EE_6;

/// Stash the `AndroidApp` from `android_main` so path/permission helpers can use it.
pub fn set_android_app(app: AndroidApp) {
    let _ = ANDROID_APP.set(app);
}

fn android_app() -> Option<&'static AndroidApp> {
    ANDROID_APP.get()
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
    match request_permissions(&["android.permission.RECORD_AUDIO"], 0x5_1EE_7) {
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

/// `Ok(true)` = at least one listed permission already granted (or none needed).
/// `Ok(false)` = dialog shown / not granted yet.
///
/// For photo-read we treat *any* of the alternatives as sufficient (API 33+
/// uses `READ_MEDIA_IMAGES`; older uses `READ_EXTERNAL_STORAGE`). For mic we
/// pass a single permission.
fn request_permissions(permissions: &[&str], request_code: i32) -> Result<bool, String> {
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

            // Alternatives (e.g. READ_MEDIA_IMAGES | READ_EXTERNAL_STORAGE): any
            // one grant is enough. Single-permission lists (mic) require that one.
            let mut need: Vec<String> = Vec::new();
            for perm in permissions {
                let jperm = env.new_string(*perm)?;
                let res = env
                    .call_method(
                        &activity,
                        jni_str!("checkSelfPermission"),
                        jni_sig!((java.lang.String) -> jint),
                        &[JValue::Object(jperm.as_ref())],
                    )?
                    .i()?;
                // PackageManager.PERMISSION_GRANTED == 0
                if res == 0 {
                    return Ok(true);
                }
                need.push((*perm).to_string());
            }

            if need.is_empty() {
                return Ok(true);
            }

            let empty = env.new_string("")?;
            let empty_obj: &JObject = empty.as_ref();
            let arr = env.new_object_array(
                need.len() as jni::sys::jsize,
                jni_str!("java/lang/String"),
                empty_obj,
            )?;
            for (i, perm) in need.iter().enumerate() {
                let jperm = env.new_string(perm)?;
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

            Ok(false)
        })
        .map_err(|e| format!("{e}"))?;

    Ok(result)
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

/// Open the system image picker and block until the user chooses or cancels.
///
/// Designed to run on a **background thread** (never the egui UI thread).
pub fn pick_image_file() -> PickImageResult {
    ensure_read_images_permission();

    {
        let mut busy = PICK_BUSY
            .lock()
            .map_err(|_| "Image picker lock poisoned".to_string())?;
        if *busy {
            return Err("An image picker is already open".into());
        }
        *busy = true;
    }

    let finish = |outcome: PickImageResult| -> PickImageResult {
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
                return finish(crate::clipboard::load_image_from_bytes(&bytes).map(Some));
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
                    return finish(crate::clipboard::load_image_from_bytes(&bytes).map(Some));
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
    activity: &jni::objects::JObject<'_>,
) -> Result<jni::objects::JObject<'a>, String> {
    use jni::objects::JValue;
    use jni::{jni_sig, jni_str};

    // Prefer modern photo picker (API 33+); fall back to OPEN_DOCUMENT.
    let action = env
        .new_string("android.provider.action.PICK_IMAGES")
        .map_err(|e| format!("{e}"))?;
    let intent_cls = env
        .find_class(jni_str!("android/content/Intent"))
        .map_err(|e| format!("{e}"))?;
    let trial = env.new_object(
        &intent_cls,
        jni_sig!((java.lang.String)),
        &[JValue::Object(action.as_ref())],
    );

    if let Ok(trial) = trial {
        let extra_type = env
            .new_string("android.provider.extra.PICK_IMAGES_MAX")
            .map_err(|e| format!("{e}"))?;
        let _ = env.call_method(
            &trial,
            jni_str!("putExtra"),
            jni_sig!((java.lang.String, jint) -> android.content.Intent),
            &[JValue::Object(extra_type.as_ref()), JValue::Int(1)],
        );
        let pm = env
            .call_method(
                activity,
                jni_str!("getPackageManager"),
                jni_sig!(() -> android.content.pm.PackageManager),
                &[],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        let resolved = env
            .call_method(
                &pm,
                jni_str!("resolveActivity"),
                jni_sig!((android.content.Intent, jint) -> android.content.pm.ResolveInfo),
                &[JValue::Object(trial.as_ref()), JValue::Int(0)],
            )
            .map_err(|e| format!("{e}"))?
            .l()
            .map_err(|e| format!("{e}"))?;
        if !resolved.is_null() {
            return Ok(trial);
        }
    }

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
    use jni::objects::JValue;
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

    let mime = env.new_string("image/*")?;
    env.call_method(
        &intent,
        jni_str!("setType"),
        jni_sig!((java.lang.String) -> android.content.Intent),
        &[JValue::Object(mime.as_ref())],
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
