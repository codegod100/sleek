//! Full-color emoji icons via the complete **Twemoji** 72×72 set.
//!
//! Any Unicode emoji (including ZWJ sequences, flags, skin tones) is mapped to
//! a color PNG by codepoint — not only a hand-picked half-dozen. Unknown or
//! non-emoji text falls back to a monochrome label.
//!
//! ```ignore
//! use vidya::{emoji_icon, icon, Icon, Theme};
//!
//! emoji_icon(ui, &th, "🚀", 20.0);
//! emoji_icon(ui, &th, "👨‍💻", 20.0);
//! icon(ui, &th, Icon::Heart, 20.0); // convenience alias
//! ```
//!
//! Pack: `assets/emoji/twemoji-72x72.zip` (~3.8k glyphs).  
//! License: Twemoji CC-BY 4.0 — see `assets/NOTICE`.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;

use egui::{
    load::SizedTexture, pos2, Align2, Color32, ColorImage, FontId, Id, Image, Rect, Response,
    Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2,
};

use crate::Theme;

/// Embedded Twemoji 72×72 pack (ZIP_STORED PNGs).
static TWEMOJI_ZIP: &[u8] = include_bytes!("../assets/emoji/twemoji-72x72.zip");

/// Filename → PNG bytes, built once on first lookup.
fn twemoji_pack() -> &'static HashMap<String, Vec<u8>> {
    static PACK: OnceLock<HashMap<String, Vec<u8>>> = OnceLock::new();
    PACK.get_or_init(|| {
        let mut map = HashMap::new();
        let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(TWEMOJI_ZIP)) else {
            return map;
        };
        for i in 0..archive.len() {
            let Ok(mut file) = archive.by_index(i) else {
                continue;
            };
            let name = file.name().to_string();
            if !name.ends_with(".png") {
                continue;
            }
            // Skip directory entries / paths with separators (zip-slip).
            if name.contains('/') || name.contains('\\') {
                continue;
            }
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() {
                map.insert(name, buf);
            }
        }
        map
    })
}

/// Named shortcuts for common reactions + UI chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    ThumbsUp,
    ThumbsDown,
    Heart,
    Laugh,
    Surprised,
    Frown,
    /// Drawn stroke — not a Twemoji glyph.
    Plus,
    /// Drawn stroke — overlapping pages (copy / duplicate).
    Copy,
}

impl Icon {
    pub const EMOJI: &'static [Icon] = &[
        Icon::ThumbsUp,
        Icon::Heart,
        Icon::Laugh,
        Icon::Surprised,
        Icon::Frown,
        Icon::ThumbsDown,
    ];

    pub const ALL: &'static [Icon] = &[
        Icon::ThumbsUp,
        Icon::Heart,
        Icon::Laugh,
        Icon::Surprised,
        Icon::Frown,
        Icon::ThumbsDown,
        Icon::Plus,
        Icon::Copy,
    ];

    /// True when this icon is stroke-drawn (not a Twemoji bitmap).
    pub fn is_stroke(self) -> bool {
        matches!(self, Icon::Plus | Icon::Copy)
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Icon::ThumbsUp => "👍",
            Icon::ThumbsDown => "👎",
            Icon::Heart => "❤️",
            Icon::Laugh => "😂",
            Icon::Surprised => "😮",
            Icon::Frown => "😢",
            Icon::Plus => "+",
            Icon::Copy => "⧉",
        }
    }
}

/// Strip only pure presentation noise (kept for callers / labels).
pub fn normalize_emoji(emoji: &str) -> String {
    emoji
        .chars()
        .filter(|&c| c != '\u{FE0E}' && c != '\u{FE0F}')
        .collect()
}

