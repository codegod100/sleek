//! Clipboard + file helpers for the compose-bar image attachment.

use std::path::Path;
use std::sync::Arc;
#[cfg(not(target_os = "android"))]
use std::sync::{Mutex, OnceLock};
#[cfg(not(target_os = "android"))]
use std::time::Duration;

use crate::state::ComposeImage;

/// Max pixel dimension after load (keeps memory / upload reasonable).
const MAX_DIM: u32 = 4096;
/// Refuse raw RGBA pastes larger than this (~25 MP).
const MAX_RGBA_BYTES: usize = 100_000_000;

/// How long the UI thread will wait on a clipboard/image helper thread.
/// Arboard/X11 conversion and file decode must never block egui's immediate
/// mode loop — a stuck compositor previously froze paste for the whole app.
#[cfg(not(target_os = "android"))]
const CLIPBOARD_UI_TIMEOUT: Duration = Duration::from_millis(400);

/// Shared arboard handle (creating one per paste starts an X11 server thread).
#[cfg(not(target_os = "android"))]
fn arboard_clipboard() -> Option<&'static Mutex<arboard::Clipboard>> {
    static CLIP: OnceLock<Option<Mutex<arboard::Clipboard>>> = OnceLock::new();
    CLIP.get_or_init(|| match arboard::Clipboard::new() {
        Ok(c) => Some(Mutex::new(c)),
        Err(e) => {
            log::warn!("arboard clipboard init failed: {e}");
            None
        }
    })
    .as_ref()
}

/// Try to read an image from the system clipboard.
///
/// Returns `None` when the clipboard has no image, the platform does not
/// support image clipboard, the read fails, or the read exceeds the UI
/// timeout (so text paste can still proceed).
pub fn try_get_image() -> Option<ComposeImage> {
    #[cfg(not(target_os = "android"))]
    {
        try_get_image_desktop()
    }
    #[cfg(target_os = "android")]
    {
        None
    }
}

#[cfg(not(target_os = "android"))]
fn try_get_image_desktop() -> Option<ComposeImage> {
    // 1) arboard (Wayland data-control when available, else X11 bridge).
    //    Always off the UI thread — `get_image` can block on X11 conversion.
    if let Some(img) = try_get_image_arboard_timed() {
        log::debug!(
            "clipboard image via arboard: {}x{}",
            img.width,
            img.height
        );
        return Some(img);
    }

    // 2) wl-paste: works on GNOME/Wayland even when arboard has no data-control.
    if let Some(img) = try_get_image_wl_paste() {
        log::debug!(
            "clipboard image via wl-paste: {}x{}",
            img.width,
            img.height
        );
        return Some(img);
    }

    // 3) File path(s) on the clipboard (file manager "copy").
    if let Some(img) = try_get_image_from_uri_list() {
        return Some(img);
    }

    None
}

/// Owned RGBA snapshot from arboard (safe to move across threads).
#[cfg(not(target_os = "android"))]
struct OwnedImage {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}

#[cfg(not(target_os = "android"))]
fn try_get_image_arboard_timed() -> Option<ComposeImage> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("sleek-clip-img".into())
        .spawn(move || {
            let result = (|| {
                let clip = arboard_clipboard()?;
                let mut guard = clip.lock().ok()?;
                let img = guard.get_image().ok()?;
                if img.width == 0 || img.height == 0 {
                    return None;
                }
                let expected = img.width.checked_mul(img.height)?.checked_mul(4)?;
                if img.bytes.len() < expected || expected > MAX_RGBA_BYTES {
                    return None;
                }
                Some(OwnedImage {
                    width: img.width,
                    height: img.height,
                    bytes: img.bytes.into_owned(),
                })
            })();
            let _ = tx.send(result);
        })
        .ok()?;

    match rx.recv_timeout(CLIPBOARD_UI_TIMEOUT) {
        Ok(Some(owned)) => compose_from_owned(owned),
        Ok(None) => None,
        Err(_) => {
            log::debug!(
                "arboard get_image timed out after {}ms — leaving text paste alone",
                CLIPBOARD_UI_TIMEOUT.as_millis()
            );
            None
        }
    }
}

