//! Layout composition surface — width-safe, compact-by-default primitives.
//!
//! Apps should prefer these helpers over raw `set_max_width` / `Layout::…`
//! plumbing. Escape hatches stay (egui is still available); this module makes
//! the **good defaults** the short path.
//!
//! # Patterns → primitives
//!
//! | App footgun | Primitive |
//! |-------------|-----------|
//! | Page scrolls off / under edge | [`page_body`] / [`central_page`] |
//! | Free-stack app UI at top level | **enforced:** [`page_body`] / [`central_page`] take a grid DSL |
//! | Card overflows column | [`card`] / [`compact_card`] |
//! | Giant gaps when parent is tall | [`vstack`] (non-justified) |
//! | Fixed tiles stretch across the window | [`pack`] (wrap, hug content) |
//! | Actions clipped off the right | [`lead_trail`] (content-hugging height) |
//! | Side-by-side vs stack breakpoint | [`two_col`] / [`side_by_side`] |
//! | Rate columns staircase ("waterfall") | [`metric_bps`] / [`metric_rate`] / [`metric_cell`] / [`grid_cols`] / [`data_table`] |
//!
//! # Top-level application UI (enforced)
//!
//! Application content in the central panel **must** be composed through the
//! grid DSL. [`page_body`] and [`central_page`] only accept a [`GridCtx`]
//! callback — free stacking of widgets at the page root is not part of the
//! public path. Nested cards, nested [`grid_cols`], gauges, and text go
//! **inside** cells / [`GridCtx::section`]s.
//!
//! ```ignore
//! vidya::central_page(ctx, &th, "main", |g| {
//!     g.section(|ui| { /* gauges row — often a nested grid_cols */ });
//!     g.section(|ui| { /* process table */ });
//! });
//! ```
//!
//! Escape hatch (scroll + width pin only, no grid): [`page_scroll`].

use std::hash::Hash;

use egui::{
    Align, FontId, Frame, Grid, InnerResponse, Layout, Margin, RichText, ScrollArea, Sense, Stroke,
    Ui, Vec2,
};

use crate::Theme;

// ── Pure policy (unit-tested without a window) ──────────────────────────────

/// Minimum residual width (px) before two equal columns fit with `gap`.
///
/// `true` means place side-by-side; `false` means stack vertically.
pub fn side_by_side(avail: f32, min_col: f32, gap: f32) -> bool {
    avail >= min_col * 2.0 + gap && min_col > 0.0 && avail > 0.0
}

/// Default min column width for [`two_col`] when apps pass theme spacing.
pub fn default_min_col(theme: &Theme) -> f32 {
    // Roughly one compact card: control + padding.
    theme.spacing.control_height * 6.0 + theme.spacing.page
}

/// Character width of a fixed monospace throughput cell ([`metric_bps`]).
pub const METRIC_BPS_CHARS: usize = 14;

/// Character width of a fixed monospace event-rate cell ([`metric_rate`]).
pub const METRIC_RATE_CHARS: usize = 10;

/// Minimum pixel width for a metric cell of `chars` monospace glyphs + padding.
///
/// Used as a **floor** for [`ColSpec::MetricBps`] / [`MetricRate`]. Actual cells
/// still grow via [`metric_cell`] if the painted string is wider.
pub fn metric_cell_px(theme: &Theme, chars: usize) -> f32 {
    // Monospace advance is typically ~0.6–0.65em; use 0.72 so columns never
    // undershoot common caption sizes (was 0.62 and clipped padded metrics).
    let advance = theme.type_scale.caption * 0.72;
    let pad = theme.spacing.md + theme.spacing.sm;
    advance * chars as f32 + pad
}

/// Left-pad `s` to exactly `width` characters (Unicode scalar count).
///
/// Longer strings are returned unchanged (width is a minimum for alignment).
pub fn pad_metric(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        s.to_string()
    } else {
        format!("{:>width$}", s, width = width)
    }
}

/// Human-readable throughput (B/s → KiB/s, …) without padding.
pub fn format_bps(bps: f64) -> String {
    const UNITS: [&str; 5] = ["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s"];
    let mut v = bps.max(0.0);
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// Fixed-width throughput for tables (monospace-safe; both edges flush).
pub fn metric_bps(bps: f64) -> String {
    pad_metric(&format_bps(bps), METRIC_BPS_CHARS)
}

/// Human-readable event rate (e.g. write syscalls/s) without padding.
pub fn format_rate(rate: f64) -> String {
    if rate < 0.05 {
        "0/s".into()
    } else if rate < 10.0 {
        format!("{rate:.1}/s")
    } else if rate < 1000.0 {
        format!("{rate:.0}/s")
    } else if rate < 1_000_000.0 {
        format!("{:.1}k/s", rate / 1000.0)
    } else {
        format!("{:.1}M/s", rate / 1_000_000.0)
    }
}

/// Fixed-width event rate for tables.
pub fn metric_rate(rate: f64) -> String {
    pad_metric(&format_rate(rate), METRIC_RATE_CHARS)
}

// ── Width-safe scopes ───────────────────────────────────────────────────────

/// Pin this scope so children cannot expand past the current available width.
pub fn fit_width(ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> InnerResponse<()> {
    let w = ui.available_width().max(1.0);
    ui.scope(|ui| {
        ui.set_max_width(w);
        add(ui);
    })
}

/// Same as [`fit_width`], but also set `min_width` so framed children fill the residual.
pub fn fill_width(ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> InnerResponse<()> {
    let w = ui.available_width().max(1.0);
    ui.scope(|ui| {
        ui.set_min_width(w);
        ui.set_max_width(w);
        add(ui);
    })
}

/// Non-justified vertical stack — **no giant gaps** when the parent is taller
/// than the content (the usual egui “waterfall” inside stretched cards).
pub fn vstack(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui)) -> InnerResponse<()> {
    let gap = theme.spacing.sm;
    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing = Vec2::new(ui.spacing().item_spacing.x, gap);
        add(ui);
    })
}

