//! Android soft-keyboard IME bridge (Gboard swipe / composing text).
//!
//! NativeActivity shows the keyboard but has no `InputConnection`, so glide typing
//! never reaches egui. `SleekIme.java` hosts a hidden `EditText` and forwards
//! composing/commit events here; we inject them as `Event::Ime` each frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use eframe::egui::{self, Event, ImeEvent, Key, Modifiers};

use crate::android_media::{android_app_handle, load_app_class};

static IME_QUEUE: Mutex<Vec<QueuedIme>> = Mutex::new(Vec::new());
static LAST_ACTIVE: Mutex<Option<bool>> = Mutex::new(None);
static LAST_SYNC: Mutex<Option<(String, usize, usize)>> = Mutex::new(None);
/// Gboard has an active composing span — do not mirror egui→Java (clobbers glide).
static COMPOSING: AtomicBool = AtomicBool::new(false);

enum QueuedIme {
    Enabled,
    Disabled,
    Preedit(String),
    Commit(String),
    Backspace,
    Delete,
}

/// Push queued IME events into egui (call once per frame before UI).
pub fn drain_ime_events(ctx: &egui::Context) {
    let events: Vec<QueuedIme> = {
        let mut q = IME_QUEUE.lock().expect("ime queue");
        std::mem::take(&mut *q)
    };
    if events.is_empty() {
        return;
    }
    ctx.input_mut(|i| {
        for ev in events {
            match ev {
                QueuedIme::Enabled => i.events.push(Event::Ime(ImeEvent::Enabled)),
                QueuedIme::Disabled => i.events.push(Event::Ime(ImeEvent::Disabled)),
                QueuedIme::Preedit(text) => {
                    i.events.push(Event::Ime(ImeEvent::Preedit(text)));
                }
                QueuedIme::Commit(text) => {
                    // Refresh ime_cursor_range before Commit. egui only inserts when
                    // cursor_range.secondary == ime_cursor_range.secondary; after
                    // backspace-to-empty that range is often stale, so the first
                    // commitText is dropped and only a later preedit/commit works.
                    i.events.push(Event::Ime(ImeEvent::Enabled));
                    i.events.push(Event::Ime(ImeEvent::Commit(text)));
                }
                QueuedIme::Backspace => {
                    // Disable IME filtering so Key::Backspace is not stripped, then
                    // delete. Commit path re-Enables as needed.
                    i.events.push(Event::Ime(ImeEvent::Disabled));
                    i.events.push(Event::Key {
                        modifiers: Modifiers::NONE,
                        physical_key: Some(Key::Backspace),
                        pressed: true,
                        repeat: false,
                        key: Key::Backspace,
                    });
                    i.events.push(Event::Key {
                        modifiers: Modifiers::NONE,
                        physical_key: Some(Key::Backspace),
                        pressed: false,
                        repeat: false,
                        key: Key::Backspace,
                    });
                }
                QueuedIme::Delete => {
                    i.events.push(Event::Ime(ImeEvent::Disabled));
                    i.events.push(Event::Key {
                        modifiers: Modifiers::NONE,
                        physical_key: Some(Key::Delete),
                        pressed: true,
                        repeat: false,
                        key: Key::Delete,
                    });
                    i.events.push(Event::Key {
                        modifiers: Modifiers::NONE,
                        physical_key: Some(Key::Delete),
                        pressed: false,
                        repeat: false,
                        key: Key::Delete,
                    });
                }
            }
        }
    });
}

/// Mirror egui compose text into the hidden editor for Gboard glide context.
/// Only runs after the bridge is active (keyboard shown) to avoid racing setActive.
/// Skips while Gboard is composing so we never advance LAST_SYNC on a no-op sync.
pub fn sync_field_text(text: &str, sel_start: usize, sel_end: usize) {
    if *LAST_ACTIVE.lock().expect("ime active") != Some(true) {
        return;
    }
    if COMPOSING.load(Ordering::Relaxed) {
        return;
    }
    let key = (text.to_string(), sel_start, sel_end);
    {
        let mut last = LAST_SYNC.lock().expect("ime sync");
        if last.as_ref() == Some(&key) {
            return;
        }
        *last = Some(key.clone());
    }
    if let Err(e) = call_sync_field(&key.0, key.1, key.2) {
        log::debug!("android IME syncField: {e}");
    }
}

