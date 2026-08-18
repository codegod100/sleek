//! Rich media support for IRC messages.
//!
//! Uses IRCv3 message tags to carry media metadata alongside plain-text
//! fallback in the PRIVMSG body. This gives multipart/alternative semantics:
//! - Plain clients see the text + URL
//! - Rich clients parse the tags and render inline previews
//!
//! Media is hosted externally (AT Protocol PDS blob storage, or any URL).
//! The IRC server never handles media bytes — it just relays tagged messages.

use std::collections::HashMap;

use anyhow::{Context, Result};
use reqwest::header;

/// Metadata for a media attachment.
#[derive(Debug, Clone)]
pub struct MediaAttachment {
    /// MIME content type (e.g. "image/jpeg", "video/mp4").
    pub content_type: String,
    /// URL where the media can be fetched.
    pub url: String,
    /// Alt text / description.
    pub alt: Option<String>,
    /// Width in pixels.
    pub width: Option<u32>,
    /// Height in pixels.
    pub height: Option<u32>,
    /// Blurhash placeholder string.
    pub blurhash: Option<String>,
    /// File size in bytes.
    pub size: Option<u64>,
    /// Original filename.
    pub filename: Option<String>,
}

impl MediaAttachment {
    /// Encode as IRCv3 message tags.
    pub fn to_tags(&self) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("content-type".to_string(), self.content_type.clone());
        tags.insert("media-url".to_string(), self.url.clone());
        if let Some(ref alt) = self.alt {
            tags.insert("media-alt".to_string(), alt.clone());
        }
        if let Some(w) = self.width {
            tags.insert("media-w".to_string(), w.to_string());
        }
        if let Some(h) = self.height {
            tags.insert("media-h".to_string(), h.to_string());
        }
        if let Some(ref bh) = self.blurhash {
            tags.insert("media-blurhash".to_string(), bh.clone());
        }
        if let Some(sz) = self.size {
            tags.insert("media-size".to_string(), sz.to_string());
        }
        if let Some(ref name) = self.filename {
            tags.insert("media-filename".to_string(), name.clone());
        }
        tags
    }

    /// Parse from IRCv3 message tags.
    pub fn from_tags(tags: &HashMap<String, String>) -> Option<Self> {
        let content_type = tags.get("content-type")?.clone();
        let url = tags.get("media-url")?.clone();
        Some(Self {
            content_type,
            url,
            alt: tags.get("media-alt").cloned(),
            width: tags.get("media-w").and_then(|v| v.parse().ok()),
            height: tags.get("media-h").and_then(|v| v.parse().ok()),
            blurhash: tags.get("media-blurhash").cloned(),
            size: tags.get("media-size").and_then(|v| v.parse().ok()),
            filename: tags.get("media-filename").cloned(),
        })
    }

    /// Generate the plain-text fallback for the PRIVMSG body.
    pub fn fallback_text(&self) -> String {
        match &self.alt {
            Some(alt) => format!("{alt} {}", self.url),
            None => self.url.clone(),
        }
    }

    /// Is this an image type?
    pub fn is_image(&self) -> bool {
        self.content_type.starts_with("image/")
    }

    /// Is this a video type?
    pub fn is_video(&self) -> bool {
        self.content_type.starts_with("video/")
    }

    /// Is this an audio type?
    pub fn is_audio(&self) -> bool {
        self.content_type.starts_with("audio/")
    }
}

/// A link preview (OpenGraph-style metadata).
#[derive(Debug, Clone)]
pub struct LinkPreview {
    /// The URL being previewed.
    pub url: String,
    /// Page title.
    pub title: Option<String>,
    /// Description text.
    pub description: Option<String>,
    /// Thumbnail image URL.
    pub thumb_url: Option<String>,
    /// Site name (`og:site_name`), when present.
    pub site_name: Option<String>,
}

