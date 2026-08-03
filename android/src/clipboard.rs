//! Clipboard + file helpers for the compose-bar media attachment.

use std::path::Path;
use std::sync::Arc;
#[cfg(not(target_os = "android"))]
use std::sync::{Mutex, OnceLock};
#[cfg(not(target_os = "android"))]
use std::time::Duration;

use crate::state::{ComposeAttach, ComposeImage, ComposeVideo};

/// Max pixel dimension after load (keeps memory / upload reasonable).
const MAX_DIM: u32 = 4096;
/// Refuse raw RGBA pastes larger than this (~25 MP).
const MAX_RGBA_BYTES: usize = 100_000_000;
/// Server + freeq-app upload cap (also used as a pick-time reject for video).
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

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

/// Result of a native media file dialog (`Ok(None)` = user cancelled).
pub type PickAttachResult = Result<Option<ComposeAttach>, String>;

/// Open the **OS file picker** on a background thread and return a receiver.
///
/// Never call the dialog on the egui UI thread — a modal `block_on` there
/// freezes the whole app (often until the process is killed). Desktop uses
/// `rfd` (xdg-desktop-portal). Android uses the system document Intent.
pub fn start_pick_media_file() -> std::sync::mpsc::Receiver<PickAttachResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    #[cfg(not(target_os = "android"))]
    {
        std::thread::Builder::new()
            .name("sleek-file-pick".into())
            .spawn(move || {
                let result = pick_media_file_desktop();
                let _ = tx.send(result);
            })
            .expect("spawn file pick thread");
    }
    #[cfg(target_os = "android")]
    {
        std::thread::Builder::new()
            .name("sleek-file-pick".into())
            .spawn(move || {
                let result = crate::android_media::pick_media_file();
                let _ = tx.send(result);
            })
            .expect("spawn android file pick thread");
    }
    rx
}

/// Process-lifetime Tokio runtime for rfd's xdg-desktop-portal backend.
///
/// ashpd caches a single `zbus::Connection` in a static `OnceLock`. That
/// connection is driven by the runtime that created it — if we build+drop a
/// runtime per pick (as we used to), cancel works once, then the next open
/// reuses the dead connection and hangs forever.
#[cfg(not(target_os = "android"))]
fn rfd_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("sleek-rfd")
            .build()
            .expect("sleek-rfd tokio runtime")
    })
}

/// Desktop: native OS dialog via rfd (portal / platform backend).
#[cfg(not(target_os = "android"))]
fn pick_media_file_desktop() -> PickAttachResult {
    // Serialize portal requests: a UI timeout can clear `file_pick_rx` while
    // the first worker is still inside `pick_file`, and two concurrent
    // OpenFileRequests on ashpd's shared connection also hang.
    static PICK_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = PICK_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "File picker lock poisoned".to_string())?;

    // rfd's xdg-portal backend (ashpd → zbus) needs a Tokio reactor. Drive it
    // from this worker thread only — never on the egui UI thread.
    let path = rfd_runtime().block_on(async {
        rfd::AsyncFileDialog::new()
            .set_title("Attach image or video")
            .add_filter(
                "Media",
                &["png", "jpg", "jpeg", "gif", "webp", "bmp", "mp4", "m4v", "webm", "mov"],
            )
            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
            .add_filter("Videos", &["mp4", "m4v", "webm", "mov"])
            .pick_file()
            .await
            .map(|handle| handle.path().to_path_buf())
    });

    let Some(path) = path else {
        return Ok(None);
    };
    Ok(Some(load_attach_from_path(&path)?))
}

/// Load an image or video file into a compose attachment.
pub fn load_attach_from_path(path: &Path) -> Result<ComposeAttach, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not read file: {e}"))?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attach")
        .to_string();
    load_attach_from_bytes(&bytes, Some(&name))
}

/// Load and decode an image file into RGBA for the compose preview / upload.
pub fn load_image_from_path(path: &Path) -> Result<ComposeImage, String> {
    match load_attach_from_path(path)? {
        ComposeAttach::Image(img) => Ok(img),
        ComposeAttach::Video(_) => Err("Not an image file".into()),
    }
}

/// Build a compose attachment from raw bytes + optional filename hint.
pub fn load_attach_from_bytes(bytes: &[u8], filename: Option<&str>) -> Result<ComposeAttach, String> {
    if bytes.is_empty() {
        return Err("Empty file".into());
    }

    let name = filename.unwrap_or("attach");
    let lower = name.to_ascii_lowercase();
    if let Some(ct) = video_content_type_for_name(&lower).or_else(|| sniff_video_content_type(bytes))
    {
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err("Video is too large (max 10MB)".into());
        }
        let filename = if lower.ends_with(".mp4")
            || lower.ends_with(".m4v")
            || lower.ends_with(".webm")
            || lower.ends_with(".mov")
        {
            name.to_string()
        } else {
            default_video_filename(ct)
        };
        return Ok(ComposeAttach::Video(ComposeVideo::new(
            Arc::from(bytes.to_vec()),
            ct,
            filename,
        )));
    }

    Ok(ComposeAttach::Image(load_image_from_bytes(bytes)?))
}

fn video_content_type_for_name(name: &str) -> Option<&'static str> {
    let base = name.split('?').next().unwrap_or(name);
    let base = base.split('#').next().unwrap_or(base);
    if base.ends_with(".mp4") || base.ends_with(".m4v") {
        Some("video/mp4")
    } else if base.ends_with(".webm") {
        Some("video/webm")
    } else if base.ends_with(".mov") {
        Some("video/quicktime")
    } else {
        None
    }
}

fn sniff_video_content_type(bytes: &[u8]) -> Option<&'static str> {
    // ISO BMFF (`….ftyp`) — mp4 / m4v / mov.
    if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        return Some("video/mp4");
    }
    // EBML / Matroska / WebM
    if bytes.len() >= 4 && bytes[0..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some("video/webm");
    }
    None
}

fn default_video_filename(content_type: &str) -> String {
    match content_type {
        "video/webm" => "clip.webm".into(),
        "video/quicktime" => "clip.mov".into(),
        _ => "clip.mp4".into(),
    }
}

/// Decode image bytes (png/jpeg/gif/webp/bmp) into a compose attachment image.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_mp4_as_video_attach() {
        // Minimal ISO BMFF header (ftyp) — enough for sniff + attach.
        let mut bytes = vec![0u8; 32];
        bytes[4..8].copy_from_slice(b"ftyp");
        bytes[8..12].copy_from_slice(b"isom");
        let attach = load_attach_from_bytes(&bytes, Some("clip.mp4")).unwrap();
        match attach {
            ComposeAttach::Video(v) => {
                assert_eq!(v.content_type, "video/mp4");
                assert_eq!(v.filename, "clip.mp4");
            }
            other => panic!("expected video, got {other:?}"),
        }
    }

    #[test]
    fn rejects_huge_video() {
        let mut bytes = vec![0u8; MAX_UPLOAD_BYTES + 1];
        bytes[4..8].copy_from_slice(b"ftyp");
        let err = load_attach_from_bytes(&bytes, Some("big.mp4")).unwrap_err();
        assert!(err.contains("10MB"), "{err}");
    }
}
