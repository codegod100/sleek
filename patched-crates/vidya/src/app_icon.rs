//! Embed a window / taskbar app icon from PNG bytes.
//!
//! Apps typically `include_bytes!` a 128–256px PNG and pass it through
//! [`with_app_icon`] / [`with_app_icon_id`] when building the eframe viewport:
//!
//! ```ignore
//! use egui::ViewportBuilder;
//! use vidya::with_app_icon_id;
//!
//! let viewport = with_app_icon_id(
//!     ViewportBuilder::default().with_title("My App"),
//!     "my-app", // Wayland app_id — match .desktop StartupWMClass / file id
//!     include_bytes!("../assets/icon-256.png"),
//! );
//! ```
//!
//! On Wayland, [`with_app_icon_id`] is preferred: the compositor matches
//! `app_id` to a FreeDesktop entry’s `Icon=` when the `.desktop` is on
//! `XDG_DATA_DIRS`. The embedded icon still sets the client-side window icon
//! where the platform supports it (X11, Windows, …).

use std::io::Cursor;
use std::sync::Arc;

use egui::{IconData, ViewportBuilder};

/// Error decoding an app icon PNG.
#[derive(Debug, Clone)]
pub struct AppIconError(String);

impl AppIconError {
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AppIconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "app icon: {}", self.0)
    }
}

impl std::error::Error for AppIconError {}

/// Decode PNG bytes into egui [`IconData`] (RGBA8, unpremultiplied).
///
/// Intended for `include_bytes!("…png")` assets used as the native window icon.
pub fn icon_data_from_png(png_bytes: &[u8]) -> Result<IconData, AppIconError> {
    let (width, height, rgba) = decode_png_rgba(png_bytes)?;
    Ok(IconData {
        rgba,
        width,
        height,
    })
}

/// Attach an embedded PNG as the viewport window icon.
///
/// On decode failure the viewport is returned unchanged so a bad asset never
/// blocks app launch.
pub fn with_app_icon(viewport: ViewportBuilder, png_bytes: &[u8]) -> ViewportBuilder {
    match icon_data_from_png(png_bytes) {
        Ok(icon) => viewport.with_icon(Arc::new(icon)),
        Err(_) => viewport,
    }
}

/// Like [`with_app_icon`], and set Wayland `app_id`.
///
/// `app_id` should match the FreeDesktop application id / `StartupWMClass`
/// (e.g. `"usage"` for `usage.desktop`).
pub fn with_app_icon_id(
    viewport: ViewportBuilder,
    app_id: impl Into<String>,
    png_bytes: &[u8],
) -> ViewportBuilder {
    with_app_icon(viewport.with_app_id(app_id), png_bytes)
}

/// Fallible variant of [`with_app_icon`].
pub fn try_with_app_icon(
    viewport: ViewportBuilder,
    png_bytes: &[u8],
) -> Result<ViewportBuilder, AppIconError> {
    let icon = icon_data_from_png(png_bytes)?;
    Ok(viewport.with_icon(Arc::new(icon)))
}

/// Fallible variant of [`with_app_icon_id`].
pub fn try_with_app_icon_id(
    viewport: ViewportBuilder,
    app_id: impl Into<String>,
    png_bytes: &[u8],
) -> Result<ViewportBuilder, AppIconError> {
    try_with_app_icon(viewport.with_app_id(app_id), png_bytes)
}

fn decode_png_rgba(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), AppIconError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder
        .read_info()
        .map_err(|e| AppIconError(format!("png header: {e}")))?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| AppIconError(format!("png frame: {e}")))?;
    let w = info.width;
    let h = info.height;
    let raw = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in raw.chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        other => {
            return Err(AppIconError(format!(
                "unsupported png color type: {other:?}"
            )));
        }
    };
    if rgba.len() != (w * h * 4) as usize {
        return Err(AppIconError(format!(
            "rgba length {} != {}×{}×4",
            rgba.len(),
            w,
            h
        )));
    }
    Ok((w, h, rgba))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1×1 red RGB PNG (valid CRC).
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xc9, 0xfe, 0x92, 0xef, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn icon_data_from_tiny_png() {
        let icon = icon_data_from_png(TINY_PNG).expect("decode");
        assert_eq!(icon.width, 1);
        assert_eq!(icon.height, 1);
        assert_eq!(icon.rgba.len(), 4);
    }

    #[test]
    fn with_app_icon_id_sets_app_id() {
        let vp = with_app_icon_id(ViewportBuilder::default(), "usage", TINY_PNG);
        assert_eq!(vp.app_id.as_deref(), Some("usage"));
        assert!(vp.icon.is_some());
    }

    #[test]
    fn bad_png_leaves_viewport_unchanged() {
        let vp = with_app_icon(ViewportBuilder::default(), b"not a png");
        assert!(vp.icon.is_none());
        assert!(icon_data_from_png(b"not a png").is_err());
    }
}