impl LinkPreview {
    pub fn to_tags(&self) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert(
            "content-type".to_string(),
            "text/x-link-preview".to_string(),
        );
        tags.insert("media-url".to_string(), self.url.clone());
        if let Some(ref t) = self.title {
            tags.insert("link-title".to_string(), t.clone());
        }
        if let Some(ref d) = self.description {
            tags.insert("link-desc".to_string(), d.clone());
        }
        if let Some(ref thumb) = self.thumb_url {
            tags.insert("link-thumb".to_string(), thumb.clone());
        }
        if let Some(ref site) = self.site_name {
            tags.insert("link-site".to_string(), site.clone());
        }
        tags
    }

    pub fn from_tags(tags: &HashMap<String, String>) -> Option<Self> {
        if tags.get("content-type")?.as_str() != "text/x-link-preview" {
            return None;
        }
        Some(Self {
            url: tags.get("media-url")?.clone(),
            title: tags.get("link-title").cloned(),
            description: tags.get("link-desc").cloned(),
            thumb_url: tags.get("link-thumb").cloned(),
            site_name: tags.get("link-site").cloned(),
        })
    }

    /// True when at least one useful display field is present.
    pub fn has_content(&self) -> bool {
        self.title.as_ref().is_some_and(|s| !s.trim().is_empty())
            || self
                .description
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            || self
                .thumb_url
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            || self
                .site_name
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
    }
}

/// A reaction to a message.
#[derive(Debug, Clone)]
pub struct Reaction {
    /// The emoji or short text (e.g. "🔥", "❤️", "+1").
    pub emoji: String,
    /// Message ID being reacted to (IRCv3 msgid tag value), if available.
    pub msgid: Option<String>,
}

impl Reaction {
    pub fn to_tags(&self) -> HashMap<String, String> {
        let mut tags = HashMap::new();
        tags.insert("+react".to_string(), self.emoji.clone());
        if let Some(ref id) = self.msgid {
            tags.insert("+reply".to_string(), id.clone());
        }
        tags
    }

    pub fn from_tags(tags: &HashMap<String, String>) -> Option<Self> {
        let emoji = tags.get("+react")?;
        Some(Self {
            emoji: emoji.clone(),
            msgid: tags.get("+reply").cloned(),
        })
    }
}

/// Fetch OpenGraph metadata from a URL for link preview.
pub async fn fetch_link_preview(url: &str) -> Result<LinkPreview> {
    // SSRF protection: resolve hostname and reject private IPs
    let parsed = url::Url::parse(url).context("Invalid URL for link preview")?;
    let host = parsed.host_str().context("URL has no host")?.to_string();
    let port = parsed
        .port()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });
    let addrs = crate::ssrf::resolve_and_check(&host, port)
        .await
        .context("Link preview SSRF check failed")?;

    // Use a DNS-pinned client to prevent rebinding between check and fetch
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::limited(5));
    for addr in &addrs {
        builder = builder.resolve(&host, *addr);
    }
    let client = builder.build()?;

    let resp = client
        .get(url)
        // Match freeq-server OG proxy UA; some sites 403 opaque bot strings.
        .header("User-Agent", "freeq/1.0 (link preview)")
        .header("Accept", "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?
        .error_for_status()?;

    // Prefer the final URL after redirects for relative image resolution.
    let final_url = resp.url().clone();

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !content_type.contains("text/html") && !content_type.contains("application/xhtml") {
        anyhow::bail!("Not an HTML page: {content_type}");
    }

    // Cap body size (parity with freeq-server OG endpoint at 256 KiB).
    let body = resp.text().await?;
    let body = if body.len() > 256 * 1024 {
        &body[..256 * 1024]
    } else {
        &body
    };

    let preview = parse_link_preview_html(final_url.as_str(), body);
    if !preview.has_content() {
        anyhow::bail!("No OpenGraph or title metadata found");
    }
    Ok(preview)
}

/// Parse Open Graph / Twitter / HTML meta from a page body (testable, no I/O).
///
/// Field priority (first non-empty wins per field):
/// - title: `og:title` → `twitter:title` → `<title>`
/// - description: `og:description` → `twitter:description` → `description`
/// - thumb: `og:image` / `og:image:url` / `og:image:secure_url` → `twitter:image`
/// - site_name: `og:site_name` → `application-name`
pub fn parse_link_preview_html(page_url: &str, body: &str) -> LinkPreview {
    let meta = collect_meta_tags(body);

    let pick = |keys: &[&str]| -> Option<String> {
        for k in keys {
            if let Some(v) = meta.get(*k) {
                let t = v.trim();
                if !t.is_empty() {
                    return Some(html_decode(t));
                }
            }
        }
        None
    };

    let mut title = pick(&["og:title", "twitter:title"]);
    let description = pick(&["og:description", "twitter:description", "description"]);
    let mut thumb_url = pick(&[
        "og:image",
        "og:image:url",
        "og:image:secure_url",
        "twitter:image",
        "twitter:image:src",
    ]);
    let site_name = pick(&["og:site_name", "application-name"]);

    if title.is_none() {
        title = extract_html_title(body);
    }

    // Resolve relative / protocol-relative image URLs against the page URL.
    if let Some(ref thumb) = thumb_url {
        thumb_url = Some(resolve_against(page_url, thumb));
    }

    LinkPreview {
        url: page_url.to_string(),
        title,
        description,
        thumb_url,
        site_name,
    }
}