// ── Surfaces ────────────────────────────────────────────────────────────────

/// Themed card that **fills** the parent column without overflowing past it.
///
/// Content is stacked with [`vstack`] so tall parents do not justify gaps.
pub fn card(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui)) -> InnerResponse<()> {
    let outer = ui.available_width().max(1.0);
    ui.set_max_width(outer);
    theme.card_frame().show(ui, |ui| {
        let inner = ui.available_width().max(1.0);
        ui.set_min_width(inner);
        ui.set_max_width(inner);
        vstack(ui, theme, add);
    })
}

/// Horizontal chrome (inner margin + stroke) added by [`Theme::card_frame`].
///
/// Outer card width = content width + this value. Pure / testable.
pub fn card_frame_chrome_x(theme: &Theme) -> f32 {
    // card_frame: Margin::same(spacing.md) + Stroke width 1 on each side.
    theme.spacing.md * 2.0 + 2.0
}

/// Compact card with a **fixed outer width** that hugs content height.
///
/// Use for gauge tiles and anomaly panels that must not stretch across the
/// window or absorb leftover horizontal space between siblings.
///
/// Outer size is clamped to residual width; content width subtracts frame
/// chrome so the painted card never exceeds the budget (avoids grid overflow).
pub fn compact_card(
    ui: &mut Ui,
    theme: &Theme,
    width: f32,
    add: impl FnOnce(&mut Ui),
) -> InnerResponse<()> {
    let chrome = card_frame_chrome_x(theme);
    // Outer size must fit the residual (grid cell / viewport).
    let outer = width.min(ui.available_width()).max(1.0);
    let inner = (outer - chrome).max(1.0);

    ui.allocate_ui_with_layout(Vec2::new(outer, 0.0), Layout::top_down(Align::Min), |ui| {
        ui.set_min_width(outer);
        ui.set_max_width(outer);
        // Do NOT set_clip_rect here: max_rect starts with height 0 before
        // children run, which would hide all content (blank window).
        theme.card_frame().show(ui, |ui| {
            ui.set_min_width(inner);
            ui.set_max_width(inner);
            vstack(ui, theme, add);
        });
    })
}

/// Soft-bordered inset row (popover surface) capped to parent width.
pub fn inset_row(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui)) -> InnerResponse<()> {
    let outer = ui.available_width().max(1.0);
    ui.set_max_width(outer);
    Frame::new()
        .fill(theme.palette.popover_bg)
        .stroke(Stroke::new(1.0, theme.palette.border_soft))
        .corner_radius(theme.spacing.radius_sm)
        .inner_margin(Margin::symmetric(
            theme.spacing.md as i8,
            theme.spacing.sm as i8,
        ))
        .show(ui, |ui| {
            let w = ui.available_width().max(1.0);
            ui.set_min_width(w);
            ui.set_max_width(w);
            vstack(ui, theme, add);
        })
}

// ── Horizontal composition ──────────────────────────────────────────────────

/// Horizontal flow that **wraps** before clipping the edge (toolbars / chips).
pub fn hflow(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui)) -> InnerResponse<()> {
    let w = ui.available_width().max(1.0);
    let gap = theme.spacing.sm;
    ui.scope(|ui| {
        ui.set_max_width(w);
        ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
        ui.horizontal_wrapped(add);
    })
}

/// Pack of **compact** children (fixed-size cards/tiles) that wrap without
/// stretching leftover horizontal space into empty gaps between items.
///
/// Same wrapping as [`hflow`], but spacing defaults to `md` so packs match
/// gauge / anomaly card groups.
pub fn pack(ui: &mut Ui, theme: &Theme, add: impl FnOnce(&mut Ui)) -> InnerResponse<()> {
    let w = ui.available_width().max(1.0);
    let gap = theme.spacing.md;
    ui.scope(|ui| {
        ui.set_max_width(w);
        ui.spacing_mut().item_spacing = Vec2::new(gap, gap);
        ui.horizontal_wrapped(add);
    })
}