/// Map a well-known short emoji to [`Icon`] (convenience). Prefer [`emoji_icon`]
/// for arbitrary reactions — that covers the full Twemoji set.
pub fn icon_for_emoji(emoji: &str) -> Option<Icon> {
    let key: String = emoji
        .chars()
        .filter(|&c| {
            c != '\u{FE0E}'
                && c != '\u{FE0F}'
                && c != '\u{200D}'
                && !matches!(c, '\u{1F3FB}'..='\u{1F3FF}')
        })
        .collect();
    match key.as_str() {
        "👍" | "+1" => Some(Icon::ThumbsUp),
        "👎" | "-1" => Some(Icon::ThumbsDown),
        "❤" | "♥" => Some(Icon::Heart),
        "😂" | "😄" | "😆" | "🤣" => Some(Icon::Laugh),
        "😮" | "😯" | "😲" => Some(Icon::Surprised),
        "😢" | "😞" | "🙁" | "☹" | "😭" => Some(Icon::Frown),
        _ => None,
    }
}

/// True when we have a color Twemoji bitmap for this string.
pub fn has_emoji_icon(emoji: &str) -> bool {
    twemoji_png(emoji).is_some()
}

/// How many glyphs are in the embedded pack (for demos / diagnostics).
pub fn emoji_pack_len() -> usize {
    twemoji_pack().len()
}

// ── Twemoji codepoint keys (matches twemoji.js grabTheRightIcon) ────────────

const ZWJ: char = '\u{200D}';
const VS16: char = '\u{FE0F}';
const VS15: char = '\u{FE0E}';

/// Twemoji file stem: lowercase hex codepoints joined by `-`.
///
/// Rule from twemoji: if the sequence has **no** ZWJ, strip FE0F/FE0E;
/// if it has ZWJ, keep variation selectors (filenames include `fe0f`).
pub fn twemoji_key(emoji: &str) -> String {
    let has_zwj = emoji.contains(ZWJ);
    let chars = emoji.chars().filter(|&c| {
        if has_zwj {
            true
        } else {
            c != VS16 && c != VS15
        }
    });
    chars
        .map(|c| format!("{:x}", c as u32))
        .collect::<Vec<_>>()
        .join("-")
}

fn strip_skin_tones(emoji: &str) -> String {
    emoji
        .chars()
        .filter(|c| !matches!(c, '\u{1F3FB}'..='\u{1F3FF}'))
        .collect()
}

/// Candidate Twemoji stems, most specific first.
fn twemoji_key_candidates(emoji: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };

    let trimmed = emoji.trim();
    if trimmed.is_empty() {
        return out;
    }

    push(twemoji_key(trimmed));

    // Always also try VS-stripped form (covers assets that omit FE0F).
    let no_vs: String = trimmed.chars().filter(|&c| c != VS16 && c != VS15).collect();
    push(twemoji_key(&no_vs));
    // Force “no ZWJ rule” key on the raw string by stripping VS.
    push(
        no_vs
            .chars()
            .map(|c| format!("{:x}", c as u32))
            .collect::<Vec<_>>()
            .join("-"),
    );

    // Skin-tone fallback → base glyph.
    let no_skin = strip_skin_tones(trimmed);
    if no_skin != trimmed {
        push(twemoji_key(&no_skin));
        let no_skin_vs: String = no_skin.chars().filter(|&c| c != VS16 && c != VS15).collect();
        push(twemoji_key(&no_skin_vs));
    }

    // First extended grapheme-ish: take until second ZWJ-less “cluster” — for
    // multi-emoji paste, try the whole string then the first scalar sequence.
    // If still missing, try each scalar / ZWJ segment from the start.
    if trimmed.chars().count() > 1 {
        // Leading base only (first non-VS non-skin char + optional VS).
        if let Some(first) = trimmed.chars().find(|c| {
            *c != VS16 && *c != VS15 && !matches!(c, '\u{1F3FB}'..='\u{1F3FF}') && *c != ZWJ
        }) {
            push(format!("{:x}", first as u32));
        }
    }

    out
}

/// PNG bytes for an emoji, if present in the pack.
fn twemoji_png(emoji: &str) -> Option<&'static [u8]> {
    let pack = twemoji_pack();
    for key in twemoji_key_candidates(emoji) {
        let name = format!("{key}.png");
        if let Some(bytes) = pack.get(&name) {
            // Safe: pack is 'static, values live for process lifetime.
            return Some(bytes.as_slice());
        }
    }
    None
}

// ── painting ────────────────────────────────────────────────────────────────

/// Paint a named icon at `size`×`size`.
pub fn icon(ui: &mut Ui, theme: &Theme, icon: Icon, size: f32) -> Response {
    icon_colored(ui, theme.palette.text, icon, size)
}

