//! System chrome (status bar / nav bar) safe areas.
//!
//! Android NativeActivity draws **edge-to-edge**: the window fills under the
//! system status bar (clock, battery, …) and the gesture/nav bar. Without an
//! explicit reserve, app chrome and labels sit under those system widgets.
//!
//! Call [`reserve_system_chrome`] once at the start of every frame (before any
//! other `TopBottomPanel` / `SidePanel` / `CentralPanel`), or use
//! [`top_header`] which does that for you.
//!
//! Prefer injecting measured insets via [`set_system_chrome`] (from
//! `AndroidApp::content_rect` or WindowInsets) so reserves match the device —
//! hardcoded fallbacks are intentionally tight for modern gesture-nav phones.

use egui::{Align2, Context, Frame, Id, Margin, Sense, Ui, WidgetText, Window};

use crate::Theme;

/// Temp-data id for app-supplied measured insets (see [`set_system_chrome`]).
const SYSTEM_CHROME_ID: &str = "vidya.system_chrome";

/// Insets for system status / navigation chrome on edge-to-edge surfaces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemChrome {
    /// Space under the status bar (clock, indicators).
    pub top: f32,
    /// Space above the system gesture / 3-button nav bar.
    pub bottom: f32,
}

impl SystemChrome {
    pub const ZERO: Self = Self {
        top: 0.0,
        bottom: 0.0,
    };

    pub fn is_zero(self) -> bool {
        self.top <= 0.0 && self.bottom <= 0.0
    }
}

/// Inject measured system insets for this frame (egui points).
///
/// Call once per frame **before** [`reserve_system_chrome`]. When set,
/// [`system_chrome`] uses these values instead of the platform fallbacks
/// (IME expansion still applies on top of `bottom`).
pub fn set_system_chrome(ctx: &Context, chrome: SystemChrome) {
    ctx.data_mut(|d| d.insert_temp(Id::new(SYSTEM_CHROME_ID), chrome));
}

/// Platform defaults for edge-to-edge drawing.
///
/// Fallback values are **tight** for modern gesture-nav phones (≈24–28 dp
/// status, ≈16–24 dp gesture handle). Prefer [`set_system_chrome`] with
/// measured `content_rect` / WindowInsets when available.
///
/// When a text field holds focus (`Context::wants_keyboard_input`), the bottom
/// inset grows so bottom bars / compose fields sit **above** the soft keyboard
/// (NativeActivity rarely resizes the GL surface for IME).
pub fn system_chrome(ctx: &Context) -> SystemChrome {
    #[cfg(target_os = "android")]
    {
        let measured = ctx.data(|d| d.get_temp::<SystemChrome>(Id::new(SYSTEM_CHROME_ID)));
        // Fallbacks when the app does not inject measured insets. Top must
        // clear the status bar under edge-to-edge (never “shy” into the clock).
        const TOP_FALLBACK: f32 = 36.0;
        const BOTTOM_FALLBACK: f32 = 20.0;
        let top = match measured {
            // Reject zero/near-zero injected tops — that was the regression:
            // content_rect top=0 overrode the fallback and drew under the clock.
            Some(c) if c.top >= 8.0 => c.top,
            Some(c) => c.top.max(TOP_FALLBACK),
            None => TOP_FALLBACK,
        };
        let mut bottom = match measured {
            Some(c) => c.bottom.max(0.0),
            None => BOTTOM_FALLBACK,
        };
        if ctx.wants_keyboard_input() {
            // Soft keyboards are typically ~35–45% of portrait height. Pad that
            // much under all UI so TopBottomPanel compose / tabs clear the IME.
            // (Seen on Pixel: ime inset ≈ 988px on a 2400px-tall display.)
            let h = ctx.screen_rect().height();
            let ime = (h * 0.40).clamp(240.0, h * 0.52);
            bottom = bottom.max(ime);
            // Keep animating a few frames while the keyboard slides in/out.
            ctx.request_repaint();
        }
        SystemChrome { top, bottom }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = ctx;
        SystemChrome::ZERO
    }
}

/// Reserve top/bottom strips so **no** subsequent panel or central content can
/// paint under the system status or navigation bars.
///
/// Call this **once per frame**, before other panels. Safe to call when insets
/// are zero (no-op on desktop).
///
/// The reserved bands use the theme headerbar fill so they blend with a top
/// header sitting just below the status bar.
pub fn reserve_system_chrome(ctx: &Context, theme: &Theme) {
    let chrome = system_chrome(ctx);
    if chrome.is_zero() {
        return;
    }

    // Top band matches the header so status-bar area reads as continuous chrome
    // under the clock (not a hole the title can climb into).
    // Bottom band matches window bg so gesture/nav clearance stays subtle.
    let top_band = Frame::new()
        .fill(theme.palette.headerbar_bg)
        .inner_margin(Margin::ZERO);
    let bottom_band = Frame::new()
        .fill(theme.palette.window_bg)
        .inner_margin(Margin::ZERO);

    if chrome.top > 0.0 {
        egui::TopBottomPanel::top("vidya_system_chrome_top")
            .exact_height(chrome.top)
            .frame(top_band)
            .show_separator_line(false)
            .show(ctx, |_ui| {});
    }

    if chrome.bottom > 0.0 {
        egui::TopBottomPanel::bottom("vidya_system_chrome_bottom")
            .exact_height(chrome.bottom)
            .frame(bottom_band)
            .show_separator_line(false)
            .show(ctx, |ui| {
                // Absorb input so gestures aren't stolen by widgets behind the band.
                ui.allocate_exact_size(ui.available_size(), Sense::hover());
            });
    }
}

/// Top app header with system status bar already reserved.
///
/// Preferred entry for shell chrome: apps cannot place title/status text under
/// the clock / indicators. Also reserves the bottom system nav band.
///
/// ```ignore
/// vidya::top_header(ctx, &theme, |ui| {
///     ui.horizontal(|ui| {
///         vidya::title(ui, &theme, "My App");
///     });
/// });
/// ```
pub fn top_header(ctx: &Context, theme: &Theme, add_contents: impl FnOnce(&mut Ui)) {
    reserve_system_chrome(ctx, theme);
    egui::TopBottomPanel::top("vidya_app_header")
        .frame(theme.header_frame())
        .show_separator_line(false)
        .show(ctx, add_contents);
}

/// Centered modal-style window with themed card chrome.
///
/// Defaults: non-collapsible, **resizable**, centered, [`Theme::card_frame`].
/// Chain `.default_size` / `.min_width` / `.resizable(false)` as needed, then
/// `.show`.
///
/// ```ignore
/// vidya::dialog("Rename", &theme)
///     .default_width(360.0)
///     .min_width(280.0)
///     .show(ctx, |ui| { /* … */ });
/// ```
pub fn dialog<'a>(title: impl Into<WidgetText> + 'a, theme: &Theme) -> Window<'a> {
    Window::new(title)
        .collapsible(false)
        .resizable(true)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(theme.card_frame())
}
