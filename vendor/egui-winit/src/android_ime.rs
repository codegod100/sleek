//! Android IME allow/deny hook — NativeActivity has no InputConnection for swipe typing.

use std::sync::OnceLock;

type SetAllowedFn = fn(bool) -> bool;

static HOOK: OnceLock<SetAllowedFn> = OnceLock::new();

/// Register a handler for egui IME focus (call from `android_main` before eframe).
pub fn set_android_ime_hook(set_allowed: fn(bool) -> bool) {
    let _ = HOOK.set(set_allowed);
}

/// Returns true when a hook handled the request (skip winit `set_ime_allowed`).
pub fn set_allowed(allowed: bool) -> bool {
    HOOK.get().is_some_and(|f| f(allowed))
}

#[cfg(test)]
pub fn is_registered() -> bool {
    HOOK.get().is_some()
}