/// Like [`icon`]; `color` only affects stroke icons ([`Icon::Plus`], [`Icon::Copy`]).
pub fn icon_colored(ui: &mut Ui, color: Color32, icon: Icon, size: f32) -> Response {
    let size = size.max(8.0);
    match icon {
        Icon::Plus | Icon::Copy => paint_stroke_icon(ui, color, icon, size),
        other => {
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
            paint_emoji_in(ui, rect, other.emoji(), color);
            response
        }
    }
}

/// Paint any emoji as a color Twemoji when available; else text fallback.
pub fn emoji_icon(ui: &mut Ui, theme: &Theme, emoji: &str, size: f32) -> Response {
    emoji_icon_colored(ui, theme, theme.palette.text, emoji, size)
}

/// Like [`emoji_icon`]; `color` is only for text / Plus fallback.
pub fn emoji_icon_colored(
    ui: &mut Ui,
    theme: &Theme,
    color: Color32,
    emoji: &str,
    size: f32,
) -> Response {
    let size = size.max(8.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_emoji_in(ui, rect, emoji, color);
    let _ = theme;
    response
}

/// Draw a stroke icon ([`Icon::Plus`], [`Icon::Copy`]) into `rect`.
/// Emoji icons need [`paint_emoji_in`] / [`paint_icon_in`].
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let w = (s * 0.12).clamp(1.5, 2.5);
    let stroke = Stroke::new(w, color);
    match icon {
        Icon::Plus => {
            let arm = s * 0.32;
            painter.line_segment([pos2(c.x - arm, c.y), pos2(c.x + arm, c.y)], stroke);
            painter.line_segment([pos2(c.x, c.y - arm), pos2(c.x, c.y + arm)], stroke);
        }
        Icon::Copy => {
            // Two overlapping pages — classic “copy” glyph (stroke only).
            let page_w = s * 0.38;
            let page_h = s * 0.46;
            let r = (s * 0.08).clamp(0.5, 2.5);
            let dx = s * 0.11;
            let dy = s * 0.11;
            let back = Rect::from_center_size(pos2(c.x - dx, c.y - dy), Vec2::new(page_w, page_h));
            let front = Rect::from_center_size(pos2(c.x + dx, c.y + dy), Vec2::new(page_w, page_h));
            painter.rect_stroke(back, r, stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(front, r, stroke, egui::StrokeKind::Inside);
        }
        _ => {}
    }
}

/// Draw a named [`Icon`] into `rect`.
pub fn paint_icon_in(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    match icon {
        Icon::Plus | Icon::Copy => paint_icon(ui.painter(), rect, icon, color),
        other => paint_emoji_in(ui, rect, other.emoji(), color),
    }
}

/// Draw any emoji into `rect` (color Twemoji or text fallback).
pub fn paint_emoji_in(ui: &Ui, rect: Rect, emoji: &str, color: Color32) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    if let Some(tex) = load_emoji_texture(ui.ctx(), emoji) {
        Image::from_texture(SizedTexture::new(tex.id(), tex.size_vec2()))
            .fit_to_exact_size(rect.size())
            .paint_at(ui, rect);
        return;
    }
    // Fallback: raw text (may be monochrome Noto / tofu).
    let shown = normalize_emoji(emoji);
    let size = rect.width().min(rect.height());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        if shown.is_empty() { emoji } else { &shown },
        FontId::proportional((size * 0.85).max(10.0)),
        color,
    );
}

/// Themed reaction chip: color emoji + optional count.
pub fn reaction_chip(
    ui: &mut Ui,
    theme: &Theme,
    emoji: &str,
    count: usize,
    mine: bool,
) -> Response {
    let p = &theme.palette;
    let fill = if mine {
        p.accent.gamma_multiply(0.35)
    } else {
        p.headerbar_bg
    };
    let border = if mine {
        Stroke::new(1.0_f32, p.accent.gamma_multiply(0.7))
    } else {
        Stroke::new(1.0_f32, p.border_soft)
    };
    let icon_size = (theme.type_scale.caption * 1.25).max(16.0);

    let inner = egui::Frame::new()
        .fill(fill)
        .stroke(border)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                emoji_icon(ui, theme, emoji, icon_size);
                if count > 1 {
                    ui.label(
                        egui::RichText::new(count.to_string())
                            .size(theme.type_scale.caption)
                            .color(p.text),
                    );
                }
            });
        });
    inner.response
}