/// Leading content grows into remaining width; trailing actions stay visible.
///
/// Height hugs content. Do **not** use bare `with_layout(… Align::Center)` here:
/// egui expands centered horizontal frames to the parent's full available
/// height, so `min_rect` eats the panel and everything below (toolbars,
/// tables, scroll areas) gets zero height — a blank detail pane.
pub fn lead_trail(
    ui: &mut Ui,
    leading: impl FnOnce(&mut Ui),
    trailing: impl FnOnce(&mut Ui),
) -> InnerResponse<()> {
    let w = ui.available_width().max(1.0);
    ui.allocate_ui_with_layout(
        Vec2::new(w, 0.0),
        Layout::right_to_left(Align::Center),
        |ui| {
            trailing(ui);
            let rest = ui.available_width().max(1.0);
            ui.allocate_ui_with_layout(
                Vec2::new(rest, 0.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_max_width(rest);
                    ui.set_min_width(rest);
                    leading(ui);
                },
            );
        },
    )
}

/// Two columns when [`side_by_side`] says so; otherwise stack.
pub fn two_col(
    ui: &mut Ui,
    theme: &Theme,
    min_col: f32,
    left: impl FnOnce(&mut Ui),
    right: impl FnOnce(&mut Ui),
) {
    let gap = theme.spacing.md;
    let avail = ui.available_width();
    if side_by_side(avail, min_col, gap) {
        ui.columns(2, |cols| {
            cols[0].set_width(cols[0].available_width());
            left(&mut cols[0]);
            cols[1].set_width(cols[1].available_width());
            right(&mut cols[1]);
        });
    } else {
        left(ui);
        ui.add_space(gap);
        right(ui);
    }
}

// ── Page shell ──────────────────────────────────────────────────────────────
//
// Application UI at the top level is **grid-only**. `page_body` /
// `central_page` take a `GridCtx` callback so free-form stacking cannot be
// the supported root composition path. Nested content lives inside cells.

/// Escape hatch: pin width + vertical scroll **without** a top-level grid.
///
/// Prefer [`page_body`] / [`central_page`] for application UI. Use this only
/// when a demo or special chrome needs free-form scrolling content.
pub fn page_scroll(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let w = ui.available_width().max(1.0);
    ui.set_max_width(w);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("vidya_page_scroll")
        .show(ui, |ui| {
            let inner = ui.available_width().max(1.0);
            ui.set_max_width(inner);
            add(ui);
        });
}

/// Scrollable application page whose **top-level content is a grid**.
///
/// Defaults to a single flex column: each [`GridCtx::section`] (or `g.row`)
/// is a full-width page block. Nested multi-column layouts use [`grid_cols`]
/// inside a section/cell.
///
/// This is the supported root for app UI — the callback cannot receive a raw
/// free-stack `Ui` at the page root.
pub fn page_body(
    ui: &mut Ui,
    theme: &Theme,
    id: impl Hash,
    add: impl FnOnce(&mut GridCtx<'_, '_>),
) {
    page_body_cols(ui, theme, id, &[ColSpec::Flex], add);
}

/// Like [`page_body`], but with explicit top-level column specs.
pub fn page_body_cols(
    ui: &mut Ui,
    theme: &Theme,
    id: impl Hash,
    cols: &[ColSpec],
    add: impl FnOnce(&mut GridCtx<'_, '_>),
) {
    page_scroll(ui, |ui| {
        // Page grid: no zebra striping, larger vertical gap between sections.
        grid_cols_with(ui, theme, id, cols, GridOpts::page(theme), add);
    });
}

/// Full central page: themed page frame + grid-enforced [`page_body`].
///
/// Application central content **must** be composed via the grid DSL
/// (`g.section` / `g.row`). See module docs.
pub fn central_page(
    ctx: &egui::Context,
    theme: &Theme,
    id: impl Hash,
    add: impl FnOnce(&mut GridCtx<'_, '_>),
) -> egui::InnerResponse<()> {
    egui::CentralPanel::default()
        .frame(theme.page_frame())
        .show(ctx, |ui| {
            page_body(ui, theme, id, add);
        })
}

/// Like [`central_page`], but with explicit top-level column specs.
pub fn central_page_cols(
    ctx: &egui::Context,
    theme: &Theme,
    id: impl Hash,
    cols: &[ColSpec],
    add: impl FnOnce(&mut GridCtx<'_, '_>),
) -> egui::InnerResponse<()> {
    egui::CentralPanel::default()
        .frame(theme.page_frame())
        .show(ctx, |ui| {
            page_body_cols(ui, theme, id, cols, add);
        })
}

// ── Grid layout DSL ─────────────────────────────────────────────────────────
//
// Declarative rows/columns so apps do not hand-roll `egui::Grid`:
//
// ```ignore
// vidya::grid_cols(ui, &th, "procs", &[
//     ColSpec::Flex,
//     ColSpec::Flex,
//     ColSpec::MetricBps,
//     ColSpec::MetricRate,
// ], |g| {
//     g.row(|r| {
//         r.heading("Name");
//         r.heading("Path");
//         r.heading("Write");
//         r.heading("Write freq");
//     });
//     g.row(|r| {
//         r.text("chrome");
//         r.dim("/usr/bin/chrome");
//         r.metric_bps(write);
//         r.metric_rate(freq);
//     });
// });
// ```

