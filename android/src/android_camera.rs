//! Android Camera2 → MoQ video publish bridge.
//!
//! Java `CameraCapture` (in APK `classes.dex`) opens Camera2 / ImageReader and
//! calls native NV12 push methods implemented here. Frames land in a
//! latest-only [`PushCameraSource`] that implements iroh-live's [`VideoSource`].
//!
//! JNI note: never resolve `CameraCapture` with `Env::find_class` from a
//! native worker thread — that uses the system ClassLoader and misses APK
//! classes. Use [`android_media::load_app_class`] (Activity ClassLoader).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use iroh_live::media::{
    format::{Nv12Planes, PixelFormat, VideoFormat, VideoFrame},
    traits::VideoSource,
};

use crate::android_media;

/// Java binary name for the Camera2 helper in APK `classes.dex`.
const CAMERA_CAPTURE_CLASS: &str = "uk.nandi.sleek.CameraCapture";

/// Shared sink written by JNI and read by the encoder thread.
struct FrameSink {
    pending: Mutex<Option<VideoFrame>>,
    format: Mutex<VideoFormat>,
    /// Set true after Java reports the capture session is live.
    opened: AtomicBool,
    /// Last open error detail (empty when ok / idle).
    last_error: Mutex<String>,
    frames_pushed: AtomicU64,
}

impl FrameSink {
    fn new(width: u32, height: u32) -> Self {
        Self {
            pending: Mutex::new(None),
            format: Mutex::new(VideoFormat {
                // Encoders that receive FrameData::Nv12 ignore this field;
                // keep Rgba as the VideoFormat default (rusty-codecs has no Nv12 variant).
                pixel_format: PixelFormat::Rgba,
                dimensions: [width, height],
            }),
            opened: AtomicBool::new(false),
            last_error: Mutex::new(String::new()),
            frames_pushed: AtomicU64::new(0),
        }
    }
}

static SINK: OnceLock<Arc<FrameSink>> = OnceLock::new();

fn sink() -> Arc<FrameSink> {
    SINK.get_or_init(|| Arc::new(FrameSink::new(640, 480)))
        .clone()
}

/// Latest-frame-only [`VideoSource`] fed by Camera2 JNI callbacks.
pub struct PushCameraSource {
    sink: Arc<FrameSink>,
    name: String,
}

impl PushCameraSource {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            sink: sink(),
            name: label.into(),
        }
    }
}

impl VideoSource for PushCameraSource {
    fn name(&self) -> &str {
        &self.name
    }

    fn format(&self) -> VideoFormat {
        self.sink
            .format
            .lock()
            .map(|g| g.clone())
            .unwrap_or(VideoFormat {
                pixel_format: PixelFormat::Rgba,
                dimensions: [640, 480],
            })
    }

    fn pop_frame(&mut self) -> Result<Option<VideoFrame>> {
        Ok(self.sink.pending.lock().ok().and_then(|mut g| g.take()))
    }

    fn start(&mut self) -> Result<()> {
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if let Ok(mut g) = self.sink.pending.lock() {
            *g = None;
        }
        Ok(())
    }
}

/// Enumerate cameras for the device picker (front first).
/// Returns `(id, label)` pairs.
pub fn list_cameras() -> Vec<(String, String)> {
    match list_cameras_jni() {
        Ok(list) => list,
        Err(e) => {
            // Warn: empty list makes AV report "camera unavailable" permanently.
            log::warn!("android camera list: {e}");
            Vec::new()
        }
    }
}

fn list_cameras_jni() -> Result<Vec<(String, String)>> {
    let app = android_media::android_app_handle().context("AndroidApp not stored")?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err(anyhow!("null JavaVM"));
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err(anyhow!("null Activity"));
    }

    use jni::objects::{JObject, JObjectArray, JString, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };
    let mut out: Option<Result<Vec<(String, String)>>> = None;
    vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
        let cls = match android_media::load_app_class(env, activity.as_ref(), CAMERA_CAPTURE_CLASS)
        {
            Ok(c) => c,
            Err(e) => {
                out = Some(Err(anyhow!("CameraCapture class: {e}")));
                return Ok(());
            }
        };
        let arr_obj = env
            .call_static_method(
                &cls,
                jni_str!("listCameras"),
                jni_sig!((android.content.Context) -> [java.lang.String]),
                &[JValue::Object(activity.as_ref())],
            )?
            .l()?;
        let arr = env.cast_local::<JObjectArray>(arr_obj)?;
        let n = arr.len(env)?;
        let mut list = Vec::with_capacity(n);
        for i in 0..n {
            let obj = arr.get_element(env, i)?;
            if obj.is_null() {
                continue;
            }
            let jstr = env.cast_local::<JString>(obj)?;
            let s = format!("{jstr}");
            let (id, name) = match s.split_once('\t') {
                Some((id, name)) => (id.to_string(), name.to_string()),
                None => (s.clone(), s),
            };
            list.push((id, name));
        }
        out = Some(Ok(list));
        Ok(())
    })
    .map_err(|e| anyhow!("list cameras JNI: {e}"))?;
    out.unwrap_or_else(|| Err(anyhow!("list cameras JNI: no result")))
}