#[cfg(not(target_os = "android"))]
fn compose_from_owned(img: OwnedImage) -> Option<ComposeImage> {
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let expected = img.width.checked_mul(img.height)?.checked_mul(4)?;
    if img.bytes.len() < expected {
        return None;
    }
    if expected > MAX_RGBA_BYTES {
        log::warn!("clipboard image too large ({expected} bytes rgba), ignoring");
        return None;
    }
    Some(ComposeImage::from_rgba(
        img.width,
        img.height,
        Arc::from(img.bytes),
    ))
}

/// Read encoded image bytes via `wl-paste` (optional system tool).
#[cfg(not(target_os = "android"))]
fn try_get_image_wl_paste() -> Option<ComposeImage> {
    const TYPES: &[&str] = &[
        "image/png",
        "image/jpeg",
        "image/jpg",
        "image/webp",
        "image/bmp",
        "image/gif",
    ];

    // Prefer an offered type when we can list them.
    let offered = wl_paste_list_types().unwrap_or_default();
    let mut try_types: Vec<&str> = TYPES
        .iter()
        .copied()
        .filter(|t| offered.is_empty() || offered.iter().any(|o| o.eq_ignore_ascii_case(t)))
        .collect();
    if try_types.is_empty() && !offered.is_empty() {
        // Clipboard has data but no image/* we know — don't spawn many failures.
        return None;
    }
    if try_types.is_empty() {
        try_types.extend_from_slice(TYPES);
    }

    for mime in try_types {
        if let Some(bytes) = wl_paste_bytes(mime) {
            match load_image_from_bytes(&bytes) {
                Ok(img) => return Some(img),
                Err(e) => log::debug!("wl-paste {mime} decode: {e}"),
            }
        }
    }
    None
}

#[cfg(not(target_os = "android"))]
fn wl_paste_list_types() -> Option<Vec<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new("wl-paste")
            .args(["--list-types"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let out = match rx.recv_timeout(CLIPBOARD_UI_TIMEOUT) {
        Ok(Ok(out)) => out,
        _ => return None,
    };
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    Some(
        s.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(not(target_os = "android"))]
fn wl_paste_bytes(mime: &str) -> Option<Vec<u8>> {
    // Run off the UI thread with a short timeout so a stuck compositor
    // cannot freeze the compose bar.
    let mime = mime.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new("wl-paste")
            .args(["--no-newline", "--type", &mime])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let out = match rx.recv_timeout(CLIPBOARD_UI_TIMEOUT) {
        Ok(Ok(out)) => out,
        _ => return None,
    };
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    // Guard against accidental huge pastes before decode.
    if out.stdout.len() > 40 * 1024 * 1024 {
        log::warn!("wl-paste image too large ({} bytes)", out.stdout.len());
        return None;
    }
    Some(out.stdout)
}

#[cfg(not(target_os = "android"))]
fn try_get_image_from_uri_list() -> Option<ComposeImage> {
    // Path list + decode off the UI thread (decode can take hundreds of ms).
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("sleek-clip-uri".into())
        .spawn(move || {
            let result = (|| {
                let clip = arboard_clipboard()?;
                let mut guard = clip.lock().ok()?;
                let paths = guard.get().file_list().ok()?;
                for path in paths {
                    if is_likely_image_path(&path) {
                        match load_image_from_path(&path) {
                            Ok(img) => {
                                log::debug!("clipboard image from file list: {}", path.display());
                                return Some(img);
                            }
                            Err(e) => log::debug!("clipboard file {}: {e}", path.display()),
                        }
                    }
                }
                None
            })();
            let _ = tx.send(result);
        })
        .ok()?;

    // Decode can take >400ms for large photos; allow longer than raw clipboard I/O.
    const URI_LIST_TIMEOUT: Duration = Duration::from_secs(2);
    match rx.recv_timeout(URI_LIST_TIMEOUT) {
        Ok(img) => img,
        Err(_) => {
            log::debug!(
                "clipboard file-list image timed out after {}ms",
                URI_LIST_TIMEOUT.as_millis()
            );
            None
        }
    }
}

fn is_likely_image_path(path: &Path) -> bool {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") => true,
        _ => false,
    }
}

/// Result of a native image file dialog (`Ok(None)` = user cancelled).
pub type PickImageResult = Result<Option<ComposeImage>, String>;

/// Open the **OS file picker** on a background thread and return a receiver.
///
/// Never call the dialog on the egui UI thread — a modal `block_on` there
/// freezes the whole app (often until the process is killed). Desktop uses
/// `rfd` (xdg-desktop-portal). Android uses the system document/photo Intent.
pub fn start_pick_image_file() -> std::sync::mpsc::Receiver<PickImageResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    #[cfg(not(target_os = "android"))]
    {
        std::thread::Builder::new()
            .name("sleek-file-pick".into())
            .spawn(move || {
                let result = pick_image_file_desktop();
                let _ = tx.send(result);
            })
            .expect("spawn file pick thread");
    }
    #[cfg(target_os = "android")]
    {
        std::thread::Builder::new()
            .name("sleek-file-pick".into())
            .spawn(move || {
                let result = crate::android_media::pick_image_file();
                let _ = tx.send(result);
            })
            .expect("spawn android file pick thread");
    }
    rx
}

