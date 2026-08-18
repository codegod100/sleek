//! Extra glyph coverage for UI punctuation and math-in-prose Unicode.
//!
//! egui’s default Ubuntu Light lacks many symbols used in HIG-style copy
//! (`→`, `●`, `○`, `▾`/`▴`, en/em dashes, curly quotes, …) and common LLM
//! math outside `$…$` (`ℝ`, `ⁿ`, `∑`, Greek, thin spaces, …). On Android those
//! codepoints render as hollow boxes (“tofu”).
//!
//! [`install_symbol_font`] registers a DejaVu Sans subset as a **fallback**
//! so primary text stays Ubuntu, but missing symbols still draw.

use egui::{
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
    Context, FontData, FontFamily,
};

/// Subset of DejaVu Sans covering UI symbols + common math-in-prose Unicode.
/// See `assets/NOTICE` and `scripts/rebuild-symbols-ttf.sh`.
static VIDYA_SYMBOLS_TTF: &[u8] = include_bytes!("../assets/vidya-symbols.ttf");

const FONT_NAME: &str = "vidya-symbols";

/// Install the symbol fallback font (idempotent).
///
/// Safe to call every frame / from [`crate::apply`]: egui skips re-install when
/// the font name is already present.
pub fn install_symbol_font(ctx: &Context) {
    ctx.add_font(FontInsert::new(
        FONT_NAME,
        FontData::from_static(VIDYA_SYMBOLS_TTF),
        vec![
            InsertFontFamily {
                family: FontFamily::Proportional,
                // After Ubuntu / emoji — only used when those lack the glyph.
                priority: FontPriority::Lowest,
            },
            InsertFontFamily {
                family: FontFamily::Monospace,
                priority: FontPriority::Lowest,
            },
        ],
    ));
}