// ── texture cache ───────────────────────────────────────────────────────────

fn load_emoji_texture(ctx: &egui::Context, emoji: &str) -> Option<TextureHandle> {
    let key = twemoji_key_candidates(emoji).into_iter().next()?;
    // Cache by resolved pack key so ❤️ and ❤ share a texture when they hit the
    // same file; we re-resolve via png bytes identity below.
    let png = twemoji_png(emoji)?;
    // Stable id from the actual file we loaded (content address via key candidates).
    let file_key = twemoji_key_candidates(emoji)
        .into_iter()
        .find(|k| twemoji_pack().contains_key(&format!("{k}.png")))
        .unwrap_or(key);
    let cache_id = Id::new(("vidya/twemoji", file_key.as_str()));

    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(cache_id)) {
        return Some(tex);
    }

    let image = decode_png_rgba(png)?;
    let handle = ctx.load_texture(
        format!("vidya/twemoji/{file_key}"),
        image,
        TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(cache_id, handle.clone()));
    Some(handle)
}

fn decode_png_rgba(bytes: &[u8]) -> Option<ColorImage> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let w = info.width as usize;
    let h = info.height as usize;
    let raw = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(w * h * 4);
            for chunk in raw.chunks_exact(3) {
                out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        _ => return None,
    };
    if rgba.len() != w * h * 4 {
        return None;
    }
    Some(ColorImage::from_rgba_unmultiplied([w, h], &rgba))
}

fn paint_stroke_icon(ui: &mut Ui, color: Color32, icon: Icon, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_icon(ui.painter(), rect, icon, color);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_is_populated() {
        let n = emoji_pack_len();
        assert!(n > 3000, "expected full Twemoji set, got {n}");
    }

    #[test]
    fn keys_match_twemoji_filenames() {
        assert_eq!(twemoji_key("👍"), "1f44d");
        assert_eq!(twemoji_key("❤️"), "2764"); // VS16 stripped (no ZWJ)
        assert_eq!(twemoji_key("😂"), "1f602");
        assert_eq!(twemoji_key("🚀"), "1f680");
        // ZWJ sequence keeps fe0f when present in input
        let technologist = "👨\u{200D}💻";
        assert_eq!(twemoji_key(technologist), "1f468-200d-1f4bb");
    }

    #[test]
    fn resolves_common_and_rare() {
        for e in [
            "👍", "❤️", "❤", "😂", "😮", "😢", "👎", "🚀", "🎉", "🔥", "✨", "👀", "💯", "🙏",
            "😎", "🤯", "🏳️", "🏴",
        ] {
            assert!(
                has_emoji_icon(e),
                "missing Twemoji for {e:?} key={:?}",
                twemoji_key_candidates(e)
            );
        }
    }

    #[test]
    fn skin_tone_falls_back_to_base() {
        // 👍🏻 → base 👍 asset
        assert!(has_emoji_icon("👍🏻"));
    }

    #[test]
    fn zwj_sequence() {
        assert!(has_emoji_icon("👨‍💻"), "technologist ZWJ");
    }

    #[test]
    fn unknown_text_has_no_icon() {
        assert!(!has_emoji_icon("hello"));
        assert!(!has_emoji_icon(""));
    }

    #[test]
    fn icon_shortcuts() {
        assert_eq!(icon_for_emoji("👍"), Some(Icon::ThumbsUp));
        assert_eq!(icon_for_emoji("❤️"), Some(Icon::Heart));
    }

    #[test]
    fn pngs_decode() {
        for e in ["👍", "❤️", "😂", "🚀", "👨‍💻"] {
            let png = twemoji_png(e).unwrap_or_else(|| panic!("no png for {e}"));
            let img = decode_png_rgba(png).unwrap_or_else(|| panic!("decode {e}"));
            assert_eq!(img.size, [72, 72]);
        }
    }
}