/// Column width hint for the grid DSL.
#[derive(Debug, Clone, Copy)]
pub enum ColSpec {
    /// Grow / shrink with content and leftover space.
    Flex,
    /// Fixed pixel width (e.g. custom metric column).
    Fixed(f32),
    /// Throughput metric column width from theme.
    MetricBps,
    /// Event-rate metric column width from theme.
    MetricRate,
}

impl ColSpec {
    /// Desired / minimum width for fixed metric columns; `None` = flex.
    pub fn px(self, theme: &Theme) -> Option<f32> {
        match self {
            ColSpec::Flex => None,
            ColSpec::Fixed(w) => Some(w),
            ColSpec::MetricBps => Some(metric_cell_px(theme, METRIC_BPS_CHARS)),
            ColSpec::MetricRate => Some(metric_cell_px(theme, METRIC_RATE_CHARS)),
        }
    }
}

/// Minimum width (px) reserved for a flex column when distributing space.
pub const FLEX_COL_MIN_PX: f32 = 48.0;

/// Distribute per-column **max** widths so `sum(widths) + gaps ≤ avail`.
///
/// - Fixed specs (`Some(w)`) keep `w` when the budget allows; otherwise they
///   scale down proportionally after flex mins are reserved.
/// - Flex specs (`None`) share the remaining budget equally (each ≥ [`FLEX_COL_MIN_PX`]
///   when possible).
///
/// Pure policy — unit-tested without a window.
pub fn distribute_col_max(specs: &[Option<f32>], avail: f32, gap: f32) -> Vec<f32> {
    let n = specs.len().max(1);
    let gaps = gap * (n.saturating_sub(1) as f32);
    let budget = (avail.max(1.0) - gaps).max(1.0);

    // No explicit specs → equal flex slices.
    if specs.is_empty() {
        return vec![budget / n as f32; n];
    }

    let flex_n = specs.iter().filter(|s| s.is_none()).count();
    let fixed_sum: f32 = specs.iter().filter_map(|s| *s).sum();

    if flex_n == 0 {
        if fixed_sum <= budget {
            return specs.iter().map(|s| s.unwrap_or(0.0)).collect();
        }
        let scale = budget / fixed_sum.max(1.0);
        return specs.iter().map(|s| s.unwrap_or(0.0) * scale).collect();
    }

    let flex_floor = FLEX_COL_MIN_PX * flex_n as f32;
    let fixed_budget = if fixed_sum + flex_floor <= budget {
        fixed_sum
    } else {
        (budget - flex_floor).max(0.0)
    };
    let fixed_scale = if fixed_sum > fixed_budget && fixed_sum > 0.0 {
        fixed_budget / fixed_sum
    } else {
        1.0
    };

    let mut out = vec![0.0_f32; n];
    let mut used_fixed = 0.0_f32;
    for (i, s) in specs.iter().enumerate() {
        if let Some(w) = s {
            out[i] = (*w * fixed_scale).max(1.0);
            used_fixed += out[i];
        }
    }
    let flex_each = ((budget - used_fixed) / flex_n as f32).max(1.0);
    for (i, s) in specs.iter().enumerate() {
        if s.is_none() {
            out[i] = flex_each;
        }
    }
    out
}

/// Live grid session (inside `egui::Grid`).
pub struct GridCtx<'ui, 'th> {
    ui: &'ui mut Ui,
    theme: &'th Theme,
    /// Desired floors for fixed columns (`None` = flex).
    col_widths: Vec<Option<f32>>,
    /// Hard max width per column so the grid fits the residual viewport.
    col_max: Vec<f32>,
}

/// Options for [`grid_cols_with`] / the page shell grid.
#[derive(Debug, Clone, Copy)]
pub struct GridOpts {
    /// Zebra striping (tables on; page shell off).
    pub striped: bool,
    /// Column gap (x) and row gap (y).
    pub spacing: Vec2,
}

impl GridOpts {
    /// Defaults for nested data tables / multi-column surfaces.
    pub fn table(theme: &Theme) -> Self {
        Self {
            striped: true,
            spacing: Vec2::new(theme.spacing.md, 2.0),
        }
    }

    /// Defaults for top-level [`page_body`] (no striping, section-sized row gap).
    pub fn page(theme: &Theme) -> Self {
        Self {
            striped: false,
            spacing: Vec2::new(theme.spacing.md, theme.spacing.lg),
        }
    }
}