/// Desktop: native OS dialog via rfd (portal / platform backend).
#[cfg(not(target_os = "android"))]
fn pick_image_file_desktop() -> PickImageResult {
    // rfd's xdg-portal backend (ashpd → zbus) needs a Tokio reactor. Run it on
    // this worker thread only — never on the egui UI thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .thread_name("sleek-rfd")
        .build()
        .map_err(|e| format!("Could not start file dialog runtime: {e}"))?;

    let path = rt.block_on(async {
        rfd::AsyncFileDialog::new()
            .set_title("Attach image")
            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            .pick_file()
            .await
            .map(|handle| handle.path().to_path_buf())
    });
    // Drop the runtime before heavy decode so worker threads go away promptly.
    drop(rt);

    let Some(path) = path else {
        return Ok(None);
    };
    Ok(Some(load_image_from_path(&path)?))
}

/// Load and decode an image file into RGBA for the compose preview / upload.
pub fn load_image_from_path(path: &Path) -> Result<ComposeImage, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read file: {e}"))?;
    load_image_from_bytes(&bytes)
}

/// Decode image bytes (png/jpeg/gif/webp/bmp) into a compose attachment.
pub fn load_image_from_bytes(bytes: &[u8]) -> Result<ComposeImage, String> {
    if bytes.is_empty() {
        return Err("Empty file".into());
    }
    // Reject multi-hundred-MB blobs before decoding.
    if bytes.len() > 40 * 1024 * 1024 {
        return Err("Image file is too large (max 40MB)".into());
    }

    let dyn_img = image::load_from_memory(bytes).map_err(|e| format!("Not a valid image: {e}"))?;
    let dyn_img = if dyn_img.width() > MAX_DIM || dyn_img.height() > MAX_DIM {
        dyn_img.thumbnail(MAX_DIM, MAX_DIM)
    } else {
        dyn_img
    };
    let rgba = dyn_img.to_rgba8();
    let width = rgba.width() as usize;
    let height = rgba.height() as usize;
    let data = rgba.into_raw();
    let expected = width.saturating_mul(height).saturating_mul(4);
    if data.len() < expected {
        return Err("Decoded image data incomplete".into());
    }
    if expected > MAX_RGBA_BYTES {
        return Err("Image is too large after decode".into());
    }
    Ok(ComposeImage::from_rgba(width, height, Arc::from(data)))
}

/// Encode a compose image as PNG bytes for upload.
pub fn encode_png(image: &ComposeImage) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;

    let w = image.width as u32;
    let h = image.height as u32;
    let mut buf = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(&mut buf);
    encoder
        .write_image(
            image.rgba.as_ref(),
            w,
            h,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("PNG encode failed: {e}"))?;
    Ok(buf)
}

/// Encode on a worker thread so large pastes cannot freeze the egui frame.
pub fn start_encode_png(
    image: ComposeImage,
) -> std::sync::mpsc::Receiver<Result<Vec<u8>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("sleek-png-encode".into())
        .spawn(move || {
            let _ = tx.send(encode_png(&image));
        })
        .expect("spawn png encode thread");
    rx
}