/// Wire egui-winit IME allow/deny to the Java bridge.
///
/// Returns `true` when the Java `EditText` bridge owns show/hide so winit does
/// **not** also call NativeActivity `set_ime_allowed` (that dual path steals
/// the `InputConnection` after the first commit and kills multi-word glide).
/// Falls back to winit only if the JNI bridge is unavailable.
pub fn set_ime_allowed(allowed: bool) -> bool {
    let mut last = LAST_ACTIVE.lock().expect("ime active");
    if *last == Some(allowed) {
        // Still claim ownership so winit never re-enters NativeActivity IME.
        return true;
    }
    if allowed {
        // Do NOT push ImeEvent::Enabled here. egui TextEdit strips Key::Backspace
        // while ime_enabled; Gboard soft/hardware DEL must remain effective after
        // typing. Enable only when a real composing session starts (see onPreedit).
    } else {
        push_ime(QueuedIme::Disabled);
        COMPOSING.store(false, Ordering::Relaxed);
        *LAST_SYNC.lock().expect("ime sync") = None;
    }
    match call_set_active(allowed) {
        Ok(()) => {
            *last = Some(allowed);
            log::info!("android IME setActive({allowed}) dispatched (java-only)");
            true
        }
        Err(e) => {
            log::warn!("android IME setActive({allowed}): {e}");
            false
        }
    }
}

fn push_ime(ev: QueuedIme) {
    IME_QUEUE.lock().expect("ime queue").push(ev);
}

fn call_set_active(active: bool) -> Result<(), String> {
    let app = android_app_handle().ok_or("AndroidApp not set")?;
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

    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };
    let mut err: Option<String> = None;
    vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
        let cls = match load_app_class(env, activity.as_ref(), "uk.nandi.sleek.SleekIme") {
            Ok(c) => c,
            Err(e) => {
                err = Some(format!("SleekIme class: {e}"));
                return Ok(());
            }
        };
        env.call_static_method(
            &cls,
            jni_str!("setActive"),
            jni_sig!((android.app.Activity, boolean) -> void),
            &[JValue::Object(activity.as_ref()), JValue::Bool(active)],
        )?;
        Ok(())
    })
    .map_err(|e| format!("JNI setActive: {e}"))?;
    if let Some(e) = err {
        return Err(e);
    }
    Ok(())
}

fn call_sync_field(text: &str, sel_start: usize, sel_end: usize) -> Result<(), String> {
    let app = android_app_handle().ok_or("AndroidApp not set")?;
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

    let vm = unsafe { JavaVM::from_raw(vm_ptr.cast()) };
    let start = i32::try_from(sel_start.min(i32::MAX as usize)).unwrap_or(i32::MAX);
    let end = i32::try_from(sel_end.min(i32::MAX as usize)).unwrap_or(i32::MAX);
    let mut err: Option<String> = None;
    vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&activity_ptr)? };
        let cls = match load_app_class(env, activity.as_ref(), "uk.nandi.sleek.SleekIme") {
            Ok(c) => c,
            Err(e) => {
                err = Some(format!("SleekIme class: {e}"));
                return Ok(());
            }
        };
        let jtext = env.new_string(text)?;
        env.call_static_method(
            &cls,
            jni_str!("syncField"),
            jni_sig!((android.app.Activity, java.lang.String, int, int) -> void),
            &[
                JValue::Object(activity.as_ref()),
                JValue::Object(&jtext),
                JValue::Int(start),
                JValue::Int(end),
            ],
        )?;
        Ok(())
    })
    .map_err(|e| format!("JNI syncField: {e}"))?;
    if let Some(e) = err {
        return Err(e);
    }
    Ok(())
}

// ── JNI callbacks from SleekIme.java ─────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "system" fn Java_uk_nandi_sleek_SleekIme_onPreedit<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    text: jni::objects::JString<'local>,
) {
    let _ = env.with_env(|_env| -> jni::errors::Result<()> {
        let s = format!("{text}");
        let composing = !s.is_empty();
        if composing && !COMPOSING.load(Ordering::Relaxed) {
            // First preedit of a glide/compose session — enable IME ordering
            // without leaving ime_enabled stuck on after commit.
            push_ime(QueuedIme::Enabled);
        }
        COMPOSING.store(composing, Ordering::Relaxed);
        if !composing {
            *LAST_SYNC.lock().expect("ime sync") = None;
        }
        push_ime(QueuedIme::Preedit(s));
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_uk_nandi_sleek_SleekIme_onCommit<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    text: jni::objects::JString<'local>,
) {
    let _ = env.with_env(|_env| -> jni::errors::Result<()> {
        COMPOSING.store(false, Ordering::Relaxed);
        // Force a post-commit syncField after composing ends.
        *LAST_SYNC.lock().expect("ime sync") = None;
        push_ime(QueuedIme::Commit(format!("{text}")));
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_uk_nandi_sleek_SleekIme_onDeleteSurrounding<'local>(
    _env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    before_length: jni::sys::jint,
    after_length: jni::sys::jint,
) {
    let before = before_length.max(0) as usize;
    let after = after_length.max(0) as usize;
    if before > 0 || after > 0 {
        // Allow syncField to mirror egui after the delete is applied.
        *LAST_SYNC.lock().expect("ime sync") = None;
    }
    for _ in 0..before {
        push_ime(QueuedIme::Backspace);
    }
    // Forward-delete: emulate with Delete key via Backspace at next char —
    // egui has no Queued delete-forward; move isn't available here. For after>0
    // push Delete keys instead.
    for _ in 0..after {
        push_ime(QueuedIme::Delete);
    }
}