/// Grid with explicit column specs (recommended for metric tables).
///
/// The grid container is pinned to `ui.available_width()` so it cannot grow
/// past the parent / viewport residual. Column max widths are distributed via
/// [`distribute_col_max`].
pub fn grid_cols(
    ui: &mut Ui,
    theme: &Theme,
    id: impl Hash,
    cols: &[ColSpec],
    add: impl FnOnce(&mut GridCtx<'_, '_>),
) {
    grid_cols_with(ui, theme, id, cols, GridOpts::table(theme), add);
}

/// Grid with explicit column specs and layout options.
pub fn grid_cols_with(
    ui: &mut Ui,
    theme: &Theme,
    id: impl Hash,
    cols: &[ColSpec],
    opts: GridOpts,
    add: impl FnOnce(&mut GridCtx<'_, '_>),
) {
    let avail = ui.available_width().max(1.0);
    let n = if cols.is_empty() { 16 } else { cols.len() };
    let col_widths: Vec<Option<f32>> = cols.iter().map(|c| c.px(theme)).collect();
    let spacing = opts.spacing;
    let col_max = if cols.is_empty() {
        distribute_col_max(&vec![None; n], avail, spacing.x)
    } else {
        distribute_col_max(&col_widths, avail, spacing.x)
    };
    let cell_max = col_max.iter().copied().fold(24.0_f32, f32::max);

    // Pin container to residual width (no early clip_rect — that can zero out
    // height before layout and blank the whole page).
    ui.scope(|ui| {
        ui.set_max_width(avail);
        Grid::new(id)
            .num_columns(n)
            .spacing(spacing)
            .min_col_width(24.0)
            .max_col_width(cell_max)
            .striped(opts.striped)
            .show(ui, |ui| {
                ui.set_max_width(avail);
                let mut ctx = GridCtx {
                    ui,
                    theme,
                    col_widths,
                    col_max,
                };
                add(&mut ctx);
            });
    });
}

/// Grid with all-flex columns. Prefer [`grid_cols`] when you have metrics.
pub fn grid(
    ui: &mut Ui,
    theme: &Theme,
    id: impl Hash,
    add: impl FnOnce(&mut GridCtx<'_, '_>),
) {
    grid_cols(ui, theme, id, &[], add);
}

impl<'ui, 'th> GridCtx<'ui, 'th> {
    /// Full-width page section: one row containing one cell.
    ///
    /// Primary building block for [`page_body`] / [`central_page`]. Put nested
    /// grids, cards, and free-form widgets **inside** the section — not as
    /// siblings of the page grid.
    pub fn section(&mut self, add: impl FnOnce(&mut Ui)) {
        self.row(|r| {
            r.cell(add);
        });
    }

    /// One table row. Cells are written left→right; `end_row` is automatic.
    pub fn row(&mut self, add: impl FnOnce(&mut RowDsl<'_, 'th>)) {
        let mut col_i = 0usize;
        let mut row = RowDsl {
            ui: self.ui,
            theme: self.theme,
            col_widths: &self.col_widths,
            col_max: &self.col_max,
            col_i: &mut col_i,
        };
        add(&mut row);
        self.ui.end_row();
    }

    /// Access the underlying grid `Ui` (escape hatch).
    pub fn ui(&mut self) -> &mut Ui {
        self.ui
    }

    pub fn theme(&self) -> &Theme {
        self.theme
    }
}

/// One row inside [`GridCtx::row`].
pub struct RowDsl<'ui, 'th> {
    ui: &'ui mut Ui,
    theme: &'th Theme,
    col_widths: &'ui [Option<f32>],
    col_max: &'ui [f32],
    col_i: &'ui mut usize,
}

impl<'ui, 'th> RowDsl<'ui, 'th> {
    fn advance(&mut self) {
        *self.col_i += 1;
    }

    fn width_hint(&self) -> Option<f32> {
        self.col_widths.get(*self.col_i).copied().flatten()
    }

    /// Hard max for this column (viewport residual budget).
    fn col_max(&self) -> f32 {
        self.col_max
            .get(*self.col_i)
            .copied()
            .unwrap_or(self.ui.available_width().max(1.0))
    }

    fn metric_width(&self) -> f32 {
        let floor = self
            .width_hint()
            .unwrap_or_else(|| metric_cell_px(self.theme, METRIC_BPS_CHARS));
        floor.min(self.col_max()).max(1.0)
    }

