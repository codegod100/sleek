//! URL detection and media/link-preview helpers for chat messages.
//!
//! Mirrors freeq-app / freeq-macos patterns: image URLs show as inline thumbs;
//! video URLs (`.mp4`/`.webm`/…) get an inline video card; other http(s) links
//! get an Open Graph card (title/description/thumb).

use freeq_sdk::media::{LinkPreview, MediaAttachment};
use std::collections::HashMap;

/// Max image download size (bytes).
pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// Chat video preview download cap (muted H.264 MP4 decode in vidya).
pub const MAX_VIDEO_BYTES: usize = 25 * 1024 * 1024;
/// Downscale long edge for chat thumbs (pixels).
pub const MAX_IMAGE_DIM: u32 = 720;

/// What kind of embed a message body should show under the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Embed {
    /// Direct image URL (or IRCv3 media attachment).
    Image { url: String },
    /// Direct video URL (or IRCv3 `video/*` media attachment).
    Video { url: String },
    /// Non-media http(s) link — fetch Open Graph metadata.
    Link { url: String },
}

/// Prefer IRCv3 media/link-preview tags; fall back to scanning message text.
pub fn embed_for_message(text: &str, tags: &HashMap<String, String>) -> Option<Embed> {
    if let Some(media) = MediaAttachment::from_tags(tags) {
        if media.is_image() {
            return Some(Embed::Image { url: media.url });
        }
        if media.is_video() {
            return Some(Embed::Video { url: media.url });
        }
        // Audio / other media still gets a link card on the media URL.
        return Some(Embed::Link { url: media.url });
    }
    if let Some(lp) = LinkPreview::from_tags(tags) {
        return Some(Embed::Link { url: lp.url });
    }
    embed_from_text(text)
}

/// Scan plain message text for the first image, video, or generic link.
pub fn embed_from_text(text: &str) -> Option<Embed> {
    let mut first_video: Option<String> = None;
    let mut first_link: Option<String> = None;
    for url in extract_urls(text) {
        if is_image_url(&url) {
            return Some(Embed::Image { url });
        }
        if is_video_url(&url) {
            if first_video.is_none() {
                first_video = Some(url);
            }
            continue;
        }
        if first_link.is_none() {
            first_link = Some(url);
        }
    }
    first_video
        .map(|url| Embed::Video { url })
        .or_else(|| first_link.map(|url| Embed::Link { url }))
}

/// A http(s) URL found in free-form text, with its byte range in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlSpan {
    /// Inclusive start byte index in the source text.
    pub start: usize,
    /// Exclusive end byte index (cleaned URL; trailing punct excluded).
    pub end: usize,
    pub url: String,
}

/// Pull http(s) URLs from free-form text (stops at whitespace / common trailers).
pub fn extract_urls(text: &str) -> Vec<String> {
    extract_url_spans(text).into_iter().map(|s| s.url).collect()
}

/// Like [`extract_urls`], but also returns byte ranges for linkifying UI text.
pub fn extract_url_spans(text: &str) -> Vec<UrlSpan> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let lower = rest.to_ascii_lowercase();
        let rel = if let Some(p) = lower.find("https://") {
            p
        } else if let Some(p) = lower.find("http://") {
            p
        } else {
            break;
        };
        let start = i + rel;
        let after = &text[start..];
        let end_rel = after
            .find(|c: char| c.is_whitespace() || "<>\"'`".contains(c))
            .unwrap_or(after.len());
        let mut url = after[..end_rel].to_string();
        // Strip trailing punctuation common in chat prose.
        while url
            .chars()
            .last()
            .is_some_and(|c| matches!(c, '.' | ',' | ';' | ')' | ']' | '}' | '!' | '?' | ':'))
        {
            url.pop();
        }
        if url.starts_with("http://") || url.starts_with("https://") {
            let end = start + url.len();
            out.push(UrlSpan {
                start,
                end,
                url,
            });
        }
        i = start + end_rel.max(1);
    }
    out
}

/// True for URLs that look like images (extension, bsky CDN, freeq media path).
pub fn is_image_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    // Bluesky CDN always serves images.
    if lower.contains("cdn.bsky.app/img/") {
        return true;
    }
    // freeq private media: `/api/v1/media/{id}/{sig}/{filename}`
    if let Some(path) = url_path(&lower) {
        if path.contains("/api/v1/media/") {
            if let Some(name) = path.rsplit('/').next() {
                if has_image_ext(name) {
                    return true;
                }
            }
        }
    }
    // Path or query-stripped path ends with image extension.
    let path = url_path(&lower).unwrap_or(lower.as_str());
    let file = path.rsplit('/').next().unwrap_or(path);
    has_image_ext(file)
}

/// True for URLs that look like videos (extension, freeq media path, blob proxy mime).
///
/// Mirrors freeq-app `VIDEO_URL_RE` / `PROXY_VIDEO_RE` and freeq-macos video
/// extensions (`.mp4` / `.m4v` / `.mov` / `.webm`).
pub fn is_video_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    // freeq blob proxy with an explicit video MIME hint.
    if lower.contains("/api/v1/blob?") && lower.contains("mime=video%2f") {
        return true;
    }
    // freeq private media: `/api/v1/media/{id}/{sig}/{filename}`
    if let Some(path) = url_path(&lower) {
        if path.contains("/api/v1/media/") {
            if let Some(name) = path.rsplit('/').next() {
                if has_video_ext(name) {
                    return true;
                }
            }
        }
    }
    let path = url_path(&lower).unwrap_or(lower.as_str());
    let file = path.rsplit('/').next().unwrap_or(path);
    has_video_ext(file)
}