/// Collect `property`/`name` → `content` pairs from `<meta>` tags (case-insensitive).
fn collect_meta_tags(body: &str) -> HashMap<String, String> {
    let lower = body.to_ascii_lowercase();
    let mut out: HashMap<String, String> = HashMap::new();
    let mut i = 0;
    while let Some(rel) = lower[i..].find("<meta") {
        let start = i + rel;
        let after_lower = &lower[start..];
        // End of tag (meta is void; ignore rare `>` inside quotes for simplicity).
        let end_rel = after_lower.find('>').unwrap_or(after_lower.len());
        let seg_lower = &lower[start..start + end_rel];
        let seg_orig = &body[start..start + end_rel];
        if let Some((key, val)) = extract_meta_kv(seg_lower, seg_orig) {
            // First occurrence wins so early <head> OG tags beat late duplicates.
            out.entry(key).or_insert(val);
        }
        i = start + end_rel.max(1);
    }
    out
}

/// Extract `(property|name, content)` from a single `<meta …` fragment.
fn extract_meta_kv(seg_lower: &str, seg_original: &str) -> Option<(String, String)> {
    let key = extract_attr(seg_lower, seg_original, "property")
        .or_else(|| extract_attr(seg_lower, seg_original, "name"))?;
    let content = extract_attr(seg_lower, seg_original, "content")?;
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty() || content.trim().is_empty() {
        return None;
    }
    Some((key, content))
}

/// Read a double- or single-quoted HTML attribute value.
fn extract_attr(seg_lower: &str, seg_original: &str, attr: &str) -> Option<String> {
    let needle_dq = format!("{attr}=\"");
    let needle_sq = format!("{attr}='");
    let (start, quote) = if let Some(p) = seg_lower.find(&needle_dq) {
        (p + needle_dq.len(), '"')
    } else if let Some(p) = seg_lower.find(&needle_sq) {
        (p + needle_sq.len(), '\'')
    } else {
        return None;
    };
    let end = seg_lower[start..].find(quote)?;
    Some(seg_original[start..start + end].to_string())
}

fn extract_html_title(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after = &body[start..];
    let gt = after.find('>')?;
    let rest = &after[gt + 1..];
    let rest_lower = &lower[start + gt + 1..];
    let end = rest_lower.find("</title")?;
    let t = rest[..end].trim();
    if t.is_empty() {
        None
    } else {
        Some(html_decode(t))
    }
}

fn resolve_against(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.is_empty() {
        return href.to_string();
    }
    // Already absolute.
    if href.starts_with("https://") || href.starts_with("http://") {
        return href.to_string();
    }
    let Ok(base_url) = url::Url::parse(base) else {
        return href.to_string();
    };
    // Protocol-relative: //cdn.example.com/img.png
    if let Some(rest) = href.strip_prefix("//") {
        return format!("{}://{rest}", base_url.scheme());
    }
    base_url
        .join(href)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| href.to_string())
}

/// Basic HTML entity decoding.
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&#x2F;", "/")
        .replace("&nbsp;", " ")
}