    /// Free-form cell, sized to the column max width (top-aligned in the row).
    pub fn cell(&mut self, add: impl FnOnce(&mut Ui)) {
        let max_w = self.col_max().max(1.0);
        // Width-capped, top-down group = one grid cell.
        //
        // Do **not** use `allocate_ui_with_layout(Vec2::new(max_w, 0.0), …)`.
        // egui::Grid places desired sizes with `Align2::LEFT_CENTER`, so a
        // zero-height seed is parked mid-row and content grows downward —
        // leaving a large empty band above (page sections look vertically
        // centered in the panel). `with_layout` uses the cell's available
        // rect from the top of the row instead.
        self.ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.set_min_width(max_w);
            ui.set_max_width(max_w);
            add(ui);
        });
        self.advance();
    }

    /// Column header (caption, strong, secondary). Capped to column max.
    pub fn heading(&mut self, text: &str) {
        let size = self.theme.type_scale.caption;
        let max_w = self.col_max();
        let rt = RichText::new(text)
            .size(size)
            .strong()
            .color(self.theme.palette.text_secondary);
        let need = measure_text(self.ui, text, size, false) + self.theme.spacing.sm;
        let width = if let Some(hint) = self.width_hint() {
            hint.max(need).min(max_w)
        } else {
            need.min(max_w)
        };
        self.ui.allocate_ui_with_layout(
            Vec2::new(width.max(1.0), size + 6.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.set_max_width(width.max(1.0));
                ui.add(egui::Label::new(rt).truncate());
            },
        );
        self.advance();
    }

    /// Primary body text (flex) — truncates within column max.
    pub fn text(&mut self, text: &str) {
        table_text_capped(self.ui, self.theme, text, true, self.col_max());
        self.advance();
    }

    /// Secondary caption text (flex) — truncates within column max.
    pub fn dim(&mut self, text: &str) {
        table_text_capped(self.ui, self.theme, text, false, self.col_max());
        self.advance();
    }

    /// Warning-colored strong caption (e.g. anomaly process name).
    pub fn warn(&mut self, text: &str) {
        let max_w = self.col_max();
        let rt = RichText::new(text)
            .size(self.theme.type_scale.caption)
            .strong()
            .color(self.theme.palette.warning);
        self.ui.scope(|ui| {
            ui.set_max_width(max_w);
            ui.add(egui::Label::new(rt).truncate());
        });
        self.advance();
    }

    /// Right-aligned monospace metric (`text` from [`metric_bps`] / [`metric_rate`]).
    pub fn metric(&mut self, text: &str) {
        metric_cell(self.ui, self.theme, self.metric_width(), text, false);
        self.advance();
    }

    /// Secondary (dim) metric.
    pub fn metric_dim(&mut self, text: &str) {
        metric_cell(self.ui, self.theme, self.metric_width(), text, true);
        self.advance();
    }

    /// Throughput from raw B/s.
    pub fn metric_bps(&mut self, bps: f64) {
        let w = self
            .width_hint()
            .unwrap_or_else(|| metric_cell_px(self.theme, METRIC_BPS_CHARS))
            .min(self.col_max())
            .max(1.0);
        metric_cell(self.ui, self.theme, w, &metric_bps(bps), false);
        self.advance();
    }

    /// Event rate from raw 1/s.
    pub fn metric_rate(&mut self, rate: f64) {
        let w = self
            .width_hint()
            .unwrap_or_else(|| metric_cell_px(self.theme, METRIC_RATE_CHARS))
            .min(self.col_max())
            .max(1.0);
        metric_cell(self.ui, self.theme, w, &metric_rate(rate), true);
        self.advance();
    }
}

// ── Metrics / tables ────────────────────────────────────────────────────────

/// Measure laid-out width of text (no wrap).
fn measure_text(ui: &Ui, text: &str, size: f32, mono: bool) -> f32 {
    let family = if mono {
        egui::FontFamily::Monospace
    } else {
        egui::FontFamily::Proportional
    };
    let font = FontId::new(size, family);
    ui.fonts(|f| {
        f.layout_no_wrap(text.to_owned(), font, egui::Color32::WHITE)
            .size()
            .x
    })
}

/// Measure monospace caption width for `text`.
pub fn measure_mono_caption(ui: &Ui, theme: &Theme, text: &str) -> f32 {
    measure_text(ui, text, theme.type_scale.caption, true)
}

/// Paint a monospace metric string, right-edge aligned.
///
/// `min_width` is the cell size (from ColSpec floor, clamped by
/// [`distribute_col_max`] to the residual viewport). Text is clip-rect'd so it
/// never paints past the cell.
pub fn metric_cell(ui: &mut Ui, theme: &Theme, min_width: f32, text: &str, secondary: bool) {
    let width = min_width.max(1.0);
    let h = theme.type_scale.caption + 8.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, h), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let color = if secondary {
        theme.palette.text_secondary
    } else {
        theme.palette.text
    };
    let font = FontId::monospace(theme.type_scale.caption);
    let painter = ui.painter().with_clip_rect(rect);
    let pos = egui::pos2(rect.right() - theme.spacing.xs.max(2.0), rect.center().y);
    painter.text(pos, egui::Align2::RIGHT_CENTER, text, font, color);
}

/// Column kind for [`data_table`] (thin table helper over the grid DSL).
#[derive(Debug, Clone, Copy)]
pub enum ColKind {
    Flex,
    Metric { width: f32 },
}

/// One column header + kind for [`data_table`].
#[derive(Debug, Clone, Copy)]
pub struct Col {
    pub header: &'static str,
    pub kind: ColKind,
}

