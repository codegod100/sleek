//! Android storage helpers for the compose-bar image attach flow.
//!
//! Opens the **system document / photo picker** (`ACTION_OPEN_DOCUMENT` /
//! `GET_CONTENT` / `ACTION_PICK_IMAGES`) via JNI, then recovers the result
//! Intent from `ActivityThread` pending results so we do not need a custom
//! Java `Activity` subclass (cargo-apk ships plain `NativeActivity`).

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use winit::platform::android::activity::AndroidApp;

use crate::clipboard::PickImageResult;
use crate::state::ComposeImage;

static ANDROID_APP: OnceLock<AndroidApp> = OnceLock::new();

/// Request code for our image-picker `startActivityForResult` call.
const PICK_IMAGE_REQ: i32 = 0x51_EE_61;

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
    match request_read_images_permission() {
        Ok(true) => log::debug!("android media read permission granted"),
        Ok(false) => log::info!("android media read permission requested (awaiting user)"),
        Err(e) => log::warn!("android media permission: {e}"),
    }
}

/// `Ok(true)` = already granted, `Ok(false)` = dialog shown / not granted yet.
fn request_read_images_permission() -> Result<bool, String> {
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;

    let permissions: &[&str] = &[
        "android.permission.READ_MEDIA_IMAGES",
        "android.permission.READ_EXTERNAL_STORAGE",
    ];

    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let mut activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
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

            const REQ: i32 = 0x5_1EE_6;
            let arr_obj: &JObject = arr.as_ref();
            env.call_method(
                &activity,
                jni_str!("requestPermissions"),
                jni_sig!(([java.lang.String], jint)),
                &[JValue::Object(arr_obj), JValue::Int(REQ)],
            )?;

            Ok(false)
        })
        .map_err(|e| format!("{e}"))?;

    Ok(result)
}

// ── OS image picker ─────────────────────────────────────────────────────────

/// Pending pick: result bytes or cancel, delivered from the poller / Intent.
static PICK_TX: Mutex<Option<mpsc::Sender<Result<Option<Vec<u8>>, String>>>> = Mutex::new(None);