fn has_image_ext(name: &str) -> bool {
    // Strip query fragment already handled; allow `foo.png` or `foo.png.1` rarely —
    // stick to standard suffixes.
    let base = name.split('?').next().unwrap_or(name);
    let base = base.split('#').next().unwrap_or(base);
    base.ends_with(".png")
        || base.ends_with(".jpg")
        || base.ends_with(".jpeg")
        || base.ends_with(".gif")
        || base.ends_with(".webp")
        || base.ends_with(".bmp")
}

fn has_video_ext(name: &str) -> bool {
    let base = name.split('?').next().unwrap_or(name);
    let base = base.split('#').next().unwrap_or(base);
    base.ends_with(".mp4")
        || base.ends_with(".m4v")
        || base.ends_with(".mov")
        || base.ends_with(".webm")
}

/// Filename (or last path segment) for video/media cards.
pub fn display_filename(url: &str) -> String {
    let path = display_path(url);
    if path.is_empty() {
        return display_host(url);
    }
    let name = path.rsplit('/').next().unwrap_or(path.as_str());
    if name.is_empty() || name == "…" {
        display_host(url)
    } else {
        name.to_string()
    }
}

fn url_path(url: &str) -> Option<&str> {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let path_start = after_scheme.find('/')?;
    Some(&after_scheme[path_start..])
}

/// Hostname for display (strip leading `www.`).
pub fn display_host(url: &str) -> String {
    let after = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = after.split('/').next().unwrap_or(after);
    let host = host.split('@').next_back().unwrap_or(host); // userinfo
    let host = host.split(':').next().unwrap_or(host); // port
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Short path for link cards.
pub fn display_path(url: &str) -> String {
    let after = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let path = after.find('/').map(|i| &after[i..]).unwrap_or("");
    let path = path.split('?').next().unwrap_or(path);
    let path = path.split('#').next().unwrap_or(path);
    if path.is_empty() || path == "/" {
        return String::new();
    }
    if path.chars().count() > 48 {
        format!("{}…", path.chars().take(48).collect::<String>())
    } else {
        path.to_string()
    }
}

/// OG metadata already present on the message (from tags), if any.
pub fn link_preview_from_tags(tags: &HashMap<String, String>) -> Option<LinkPreview> {
    LinkPreview::from_tags(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_urls_and_strips_punct() {
        let u = extract_urls("see https://example.com/a.png, and http://x.test/y");
        assert_eq!(u.len(), 2);
        assert_eq!(u[0], "https://example.com/a.png");
        assert_eq!(u[1], "http://x.test/y");
    }

    #[test]
    fn extract_url_spans_ranges_match_source() {
        let text = "see https://example.com/a.png, please";
        let spans = extract_url_spans(text);
        assert_eq!(spans.len(), 1);
        assert_eq!(&text[spans[0].start..spans[0].end], spans[0].url);
        assert_eq!(spans[0].url, "https://example.com/a.png");
    }

    #[test]
    fn detects_image_urls() {
        assert!(is_image_url("https://x.com/cat.JPG?w=1"));
        assert!(is_image_url(
            "https://cdn.bsky.app/img/feed_thumbnail/plain/did:plc:x/bafy"
        ));
        assert!(is_image_url(
            "https://irc.freeq.at/api/v1/media/abc/sig/paste.png"
        ));
        assert!(!is_image_url("https://example.com/page"));
        assert!(!is_image_url("https://ex.com/clip.mp4"));
    }

    #[test]
    fn detects_video_urls() {
        assert!(is_video_url("https://ex.com/clip.MP4?x=1"));
        assert!(is_video_url("https://ex.com/a.webm"));
        assert!(is_video_url("https://ex.com/a.mov"));
        assert!(is_video_url("https://ex.com/a.m4v"));
        assert!(is_video_url(
            "https://irc.freeq.at/api/v1/media/abc/sig/paste.mp4"
        ));
        assert!(is_video_url(
            "https://irc.freeq.at/api/v1/blob?url=https%3A%2F%2Fx&mime=video%2Fmp4"
        ));
        assert!(!is_video_url("https://example.com/page"));
        assert!(!is_video_url("https://ex.com/a.png"));
    }

    #[test]
    fn prefers_image_over_link() {
        match embed_from_text("hi https://ex.com/a.png and https://ex.com/doc") {
            Some(Embed::Image { url }) => assert!(url.ends_with("a.png")),
            other => panic!("expected image, got {other:?}"),
        }
    }

    #[test]
    fn prefers_video_over_generic_link() {
        match embed_from_text("hi https://ex.com/clip.mp4 and https://ex.com/doc") {
            Some(Embed::Video { url }) => assert!(url.ends_with("clip.mp4")),
            other => panic!("expected video, got {other:?}"),
        }
    }

    #[test]
    fn prefers_image_over_video() {
        match embed_from_text("https://ex.com/a.png https://ex.com/a.mp4") {
            Some(Embed::Image { url }) => assert!(url.ends_with("a.png")),
            other => panic!("expected image, got {other:?}"),
        }
    }

    #[test]
    fn media_attachment_video_tag() {
        let mut tags = HashMap::new();
        tags.insert("content-type".into(), "video/mp4".into());
        tags.insert("media-url".into(), "https://ex.com/v.mp4".into());
        match embed_for_message("https://ex.com/v.mp4", &tags) {
            Some(Embed::Video { url }) => assert_eq!(url, "https://ex.com/v.mp4"),
            other => panic!("expected video, got {other:?}"),
        }
    }
}
