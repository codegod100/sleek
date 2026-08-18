//! Extra glyph coverage for UI punctuation, math-in-prose, and block art.
//!
//! egui’s default Ubuntu Light lacks many symbols used in HIG-style copy
//! (`→`, `●`, `○`, `▾`/`▴`, en/em dashes, curly quotes, …), common LLM
//! math outside `$…$` (`ℝ`, `ⁿ`, `∑`, Greek, …), and box/block elements
//! used in Bluesky bios (`█▄▀░`, box drawing). On Android those codepoints
//! render as hollow boxes (“tofu”).
//!
//! [`install_symbol_font`] registers a DejaVu Sans subset as a **fallback**
//! so primary text stays Ubuntu, but missing symbols still draw.

use egui::{
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
    Context, FontData, FontFamily,
};

/// Subset of DejaVu Sans covering UI symbols, math-in-prose, and box/block art.
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

#[cfg(test)]
mod tests {
    use super::{install_symbol_font, VIDYA_SYMBOLS_TTF};

    /// Glyphs from a real Bluesky bio (nandi.uk) that previously tofued in Sleek.
    const BLOCK_ART: &str = "█▄▀░";

    #[test]
    fn symbol_font_covers_block_elements() {
        assert!(
            VIDYA_SYMBOLS_TTF.len() > 10_000,
            "vidya-symbols.ttf looks empty/truncated"
        );

        let ctx = egui::Context::default();
        install_symbol_font(&ctx);
        // Allocate fonts so fallback families are built.
        ctx.begin_pass(egui::RawInput::default());

        let missing: Vec<char> = ctx.fonts(|fonts| {
            let font_id = egui::FontId::proportional(16.0);
            BLOCK_ART
                .chars()
                .filter(|&c| !fonts.has_glyph(&font_id, c))
                .collect()
        });
        assert!(
            missing.is_empty(),
            "proportional fallback missing glyphs: {missing:?}"
        );
    }
}
