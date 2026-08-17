//! **Vidya** — theme layer on top of [egui].
//!
//! Palette and spacing track GNOME HIG / libadwaita *feel* without linking GTK
//! or using that name in the public API. Dark is the default.
//!
//! On Android (edge-to-edge), call [`reserve_system_chrome`] once per frame
//! **before** other panels, or use [`top_header`] so content cannot sit under
//! the system status / navigation bars.
//!
//! Layout composition: prefer [`page_body`] / [`card`] / [`compact_card`] /
//! [`pack`] / [`vstack`] / [`lead_trail`] / [`two_col`] / [`grid_cols`] /
//! [`metric_bps`] so apps avoid width overflow, justified-gap stretch, and
//! metric “waterfall” columns.
//!
//! **Top-level application UI is grid-only:** [`central_page`] / [`page_body`]
//! take a [`GridCtx`] callback (not a free-stack `Ui`). Compose with
//! [`GridCtx::section`] / `g.row`; nest cards and nested grids inside cells.
//!
//! ```ignore
//! vidya::central_page(ctx, &th, "main", |g| {
//!     g.section(|ui| { /* … */ });
//!     g.section(|ui| { /* … */ });
//! });
//! ```
//!
//! Nested / table Grid DSL:
//! ```ignore
//! vidya::grid_cols(ui, &th, "procs", &[ColSpec::Flex, ColSpec::MetricBps], |g| {
//!     g.row(|r| { r.heading("Name"); r.heading("Write"); });
//!     g.row(|r| { r.text(name); r.metric_bps(rate); });
//! });
//! ```
//!
//! Window icon (embed a PNG with `include_bytes!`):
//! ```ignore
//! use egui::ViewportBuilder;
//! use vidya::with_app_icon_id;
//!
//! let viewport = with_app_icon_id(
//!     ViewportBuilder::default().with_title("usage"),
//!     "usage", // Wayland app_id ↔ .desktop StartupWMClass
//!     include_bytes!("../assets/usage-256.png"),
//! );
//! ```
//!
//! ```ignore
//! use vidya::{apply_dark, Theme};
//!
//! apply_dark(ctx);
//! if vidya::primary_button(ui, &Theme::dark(), "Open").clicked() { /* … */ }
//! ```

mod app_icon;
mod chrome;
mod fonts;
mod hotkeys;
mod icons;
mod layout;
mod theme;

pub use app_icon::{
    icon_data_from_png, try_with_app_icon, try_with_app_icon_id, with_app_icon, with_app_icon_id,
    AppIconError,
};
pub use chrome::{
    dialog, reserve_system_chrome, set_system_chrome, system_chrome, top_header, SystemChrome,
};
pub use fonts::install_symbol_font;
pub use hotkeys::{
    command_glyph, command_pressed, command_shift_shortcut_label, command_shortcut_label,
    consume_command, consume_command_shift, consume_escape, escape_label, is_macos,
};
pub use icons::{
    emoji_icon, emoji_icon_colored, emoji_pack_len, has_emoji_icon, icon, icon_colored,
    icon_for_emoji, normalize_emoji, paint_emoji_in, paint_icon, paint_icon_in, reaction_chip,
    twemoji_key, Icon,
};
pub use layout::{
    card, card_frame_chrome_x, central_page, central_page_cols, compact_card, data_table,
    default_min_col, distribute_col_max, fill_width, fit_width, format_bps, format_rate, grid,
    grid_cols, grid_cols_with, hflow, inset_row, lead_trail, measure_mono_caption, metric_bps,
    metric_cell, metric_cell_px, metric_rate, pack, pad_metric, page_body, page_body_cols,
    page_scroll, side_by_side, table_metric, table_text, table_text_capped, two_col, vstack, Col,
    ColKind, ColSpec, GridCtx, GridOpts, RowDsl, FLEX_COL_MIN_PX, METRIC_BPS_CHARS,
    METRIC_RATE_CHARS,
};
pub use theme::{
    body, button, checkbox, destructive_button, dim_label, icon_button, primary_button, status_dot,
    text_field_multiline, text_field_singleline, title, title_2, Mode, Palette, Spacing, Theme,
    TypeScale,
};

use egui::{Context, FontFamily, FontId, Style, TextStyle};

/// Install palette + spacing + text styles on the egui context.
///
/// Also registers a small symbol-font fallback so punctuation like `→` / `●` /
/// `▾` / `▴` does not render as hollow boxes on Android.
pub fn apply(ctx: &Context, theme: &Theme) {
    install_symbol_font(ctx);
    ctx.set_visuals(theme.visuals());
    ctx.set_style(theme.style());
}

/// Dark shell (default).
pub fn apply_dark(ctx: &Context) {
    apply(ctx, &Theme::dark());
}

/// Light shell.
pub fn apply_light(ctx: &Context) {
    apply(ctx, &Theme::light());
}

/// Map type scale onto egui text styles.
pub fn install_text_styles(style: &mut Style, scale: &TypeScale) {
    use FontFamily::Proportional;
    style.text_styles = [
        (TextStyle::Small, FontId::new(scale.caption, Proportional)),
        (TextStyle::Body, FontId::new(scale.body, Proportional)),
        (TextStyle::Button, FontId::new(scale.body, Proportional)),
        (TextStyle::Heading, FontId::new(scale.title, Proportional)),
        (
            TextStyle::Monospace,
            FontId::new(scale.body, FontFamily::Monospace),
        ),
    ]
    .into();
}