/// Striped data table built on [`grid_cols`].
///
/// Prefer the row DSL (`grid_cols` + `g.row`) for new code; this keeps the
/// index-callback shape used by existing consumers.
pub fn data_table(
    ui: &mut Ui,
    theme: &Theme,
    id: impl Hash,
    columns: &[Col],
    mut row: impl FnMut(&mut Ui, usize),
    row_count: usize,
) {
    let specs: Vec<ColSpec> = columns
        .iter()
        .map(|c| match c.kind {
            ColKind::Flex => ColSpec::Flex,
            ColKind::Metric { width } => ColSpec::Fixed(width),
        })
        .collect();

    // Same viewport pin + col-max policy as [`grid_cols`].
    let avail = ui.available_width().max(1.0);
    let n = columns.len().max(1);
    let col_widths: Vec<Option<f32>> = specs.iter().map(|c| c.px(theme)).collect();
    let spacing = Vec2::new(theme.spacing.md, 2.0);
    let col_max = distribute_col_max(&col_widths, avail, spacing.x);
    let cell_max = col_max.iter().copied().fold(40.0_f32, f32::max);

    ui.scope(|ui| {
        ui.set_max_width(avail);
        Grid::new(id)
            .num_columns(n)
            .spacing(spacing)
            .min_col_width(24.0)
            .max_col_width(cell_max)
            .striped(true)
            .show(ui, |ui| {
                ui.set_max_width(avail);
                let mut col_i = 0usize;
                {
                    let mut r = RowDsl {
                        ui,
                        theme,
                        col_widths: &col_widths,
                        col_max: &col_max,
                        col_i: &mut col_i,
                    };
                    for col in columns {
                        r.heading(col.header);
                    }
                }
                ui.end_row();
                for i in 0..row_count {
                    row(ui, i);
                    ui.end_row();
                }
            });
    });
}

/// Flex text cell for [`data_table`] rows / grid rows (truncate to available).
pub fn table_text(ui: &mut Ui, theme: &Theme, text: &str, primary: bool) {
    table_text_capped(ui, theme, text, primary, ui.available_width());
}

/// Flex text capped to `max_w` so grid columns stay within the viewport budget.
pub fn table_text_capped(ui: &mut Ui, theme: &Theme, text: &str, primary: bool, max_w: f32) {
    let color = if primary {
        theme.palette.text
    } else {
        theme.palette.text_secondary
    };
    ui.scope(|ui| {
        ui.set_max_width(max_w.max(1.0));
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .size(if primary {
                        theme.type_scale.body
                    } else {
                        theme.type_scale.caption
                    })
                    .color(color),
            )
            .truncate(),
        );
    });
}

