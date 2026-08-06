use raw_window_handle::RawDisplayHandle;

/// Handles interfacing with the OS clipboard.
///
/// If the "clipboard" feature is off, or we cannot connect to the OS clipboard,
/// then a fallback clipboard that just works within the same app is used instead.
///
/// On Android, register hooks via [`set_android_clipboard_hooks`] so long-press
/// paste reads the system clipboard (stock egui-winit only keeps in-app text).
#[cfg(target_os = "android")]
mod android_clipboard {
    use std::sync::OnceLock;

    type GetFn = fn() -> Option<String>;
    type SetFn = fn(&str) -> bool;

    static HOOKS: OnceLock<(GetFn, SetFn)> = OnceLock::new();

    pub fn set(get: GetFn, set: SetFn) {
        let _ = HOOKS.set((get, set));
    }

    pub fn get() -> Option<String> {
        HOOKS.get().and_then(|(g, _)| g())
    }

    pub fn set_text(text: &str) -> bool {
        HOOKS.get().is_some_and(|(_, s)| s(text))
    }
}

/// Wire Android system clipboard read/write (call from `android_main` before eframe).
#[cfg(target_os = "android")]
pub fn set_android_clipboard_hooks(get: fn() -> Option<String>, set: fn(&str) -> bool) {
    android_clipboard::set(get, set);
}

pub struct Clipboard {
    #[cfg(all(feature = "arboard", not(target_os = "android")))]
    arboard: Option<arboard::Clipboard>,

    #[cfg(all(
        any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ),
        feature = "smithay-clipboard"
    ))]
    smithay: Option<smithay_clipboard::Clipboard>,

    /// Fallback manual clipboard.
    clipboard: String,
}

impl Clipboard {
    /// Construct a new instance
    pub fn new(_raw_display_handle: Option<RawDisplayHandle>) -> Self {
        Self {
            #[cfg(all(feature = "arboard", not(target_os = "android")))]
            arboard: init_arboard(),

            #[cfg(all(
                any(
                    target_os = "linux",
                    target_os = "dragonfly",
                    target_os = "freebsd",
                    target_os = "netbsd",
                    target_os = "openbsd"
                ),
                feature = "smithay-clipboard"
            ))]
            smithay: init_smithay_clipboard(_raw_display_handle),

            clipboard: Default::default(),
        }
    }

    pub fn get(&mut self) -> Option<String> {
        // Prefer smithay on Wayland (regular data-device; works without wlr-data-control).
        // On failure (e.g. image-only offer), fall through to arboard (X11 bridge on GNOME).
        #[cfg(all(
            any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            ),
            feature = "smithay-clipboard"
        ))]
        if let Some(clipboard) = &mut self.smithay {
            match clipboard.load() {
                Ok(text) => return Some(text),
                Err(err) => {
                    // Common when clipboard holds only an image (no text/* mime).
                    // Downstream may still recover via arboard or app-level image paste.
                    log::debug!("smithay paste: {err}");
                }
            }
        }

        #[cfg(all(feature = "arboard", not(target_os = "android")))]
        if let Some(clipboard) = &mut self.arboard {
            return match clipboard.get_text() {
                Ok(text) => Some(text),
                Err(err) => {
                    log::debug!("arboard paste: {err}");
                    None
                }
            };
        }

        #[cfg(target_os = "android")]
        if let Some(text) = android_clipboard::get() {
            if !text.is_empty() {
                return Some(text);
            }
        }

        Some(self.clipboard.clone())
    }

    pub fn set_text(&mut self, text: String) {
        #[cfg(all(
            any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            ),
            feature = "smithay-clipboard"
        ))]
        if let Some(clipboard) = &mut self.smithay {
            clipboard.store(text);
            return;
        }

        #[cfg(all(feature = "arboard", not(target_os = "android")))]
        if let Some(clipboard) = &mut self.arboard {
            if let Err(err) = clipboard.set_text(text.clone()) {
                log::error!("arboard copy/cut error: {err}");
            }
            return;
        }

        #[cfg(target_os = "android")]
        if android_clipboard::set_text(&text) {
            self.clipboard = text;
            return;
        }

        self.clipboard = text;
    }

    pub fn set_image(&mut self, image: &egui::ColorImage) {
        #[cfg(all(feature = "arboard", not(target_os = "android")))]
        if let Some(clipboard) = &mut self.arboard {
            if let Err(err) = clipboard.set_image(arboard::ImageData {
                width: image.width(),
                height: image.height(),
                bytes: std::borrow::Cow::Borrowed(bytemuck::cast_slice(&image.pixels)),
            }) {
                log::error!("arboard copy/cut error: {err}");
            }
            log::debug!("Copied image to clipboard");
            return;
        }

        log::error!("Copying images is not supported. Enable the 'clipboard' feature of `egui-winit` to enable it.");
        _ = image;
    }
}

#[cfg(all(feature = "arboard", not(target_os = "android")))]
fn init_arboard() -> Option<arboard::Clipboard> {
    profiling::function_scope!();

    log::trace!("Initializing arboard clipboard…");
    match arboard::Clipboard::new() {
        Ok(clipboard) => Some(clipboard),
        Err(err) => {
            log::warn!("Failed to initialize arboard clipboard: {err}");
            None
        }
    }
}

#[cfg(all(
    any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ),
    feature = "smithay-clipboard"
))]
fn init_smithay_clipboard(
    raw_display_handle: Option<RawDisplayHandle>,
) -> Option<smithay_clipboard::Clipboard> {
    #![allow(clippy::undocumented_unsafe_blocks)]

    profiling::function_scope!();

    if let Some(RawDisplayHandle::Wayland(display)) = raw_display_handle {
        log::trace!("Initializing smithay clipboard…");
        #[allow(unsafe_code)]
        Some(unsafe { smithay_clipboard::Clipboard::new(display.display.as_ptr()) })
    } else {
        #[cfg(feature = "wayland")]
        log::debug!("Cannot init smithay clipboard without a Wayland display handle");
        #[cfg(not(feature = "wayland"))]
        log::debug!(
            "Cannot init smithay clipboard: the 'wayland' feature of 'egui-winit' is not enabled"
        );
        None
    }
}