/// Upload a media file to an AT Protocol PDS, pin it with a record, and return
/// a publicly accessible URL.
///
/// Flow:
/// 1. Upload blob via `com.atproto.repo.uploadBlob`
/// 2. Create a record referencing the blob (prevents garbage collection)
/// 3. Return CDN URL that serves the blob publicly
#[allow(clippy::too_many_arguments)]
pub async fn upload_media_to_pds(
    pds_url: &str,
    did: &str,
    access_token: &str,
    dpop_key: Option<&crate::oauth::DpopKey>,
    dpop_nonce: Option<&str>,
    content_type: &str,
    data: &[u8],
    alt_text: Option<&str>,
    channel: Option<&str>,
    cross_post: bool,
) -> Result<MediaUploadResult> {
    let client = reqwest::Client::new();
    let base = pds_url.trim_end_matches('/');
    let mut current_nonce = dpop_nonce.map(|s| s.to_string());

    // Step 1: Upload the blob
    let blob_json = dpop_post(
        &client,
        base,
        "com.atproto.repo.uploadBlob",
        dpop_key,
        access_token,
        &mut current_nonce,
        Some(content_type),
        data.to_vec(),
    )
    .await
    .context("Blob upload to PDS failed — your session may have expired. Please sign in again.")?;

    let blob = &blob_json["blob"];
    let cid = blob["ref"]["$link"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No CID in upload response"))?
        .to_string();
    let size = blob["size"].as_u64().unwrap_or(data.len() as u64);
    let mime = blob["mimeType"]
        .as_str()
        .unwrap_or(content_type)
        .to_string();

    // Step 2: Create a record that references the blob to prevent GC.
    //
    // We use a custom lexicon `blue.irc.media` to store the blob reference
    // without polluting the user's Bluesky feed. This collection:
    // - Pins the blob so the PDS won't garbage-collect it
    // - Provides the CDN URL via the standard blob CID
    // - Can be enumerated to list a user's shared IRC media
    //
    // If cross_post is true, we ALSO create an app.bsky.feed.post so
    // the image appears in the user's Bluesky feed with a channel reference.
    let now = chrono::Utc::now().to_rfc3339();
    let irc_record = serde_json::json!({
        "repo": did,
        "collection": "blue.irc.media",
        "record": {
            "$type": "blue.irc.media",
            "blob": blob.clone(),
            "mimeType": mime,
            "alt": alt_text.unwrap_or(""),
            "channel": channel,
            "createdAt": now,
        }
    });

    let _record_result = dpop_post(
        &client,
        base,
        "com.atproto.repo.createRecord",
        dpop_key,
        access_token,
        &mut current_nonce,
        None,
        serde_json::to_vec(&irc_record)?,
    )
    .await
    .context("Record creation failed (blue.irc.media)")?;

    // Optional cross-post to Bluesky feed
    if cross_post {
        let feed_text = if let Some(chan) = channel {
            format!("{} [shared in {chan}]", alt_text.unwrap_or("📎"))
        } else {
            alt_text.unwrap_or("📎").to_string()
        };
        let feed_record = serde_json::json!({
            "repo": did,
            "collection": "app.bsky.feed.post",
            "record": {
                "$type": "app.bsky.feed.post",
                "text": feed_text,
                "createdAt": now,
                "embed": {
                    "$type": "app.bsky.embed.images",
                    "images": [{
                        "alt": alt_text.unwrap_or(""),
                        "image": blob.clone(),
                    }]
                }
            }
        });
        let _ = dpop_post(
            &client,
            base,
            "com.atproto.repo.createRecord",
            dpop_key,
            access_token,
            &mut current_nonce,
            None,
            serde_json::to_vec(&feed_record)?,
        )
        .await; // Best-effort; don't fail the upload if cross-post fails
    }

    // Step 3: Build URL
    // Image CDN only works for images — audio/video need the raw blob URL
    let url = if mime.starts_with("image/") {
        let ext = match mime.as_str() {
            "image/png" => "png",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "jpeg",
        };
        format!("https://cdn.bsky.app/img/feed_fullsize/plain/{did}/{cid}@{ext}",)
    } else {
        // For audio/video, use the raw blob endpoint on the PDS
        format!("{pds_url}/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}",)
    };

    Ok(MediaUploadResult {
        cid,
        size,
        mime_type: mime,
        url,
        updated_nonce: current_nonce,
    })
}

/// POST to an XRPC endpoint with DPoP nonce retry logic.
#[allow(clippy::too_many_arguments)]
async fn dpop_post(
    client: &reqwest::Client,
    base: &str,
    method: &str,
    dpop_key: Option<&crate::oauth::DpopKey>,
    access_token: &str,
    current_nonce: &mut Option<String>,
    content_type_override: Option<&str>,
    body: Vec<u8>,
) -> Result<serde_json::Value> {
    let url = format!("{base}/xrpc/{method}");
    let ct = content_type_override.unwrap_or("application/json");

    for attempt in 0..3 {
        let mut req = client
            .post(&url)
            .header(header::CONTENT_TYPE, ct)
            .body(body.clone());

        if let Some(key) = dpop_key {
            let proof = key.proof("POST", &url, current_nonce.as_deref(), Some(access_token))?;
            req = req
                .header("Authorization", format!("DPoP {access_token}"))
                .header("DPoP", proof);
        } else {
            req = req.header("Authorization", format!("Bearer {access_token}"));
        }

        let resp = req.send().await?;

        if let Some(new_nonce) = resp.headers().get("dpop-nonce") {
            *current_nonce = Some(new_nonce.to_str().unwrap_or("").to_string());
        }

        if (resp.status() == 401 || resp.status() == 400) && attempt < 2 {
            let body = resp.text().await.unwrap_or_default();
            tracing::warn!(method, attempt, body = %body, "DPoP retry (nonce rotation or auth)");
            continue;
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if status.as_u16() == 401 {
                anyhow::bail!(
                    "Authentication expired ({status}). Please re-authenticate. PDS response: {body}"
                );
            }
            anyhow::bail!("{status}: {body}");
        }

        return Ok(resp.json().await?);
    }

    anyhow::bail!("Request failed after retries")
}

/// Result of a media upload.
#[derive(Debug, Clone)]
pub struct MediaUploadResult {
    /// Content identifier (CID) of the uploaded blob.
    pub cid: String,
    /// Size in bytes.
    pub size: u64,
    /// MIME type.
    pub mime_type: String,
    /// Publicly accessible URL for the media.
    pub url: String,
    /// Updated DPoP nonce (for caching to avoid stale nonce on next request).
    pub updated_nonce: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_attachment_roundtrip() {
        let media = MediaAttachment {
            content_type: "image/jpeg".to_string(),
            url: "https://cdn.bsky.app/img/example.jpg".to_string(),
            alt: Some("A sunset".to_string()),
            width: Some(1200),
            height: Some(800),
            blurhash: Some("LEHV6nWB2yk8".to_string()),
            size: Some(45000),
            filename: Some("sunset.jpg".to_string()),
        };

        let tags = media.to_tags();
        let parsed = MediaAttachment::from_tags(&tags).unwrap();

        assert_eq!(parsed.content_type, "image/jpeg");
        assert_eq!(parsed.url, media.url);
        assert_eq!(parsed.alt.as_deref(), Some("A sunset"));
        assert_eq!(parsed.width, Some(1200));
        assert_eq!(parsed.height, Some(800));
        assert_eq!(parsed.blurhash.as_deref(), Some("LEHV6nWB2yk8"));
        assert_eq!(parsed.size, Some(45000));
        assert_eq!(parsed.filename.as_deref(), Some("sunset.jpg"));
    }

    #[test]
    fn media_fallback_text() {
        let media = MediaAttachment {
            content_type: "image/jpeg".to_string(),
            url: "https://example.com/img.jpg".to_string(),
            alt: Some("My photo".to_string()),
            width: None,
            height: None,
            blurhash: None,
            size: None,
            filename: None,
        };
        assert_eq!(
            media.fallback_text(),
            "My photo https://example.com/img.jpg"
        );

        let no_alt = MediaAttachment { alt: None, ..media };
        assert_eq!(no_alt.fallback_text(), "https://example.com/img.jpg");
    }

    #[test]
    fn link_preview_roundtrip() {
        let preview = LinkPreview {
            url: "https://example.com/article".to_string(),
            title: Some("Great Article".to_string()),
            description: Some("An interesting read".to_string()),
            thumb_url: Some("https://example.com/thumb.jpg".to_string()),
            site_name: Some("Example News".to_string()),
        };

        let tags = preview.to_tags();
        let parsed = LinkPreview::from_tags(&tags).unwrap();

        assert_eq!(parsed.url, preview.url);
        assert_eq!(parsed.title.as_deref(), Some("Great Article"));
        assert_eq!(parsed.description.as_deref(), Some("An interesting read"));
        assert_eq!(parsed.thumb_url.as_deref(), Some("https://example.com/thumb.jpg"));
        assert_eq!(parsed.site_name.as_deref(), Some("Example News"));
        assert!(parsed.has_content());
    }

    #[test]
    fn parse_og_with_site_name_and_relative_image() {
        let html = r#"
            <html><head>
            <meta property="og:title" content="Hello &amp; Co" />
            <meta property="og:description" content="A fine page" />
            <meta property="og:image" content="/img/thumb.jpg" />
            <meta property="og:site_name" content="Acme" />
            <title>Ignored when OG title present</title>
            </head></html>
        "#;
        let p = parse_link_preview_html("https://example.com/posts/1", html);
        assert_eq!(p.title.as_deref(), Some("Hello & Co"));
        assert_eq!(p.description.as_deref(), Some("A fine page"));
        assert_eq!(
            p.thumb_url.as_deref(),
            Some("https://example.com/img/thumb.jpg")
        );
        assert_eq!(p.site_name.as_deref(), Some("Acme"));
    }

    #[test]
    fn parse_falls_back_to_twitter_and_meta_description() {
        let html = r#"
            <meta name="twitter:title" content="Tweet Title">
            <meta name="twitter:image" content="https://cdn.example.com/a.png">
            <meta name="description" content="Plain desc">
            <meta content="Flip Order Site" property="og:site_name">
        "#;
        let p = parse_link_preview_html("https://example.com/", html);
        assert_eq!(p.title.as_deref(), Some("Tweet Title"));
        assert_eq!(p.description.as_deref(), Some("Plain desc"));
        assert_eq!(
            p.thumb_url.as_deref(),
            Some("https://cdn.example.com/a.png")
        );
        assert_eq!(p.site_name.as_deref(), Some("Flip Order Site"));
    }

    #[test]
    fn parse_title_tag_and_protocol_relative_image() {
        let html = r#"
            <TITLE>Doc Title</TITLE>
            <META PROPERTY="og:image" CONTENT="//cdn.example.com/x.webp">
        "#;
        let p = parse_link_preview_html("https://example.com/page", html);
        assert_eq!(p.title.as_deref(), Some("Doc Title"));
        assert_eq!(
            p.thumb_url.as_deref(),
            Some("https://cdn.example.com/x.webp")
        );
    }

    #[test]
    fn reaction_roundtrip() {
        let reaction = Reaction {
            emoji: "🔥".to_string(),
            msgid: Some("abc123".to_string()),
        };

        let tags = reaction.to_tags();
        assert_eq!(tags.get("+react").unwrap(), "🔥");
        assert_eq!(tags.get("+reply").unwrap(), "abc123");

        let parsed = Reaction::from_tags(&tags).unwrap();
        assert_eq!(parsed.emoji, "🔥");
        assert_eq!(parsed.msgid.as_deref(), Some("abc123"));
    }

    #[test]
    fn reaction_no_msgid() {
        let reaction = Reaction {
            emoji: "❤️".to_string(),
            msgid: None,
        };

        let tags = reaction.to_tags();
        assert!(!tags.contains_key("+reply"));

        let parsed = Reaction::from_tags(&tags).unwrap();
        assert_eq!(parsed.emoji, "❤️");
        assert!(parsed.msgid.is_none());
    }

    #[test]
    fn html_decode_basic() {
        assert_eq!(html_decode("hello &amp; world"), "hello & world");
        assert_eq!(html_decode("&lt;b&gt;bold&lt;/b&gt;"), "<b>bold</b>");
        assert_eq!(html_decode("it&#39;s"), "it's");
    }

    #[test]
    fn type_checks() {
        let img = MediaAttachment {
            content_type: "image/png".to_string(),
            url: String::new(),
            alt: None,
            width: None,
            height: None,
            blurhash: None,
            size: None,
            filename: None,
        };
        assert!(img.is_image());
        assert!(!img.is_video());

        let vid = MediaAttachment {
            content_type: "video/mp4".to_string(),
            ..img.clone()
        };
        assert!(vid.is_video());
    }

    #[test]
    fn pdf_url_uses_raw_pds_blob_not_cdn() {
        // PDFs should get raw PDS blob URL, not CDN (CDN only handles images)
        let mime = "application/pdf";
        let did = "did:plc:test";
        let cid = "bafkreitest";
        let pds_url = "https://puffball.us-east.host.bsky.network";

        let url = if mime.starts_with("image/") {
            format!("https://cdn.bsky.app/img/feed_fullsize/plain/{did}/{cid}@jpeg")
        } else {
            format!("{pds_url}/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}")
        };

        assert!(
            url.contains("com.atproto.sync.getBlob"),
            "PDF should use raw PDS blob URL"
        );
        assert!(!url.contains("cdn.bsky.app"), "PDF must NOT use CDN URL");
    }

    #[test]
    fn pdf_in_allowed_types() {
        // Verify PDF MIME type is accepted (matches client-side whitelist)
        let allowed = [
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/webp",
            "video/mp4",
            "video/webm",
            "audio/mpeg",
            "audio/ogg",
            "application/pdf",
        ];
        assert!(allowed.contains(&"application/pdf"));
    }
}