/// Start Camera2 capture. Returns when the open request is dispatched (session
/// readiness is async — wait with [`wait_until_opened`]).
pub fn start_capture(camera_id: Option<&str>) -> Result<()> {
    let s = sink();
    s.opened.store(false, Ordering::Relaxed);
    s.frames_pushed.store(0, Ordering::Relaxed);
    if let Ok(mut e) = s.last_error.lock() {
        e.clear();
    }
    if let Ok(mut p) = s.pending.lock() {
        *p = None;
    }

    let app = android_media::android_app_handle().context("AndroidApp not stored")?;
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return Err(anyhow!("null JavaVM"));
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return Err(anyhow!("null Activity"));
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    let id = camera_id.unwrap_or("").to_string();
    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };
    let mut start_err: Option<anyhow::Error> = None;
    vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
        let cls = match android_media::load_app_class(env, activity.as_ref(), CAMERA_CAPTURE_CLASS)
        {
            Ok(c) => c,
            Err(e) => {
                start_err = Some(anyhow!("CameraCapture class: {e}"));
                return Ok(());
            }
        };
        let jid = env.new_string(&id)?;
        env.call_static_method(
            &cls,
            jni_str!("start"),
            jni_sig!((android.app.Activity, java.lang.String) -> void),
            &[JValue::Object(activity.as_ref()), JValue::Object(&jid)],
        )?;
        Ok(())
    })
    .map_err(|e| anyhow!("CameraCapture.start: {e}"))?;
    if let Some(e) = start_err {
        return Err(e).context("CameraCapture.start");
    }
    Ok(())
}

/// Block briefly until Java reports the capture session is live.
pub fn wait_until_opened(timeout: Duration) -> Result<()> {
    let s = sink();
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if s.opened.load(Ordering::Relaxed) {
            return Ok(());
        }
        let err = s
            .last_error
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        if !err.is_empty() {
            return Err(anyhow!("camera open failed: {err}"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let err = s
        .last_error
        .lock()
        .ok()
        .map(|g| g.clone())
        .unwrap_or_default();
    if s.opened.load(Ordering::Relaxed) {
        Ok(())
    } else if !err.is_empty() {
        Err(anyhow!("camera open failed: {err}"))
    } else {
        Err(anyhow!("camera open timed out"))
    }
}

/// Stop Camera2 capture (idempotent).
pub fn stop_capture() {
    let Some(app) = android_media::android_app_handle() else {
        return;
    };
    let vm_ptr = app.vm_as_ptr();
    if vm_ptr.is_null() {
        return;
    }
    let activity_ptr = app.activity_as_ptr() as jni::sys::jobject;
    if activity_ptr.is_null() {
        return;
    }

    use jni::objects::{JObject, JValue};
    use jni::refs::Global;
    use jni::{jni_sig, jni_str, JavaVM};

    // SAFETY: vm from live AndroidApp.
    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };
    let _ = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
        let cls = match android_media::load_app_class(env, activity.as_ref(), CAMERA_CAPTURE_CLASS)
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!("android camera stop: CameraCapture class: {e}");
                return Ok(());
            }
        };
        env.call_static_method(
            &cls,
            jni_str!("stop"),
            jni_sig!((android.app.Activity) -> void),
            &[JValue::Object(activity.as_ref())],
        )?;
        Ok(())
    });
    let s = sink();
    s.opened.store(false, Ordering::Relaxed);
    if let Ok(mut p) = s.pending.lock() {
        *p = None;
    };
}

/// RAII guard that stops Camera2 when the media session ends.
pub struct CameraCaptureGuard;

impl Drop for CameraCaptureGuard {
    fn drop(&mut self) {
        stop_capture();
        log::info!("android camera: capture stopped (guard drop)");
    }
}

// ── JNI callbacks from CameraCapture.java ─────────────────────────────────

#[unsafe(no_mangle)]
pub extern "system" fn Java_uk_nandi_sleek_CameraCapture_onNv12Frame<'local>(
    mut unowned_env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    y_data: jni::objects::JByteArray<'local>,
    uv_data: jni::objects::JByteArray<'local>,
    width: jni::sys::jint,
    height: jni::sys::jint,
    y_stride: jni::sys::jint,
    uv_stride: jni::sys::jint,
) {
    if width <= 0 || height <= 0 {
        return;
    }
    let _ = unowned_env
        .with_env(|env| -> jni::errors::Result<()> {
            let y_bytes = env.convert_byte_array(&y_data)?;
            let uv_bytes = env.convert_byte_array(&uv_data)?;
            let w = width as u32;
            let h = height as u32;
            let frame = VideoFrame::new_nv12(
                Nv12Planes {
                    y_data: y_bytes,
                    y_stride: y_stride as u32,
                    uv_data: uv_bytes,
                    uv_stride: uv_stride as u32,
                    width: w,
                    height: h,
                },
                Duration::ZERO,
            );
            let s = sink();
            if let Ok(mut fmt) = s.format.lock() {
                if fmt.dimensions != [w, h] {
                    *fmt = VideoFormat {
                        pixel_format: PixelFormat::Rgba,
                        dimensions: [w, h],
                    };
                }
            }
            if let Ok(mut pending) = s.pending.lock() {
                *pending = Some(frame);
            }
            let n = s.frames_pushed.fetch_add(1, Ordering::Relaxed);
            if n == 0 {
                log::info!("android camera: first NV12 frame {w}x{h}");
            }
            Ok(())
        })
        .into_outcome();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_uk_nandi_sleek_CameraCapture_onCameraState<'local>(
    mut unowned_env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    opened: jni::sys::jboolean,
    detail: jni::objects::JString<'local>,
) {
    let _ = unowned_env
        .with_env(|env| -> jni::errors::Result<()> {
            let detail_str = format!("{detail}");
            let s = sink();
            let ok = opened != jni::sys::JNI_FALSE;
            s.opened.store(ok, Ordering::Relaxed);
            if let Ok(mut e) = s.last_error.lock() {
                if ok {
                    e.clear();
                } else {
                    *e = detail_str.clone();
                }
            }
            if ok {
                log::info!("android camera: opened ({detail_str})");
            } else {
                log::warn!("android camera: not opened ({detail_str})");
            }
            Ok(())
        })
        .into_outcome();
}