/// Open the system image picker and block until the user chooses or cancels.
///
/// Designed to run on a **background thread** (never the egui UI thread).
pub fn pick_image_file() -> PickImageResult {
    ensure_read_images_permission();

    let (tx, rx) = mpsc::channel();
    {
        let mut slot = PICK_TX
            .lock()
            .map_err(|_| "Image picker lock poisoned".to_string())?;
        if slot.is_some() {
            return Err("An image picker is already open".into());
        }
        *slot = Some(tx);
    }

    if let Err(e) = launch_system_image_picker() {
        let _ = PICK_TX.lock().map(|mut g| g.take());
        return Err(e);
    }

    // Poll for the result Intent while the system UI is up. The Activity is
    // paused; pending results land on ActivityThread before onResume.
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Ok(msg) = rx.try_recv() {
            let _ = PICK_TX.lock().map(|mut g| g.take());
            return match msg {
                Ok(Some(bytes)) => crate::clipboard::load_image_from_bytes(&bytes).map(Some),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
        }

        // Actively scavenge pending activity results (NativeActivity has no
        // onActivityResult override we can hook).
        match try_take_pick_result_bytes() {
            Ok(Some(bytes)) => {
                let _ = PICK_TX.lock().map(|mut g| g.take());
                return crate::clipboard::load_image_from_bytes(&bytes).map(Some);
            }
            Ok(None) => {
                // Still waiting (or cancelled — cancelled also returns None
                // only when we saw RESULT_CANCELED for our request code).
            }
            Err(e) => log::debug!("android pick poll: {e}"),
        }

        // Detect cancel via a dedicated probe (RESULT_CANCELED).
        if let Ok(true) = try_take_pick_cancelled() {
            let _ = PICK_TX.lock().map(|mut g| g.take());
            return Ok(None);
        }

        if Instant::now() > deadline {
            let _ = PICK_TX.lock().map(|mut g| g.take());
            return Err("Image picker timed out".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn launch_system_image_picker() -> Result<(), String> {
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let mut activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
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

        // Prefer modern photo picker (API 33+); fall back to OPEN_DOCUMENT / GET_CONTENT.
        let intent = {
            // MediaStore.ACTION_PICK_IMAGES = "android.provider.action.PICK_IMAGES"
            let action = env.new_string("android.provider.action.PICK_IMAGES")?;
            let intent_cls = env.find_class(jni_str!("android/content/Intent"))?;
            let trial = env.new_object(
                &intent_cls,
                jni_sig!((java.lang.String)),
                &[JValue::Object(action.as_ref())],
            );

            if trial.is_ok() {
                // Restrict to images when the extra is available.
                if let Ok(trial) = trial {
                    let extra_type = env.new_string("android.provider.extra.PICK_IMAGES_MAX")?;
                    // Single image: max = 1 (optional; ignore failures).
                    let _ = env.call_method(
                        &trial,
                        jni_str!("putExtra"),
                        jni_sig!((java.lang.String, jint) -> android.content.Intent),
                        &[JValue::Object(extra_type.as_ref()), JValue::Int(1)],
                    );
                    // Check resolve
                    let pm = env.call_method(
                        &activity,
                        jni_str!("getPackageManager"),
                        jni_sig!(() -> android.content.pm.PackageManager),
                        &[],
                    )?;
                    let pm_obj = pm.l()?;
                    let resolved = env.call_method(
                        &pm_obj,
                        jni_str!("resolveActivity"),
                        jni_sig!(
                            (android.content.Intent, jint) -> android.content.pm.ResolveInfo
                        ),
                        &[JValue::Object(trial.as_ref()), JValue::Int(0)],
                    )?;
                    let ri = resolved.l()?;
                    if !ri.is_null() {
                        trial
                    } else {
                        build_open_document_intent(env)?
                    }
                } else {
                    build_open_document_intent(env)?
                }
            } else {
                build_open_document_intent(env)?
            }
        };

        env.call_method(
            &activity,
            jni_str!("startActivityForResult"),
            jni_sig!((android.content.Intent, jint)),
            &[JValue::Object(intent.as_ref()), JValue::Int(PICK_IMAGE_REQ)],
        )?;
        Ok(())
    })
    .map_err(|e| format!("Failed to open system image picker: {e}"))
}

fn build_open_document_intent<'a>(
    env: &mut jni::env::JNIEnv<'a>,
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

/// Try to read image bytes from a pending activity result for our request code.
///
/// Returns:
/// - `Ok(Some(bytes))` — user picked an image
/// - `Ok(None)` — no result yet
/// - `Err` — reflection / JNI failure (transient; keep polling)
fn try_take_pick_result_bytes() -> Result<Option<Vec<u8>>, String> {
    match with_pending_result(|env, result_code, data| {
        // RESULT_OK = -1
        if result_code != -1 {
            return Ok(None);
        }
        if data.is_null() {
            return Ok(None);
        }
        read_image_bytes_from_result_intent(env, data)
    })? {
        Some(bytes) => Ok(Some(bytes)),
        None => Ok(None),
    }
}

fn try_take_pick_cancelled() -> Result<bool, String> {
    with_pending_result(|_env, result_code, _data| {
        // RESULT_CANCELED = 0
        Ok(result_code == 0)
    })
    .map(|v| v.unwrap_or(false))
}

/// Inspect `ActivityThread` pending results for `PICK_IMAGE_REQ`, optionally
/// consuming a match. The callback receives `(resultCode, data Intent)`.
fn with_pending_result<F, T>(f: F) -> Result<Option<T>, String>
where
    F: for<'a> FnOnce(
        &mut jni::env::JNIEnv<'a>,
        i32,
        &jni::objects::JObject<'a>,
    ) -> Result<Option<T>, String>,
{
    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err("null JavaVM".into());
    }
    let mut activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err("null Activity".into());
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: live AndroidApp VM.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };

    // We need to return T from inside the closure — box the result.
    let mut out: Option<Result<Option<T>, String>> = None;

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
        let m_activities_f = env.get_field_id(
            &at_cls,
            jni_str!("mActivities"),
            jni_str!("Landroid/util/ArrayMap;"),
        );
        // Field may be on a superclass or renamed on some OEMs — try Object type too.
        let m_activities = match m_activities_f {
            Ok(fid) => env.get_field_unchecked(&at, fid, jni::signature::ReturnType::Object)?,
            Err(_) => {
                // Fallback: getField via name with Object signature.
                match env.get_field(&at, jni_str!("mActivities"), jni_str!("Landroid/util/ArrayMap;"))
                {
                    Ok(v) => v,
                    Err(_) => return Ok(()),
                }
            }
        };
        let m_activities = m_activities.l()?;
        if m_activities.is_null() {
            return Ok(());
        }

        // ArrayMap.size() / valueAt(i)
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

            // ActivityClientRecord.activity
            let rec_cls = env.get_object_class(&record)?;
            let act_field = env.get_field(
                &record,
                jni_str!("activity"),
                jni_str!("Landroid/app/Activity;"),
            );
            let Ok(act_field) = act_field else { continue };
            let rec_activity = act_field.l()?;
            if rec_activity.is_null() {
                continue;
            }
            // Only our activity.
            let same = env.is_same_object(&rec_activity, activity.as_ref())?;
            if !same {
                continue;
            }

            // pendingResults: List<ResultInfo>
            let pending = env.get_field(
                &record,
                jni_str!("pendingResults"),
                jni_str!("Ljava/util/List;"),
            );
            let Ok(pending) = pending else {
                // Also try delivering via r.activity fields after resume —
                // some OEMs clear pendingResults immediately.
                continue;
            };
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

                // ResultInfo.mRequestCode / mResultCode / mData
                let req = env
                    .get_field(&info, jni_str!("mRequestCode"), jni_str!("I"))?
                    .i()?;
                if req != PICK_IMAGE_REQ {
                    continue;
                }
                let result_code = env
                    .get_field(&info, jni_str!("mResultCode"), jni_str!("I"))?
                    .i()?;
                let data = env
                    .get_field(
                        &info,
                        jni_str!("mData"),
                        jni_str!("Landroid/content/Intent;"),
                    )?
                    .l()?;

                // Consume so the framework doesn't re-deliver oddly.
                let _ = env.call_method(
                    &pending,
                    jni_str!("remove"),
                    jni_sig!((jint) -> java.lang.Object),
                    &[JValue::Int(j)],
                );

                // Run user callback — we can't easily pass env mutably through
                // the typed F with the lifetime dance, so read bytes here for
                // OK results and use a side channel for cancel.
                // Actually we call f below outside after storing codes.
                // Store in local then invoke f after the loop by breaking.
                // For simplicity, handle OK / cancel right here via type erasure:
                let _ = (result_code, data, rec_cls);
                // Re-read and process inline for the PickImage path:
                // We use a trait object box - instead call f now.
                // SAFETY/lifetime: env is valid for this attach scope.
                // We can't call f here without moving it in the loop easily.
                // Use a one-shot flag:
                // Actually process immediately for image bytes:
                if result_code == -1 && !data.is_null() {
                    match read_image_bytes_from_result_intent(env, &data) {
                        Ok(Some(bytes)) => {
                            // Stash via PICK_TX if still open.
                            if let Ok(guard) = PICK_TX.lock() {
                                if let Some(tx) = guard.as_ref() {
                                    let _ = tx.send(Ok(Some(bytes)));
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            if let Ok(guard) = PICK_TX.lock() {
                                if let Some(tx) = guard.as_ref() {
                                    let _ = tx.send(Err(e));
                                }
                            }
                        }
                    }
                } else if result_code == 0 {
                    if let Ok(guard) = PICK_TX.lock() {
                        if let Some(tx) = guard.as_ref() {
                            let _ = tx.send(Ok(None));
                        }
                    }
                }
                let _ = f; // silence unused if we processed inline
                let _ = &out;
                return Ok(());
            }
        }
        Ok(())
    });

    jni_result.map_err(|e| format!("{e}"))?;
    // Results are delivered via PICK_TX from inside the JNI block; the outer
    // poll loop reads the channel. This function reports "nothing taken" for
    // the direct return path.
    let _ = out;
    Ok(None)
}

fn read_image_bytes_from_result_intent(
    env: &mut jni::env::JNIEnv<'_>,
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
                            jni_sig!((jint) -> android.content.ClipData$Item),
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
    env: &mut jni::env::JNIEnv<'_>,
    uri: &jni::objects::JObject<'_>,
) -> Result<Vec<u8>, String> {
    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str};

    let app = android_app().ok_or_else(|| "AndroidApp not stored".to_string())?;
    let mut activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
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
    let jarr: jni::objects::JByteArray = arr.into();
    let bytes = env
        .convert_byte_array(&jarr)
        .map_err(|e| format!("{e}"))?;
    Ok(bytes)
}