/// Metric cell for [`data_table`] rows (`text` should be [`metric_bps`] / [`metric_rate`]).
pub fn table_metric(ui: &mut Ui, theme: &Theme, width: f32, text: &str, secondary: bool) {
    metric_cell(ui, theme, width, text, secondary);
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;

    #[test]
    fn side_by_side_policy_matches_two_col_breakpoint() {
        let gap = 12.0;
        let min = 160.0;
        assert!(!side_by_side(100.0, min, gap));
        assert!(!side_by_side(min * 2.0 + gap - 1.0, min, gap));
        assert!(side_by_side(min * 2.0 + gap, min, gap));
        assert!(side_by_side(800.0, min, gap));
        assert!(!side_by_side(800.0, 0.0, gap));
    }

    #[test]
    fn metric_bps_fixed_char_width() {
        for bps in [0.0, 100.0, 44.8 * 1024.0, 2.0 * 1024.0 * 1024.0, 999.0] {
            let s = metric_bps(bps);
            assert_eq!(
                s.chars().count(),
                METRIC_BPS_CHARS,
                "metric_bps({bps}) = {s:?}"
            );
            assert!(
                s.ends_with(format_bps(bps).as_str()),
                "padding must preserve value"
            );
        }
    }

    #[test]
    fn metric_rate_fixed_char_width() {
        for rate in [0.0, 1.5, 134.0, 2600.0, 1_500_000.0] {
            let s = metric_rate(rate);
            assert_eq!(
                s.chars().count(),
                METRIC_RATE_CHARS,
                "metric_rate({rate}) = {s:?}"
            );
            assert!(s.contains(format_rate(rate).as_str()) || s.ends_with(format_rate(rate).as_str()));
        }
    }

    #[test]
    fn pad_metric_is_identity_when_already_wide() {
        let long = "123456789012345"; // 15 > 14
        assert_eq!(pad_metric(long, METRIC_BPS_CHARS), long);
    }

    #[test]
    fn metric_cell_px_scales_with_theme_caption() {
        let th = Theme::dark();
        let a = metric_cell_px(&th, METRIC_BPS_CHARS);
        let b = metric_cell_px(&th, METRIC_RATE_CHARS);
        assert!(a > b);
        assert!(a > 40.0);
        // Floor must cover full padded glyph run (chars × 0.72em + pad).
        let floor = th.type_scale.caption * 0.72 * METRIC_BPS_CHARS as f32;
        assert!(
            a >= floor,
            "metric_cell_px={a} must be >= glyph floor {floor}"
        );
    }

    #[test]
    fn metric_bps_string_never_exceeds_char_budget() {
        // Ensures the padded formatter and char budget stay in sync so
        // metric_cell_px floors remain meaningful.
        for bps in [0.0, 1.0, 512.0, 1024.0 * 50.0, 1024.0 * 1024.0 * 9.9] {
            let s = metric_bps(bps);
            assert!(
                s.chars().count() <= METRIC_BPS_CHARS
                    || s.chars().count() == METRIC_BPS_CHARS,
                "unexpected width for {s:?}"
            );
            assert_eq!(s.chars().count(), METRIC_BPS_CHARS);
        }
    }

    #[test]
    fn default_min_col_positive() {
        let th = Theme::dark();
        assert!(default_min_col(&th) > 100.0);
    }

    #[test]
    fn col_spec_resolves_metric_widths() {
        let th = Theme::dark();
        assert!(ColSpec::Flex.px(&th).is_none());
        assert_eq!(ColSpec::Fixed(120.0).px(&th), Some(120.0));
        let bps = ColSpec::MetricBps.px(&th).unwrap();
        let rate = ColSpec::MetricRate.px(&th).unwrap();
        assert!(bps > rate);
        assert!((bps - metric_cell_px(&th, METRIC_BPS_CHARS)).abs() < 0.01);
    }

    #[test]
    fn grid_opts_page_is_not_striped_and_uses_lg_row_gap() {
        let th = Theme::dark();
        let page = GridOpts::page(&th);
        let table = GridOpts::table(&th);
        assert!(!page.striped);
        assert!(table.striped);
        assert!((page.spacing.y - th.spacing.lg).abs() < 0.01);
        assert!((table.spacing.y - 2.0).abs() < 0.01);
    }

    #[test]
    fn page_shell_api_is_grid_only() {
        // Source-level contract: page_body / central_page take GridCtx, not free Ui.
        let layout = include_str!("layout.rs");
        // Signature block for page_body must include theme + GridCtx (not free Ui only).
        let start = layout
            .find("pub fn page_body(\n")
            .expect("page_body definition");
        let sig = &layout[start..start + 280];
        assert!(
            sig.contains("theme: &Theme"),
            "page_body must take theme for grid"
        );
        assert!(
            sig.contains("GridCtx"),
            "page_body must take GridCtx callback: {sig}"
        );
        assert!(
            !sig.contains("FnOnce(&mut Ui)"),
            "page_body must not accept free-form Ui: {sig}"
        );
        assert!(
            layout.contains("page_body(ui, theme, id, add)"),
            "central_page must route through grid page_body"
        );
        assert!(
            layout.contains("/// Full-width page section"),
            "GridCtx::section is the preferred page building block"
        );
        // Escape hatch exists but is not the app path.
        assert!(layout.contains("pub fn page_scroll("));
    }

    #[test]
    fn distribute_col_max_never_exceeds_budget() {
        let gap = 12.0;
        let specs = vec![None, None, Some(100.0), Some(80.0)];
        for avail in [200.0_f32, 400.0, 800.0, 100.0] {
            let maxes = distribute_col_max(&specs, avail, gap);
            let gaps = gap * (specs.len() - 1) as f32;
            let sum: f32 = maxes.iter().sum();
            assert!(
                sum + gaps <= avail + 0.5,
                "sum={sum} gaps={gaps} avail={avail} maxes={maxes:?}"
            );
            assert_eq!(maxes.len(), specs.len());
        }
    }

    #[test]
    fn distribute_col_max_scales_fixed_when_tight() {
        let specs = vec![Some(200.0), Some(200.0)];
        let maxes = distribute_col_max(&specs, 200.0, 0.0);
        assert!((maxes[0] + maxes[1] - 200.0).abs() < 0.01);
        assert!(maxes[0] < 200.0);
    }

    #[test]
    fn distribute_all_flex_equal() {
        let specs = vec![None, None, None, None];
        let maxes = distribute_col_max(&specs, 400.0, 0.0);
        for w in &maxes {
            assert!((*w - 100.0).abs() < 0.01);
        }
    }

    #[test]
    fn card_frame_chrome_x_is_margins_plus_stroke() {
        let th = Theme::dark();
        // md*2 + 1px stroke each side
        assert!((card_frame_chrome_x(&th) - (th.spacing.md * 2.0 + 2.0)).abs() < 0.01);
        assert!(card_frame_chrome_x(&th) > th.spacing.md);
    }

    #[test]
    fn four_flex_cols_fit_viewport_budget() {
        // Same shape as the usage gauge row.
        let gap = 12.0;
        let avail = 700.0;
        let specs = vec![None, None, None, None];
        let maxes = distribute_col_max(&specs, avail, gap);
        let sum: f32 = maxes.iter().sum::<f32>() + gap * 3.0;
        assert!(sum <= avail + 0.01, "sum={sum} avail={avail}");
        for w in maxes {
            assert!(w > 50.0);
        }
    }
}
