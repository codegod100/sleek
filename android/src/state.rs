//! Application state — channel buffers, connection, and freeq-inspired models.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use eframe::egui::{self, ColorImage, TextureHandle};

use crate::av::{ChannelCall, LocalCall, VideoFrameStore};
use crate::preview::Embed;

/// How long toasts stay visible before auto-dismiss.
const TOAST_DURATION: Duration = Duration::from_secs(4);

/// Max messages retained per buffer.
const MAX_MESSAGES: usize = 500;

/// How long a search-hit message stays highlighted after navigation.
const SEARCH_HIGHLIGHT_DURATION: Duration = Duration::from_secs(2);

/// Cap on message-search hits shown in the overlay.
const MESSAGE_SEARCH_LIMIT: usize = 50;

/// IRC-style Tab nick completion cycle for the chat compose field.
#[derive(Debug, Clone, Default)]
pub struct NickTabComplete {
    /// True while Tab is cycling through matches for the same prefix.
    pub active: bool,
    /// Character index where the completed word starts (includes `@` if any).
    pub word_start: usize,
    /// Matching nicks (display casing, no `@` prefix).
    pub matches: Vec<String>,
    /// Index of the last applied match.
    pub index: usize,
    /// Start-of-line completion uses `nick: `; otherwise `nick `.
    pub leading: bool,
    /// User typed `@` before the nick — keep it while cycling.
    pub had_at: bool,
    /// Cursor character index after the last applied completion.
    pub cursor_end: usize,
}

impl NickTabComplete {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Registered,
}

impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disconnected => "Disconnected",
            Self::Connecting => "Connecting…",
            Self::Connected => "Connected",
            Self::Registered => "Online",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, Self::Connected | Self::Registered)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chats,
    Discover,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Chats, Tab::Discover, Tab::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Chats => "Chats",
            Tab::Discover => "Discover",
            Tab::Settings => "Settings",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Tab::Chats => "Chats",
            Tab::Discover => "Discover",
            Tab::Settings => "Settings",
        }
    }
}

/// Navigation within the connected shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Tabs,
    Chat(String),
}

/// Open Graph fields for a link card (from IRCv3 tags or a live fetch).
#[derive(Debug, Clone, Default)]
pub struct LinkMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub thumb_url: Option<String>,
    /// `og:site_name` (or application-name) when known.
    pub site_name: Option<String>,
}

/// One hit from global in-memory message search (freeq-android SearchSheet).
#[derive(Debug, Clone)]
pub struct MessageSearchHit {
    pub channel: String,
    pub message: ChatMessage,
}

/// Compose-bar reply context (mirrors freeq-android `replyingTo`).
#[derive(Debug, Clone)]
pub struct ReplyTarget {
    pub msgid: String,
    pub from: String,
    pub preview: String,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: String,
    pub from: String,
    pub text: String,
    pub is_system: bool,
    pub is_action: bool,
    pub is_edited: bool,
    pub is_deleted: bool,
    pub timestamp: DateTime<Local>,
    pub reply_to: Option<String>,
    /// Root msgid before any edit chain (delete names this; history dedup).
    pub edit_of: Option<String>,
    pub is_signed: bool,
    /// Inline image / OG link embed (from tags or URL scan).
    pub embed: Option<Embed>,
    /// OG metadata when already known (IRCv3 link-preview tags).
    pub link_meta: Option<LinkMeta>,
    /// Emoji → nicks who reacted (live TAGMSG + CHATHISTORY `+freeq.at/reactions`).
    pub reactions: HashMap<String, HashSet<String>>,
}

impl ChatMessage {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            id: format!("sys-{}", Utc::now().timestamp_millis()),
            from: String::new(),
            text: text.into(),
            is_system: true,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        }
    }

    /// True when `nick` has already reacted with this emoji.
    pub fn has_reaction_from(&self, emoji: &str, nick: &str) -> bool {
        self.reactions
            .get(emoji)
            .is_some_and(|nicks| nicks.iter().any(|n| n.eq_ignore_ascii_case(nick)))
    }

    /// Resolve embed: prefer fields set at receive, else scan body text.
    pub fn resolved_embed(&self) -> Option<Embed> {
        self.embed
            .clone()
            .or_else(|| crate::preview::embed_from_text(&self.text))
    }

    pub fn time_label(&self) -> String {
        // 12-hour clock with AM/PM (e.g. "3:45 PM").
        self.timestamp.format("%-I:%M %p").to_string()
    }

    pub fn preview(&self) -> String {
        if self.is_deleted {
            return "Message deleted".into();
        }
        if self.is_system {
            return self.text.clone();
        }
        if self.is_action {
            return format!("* {} {}", self.from, self.text);
        }
        let body = if let Some(title) = self
            .link_meta
            .as_ref()
            .and_then(|m| m.title.as_ref())
            .filter(|t| !t.trim().is_empty())
        {
            format!("🔗 {title}")
        } else if let Some(embed) = self.resolved_embed() {
            match &embed {
                crate::preview::Embed::Image { url } | crate::preview::Embed::Video { url } => {
                    let caption =
                        crate::preview::caption_without_embed_url(self.text.trim(), url);
                    if caption.is_empty() {
                        if matches!(embed, crate::preview::Embed::Image { .. }) {
                            "📷 Image".into()
                        } else {
                            "🎬 Video".into()
                        }
                    } else {
                        caption
                    }
                }
                crate::preview::Embed::Link { .. } => self.text.clone(),
            }
        } else {
            self.text.clone()
        };
        let body = if body.trim().is_empty() {
            "[message cleared]".into()
        } else {
            body
        };
        if self.from.is_empty() {
            body
        } else {
            format!("{}: {}", self.from, body)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Buffer {
    pub name: String,
    pub messages: VecDeque<ChatMessage>,
    pub members: Vec<String>,
    pub topic: String,
    pub unread: u32,
    pub last_activity: i64,
    pub names_pending: bool,
    /// True while a JOIN is in flight (optimistic open before confirmation).
    pub join_pending: bool,
    /// Server rejected JOIN (e.g. guest on a policy-gated channel like #policytest).
    pub join_error: Option<String>,
    /// Active freeq AV call in this channel (`av-state`), if any.
    pub call: Option<ChannelCall>,
    message_ids: HashSet<String>,
    /// Original msgids rewritten by `+draft/edit` — skip if history replays them.
    edit_aliases: HashSet<String>,
}

impl Buffer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            messages: VecDeque::new(),
            members: Vec::new(),
            topic: String::new(),
            unread: 0,
            last_activity: 0,
            names_pending: false,
            join_pending: false,
            join_error: None,
            call: None,
            message_ids: HashSet::new(),
            edit_aliases: HashSet::new(),
        }
    }

    pub fn is_channel(&self) -> bool {
        self.name.starts_with('#') || self.name.starts_with('&')
    }

    /// Successfully in the channel (not still joining, not denied).
    pub fn is_joined(&self) -> bool {
        if !self.is_channel() {
            return true;
        }
        !self.join_pending && self.join_error.is_none()
    }

    pub fn display_name(&self) -> &str {
        self.name.as_str()
    }

    pub fn is_dm(&self) -> bool {
        !self.is_channel() && self.name != "*status"
    }

    pub fn last_preview(&self) -> String {
        if let Some(err) = &self.join_error {
            return err.clone();
        }
        if let Some(call) = &self.call {
            let n = call.participants.max(1);
            return format!("📞 Call active · {n} in call");
        }
        if self.join_pending && self.messages.is_empty() {
            return "Joining…".into();
        }
        self.messages
            .iter()
            .rev()
            .find(|m| !m.is_system || m.from.is_empty())
            .or_else(|| self.messages.back())
            .map(|m| m.preview())
            .unwrap_or_else(|| "No messages yet".into())
    }

    /// Seed `last_activity` from a CHATHISTORY TARGETS server-time tag so the
    /// chat list sorts correctly before per-DM history backfills.
    pub fn seed_activity_from_target(&mut self, server_time: Option<&str>) {
        let Some(raw) = server_time.filter(|s| !s.is_empty()) else {
            return;
        };
        let Ok(dt) = DateTime::parse_from_rfc3339(raw) else {
            return;
        };
        let ts = dt.timestamp();
        if self.messages.is_empty() || ts > self.last_activity {
            self.last_activity = ts;
        }
    }

    pub fn append(&mut self, msg: ChatMessage) {
        // Pre-edit row already rewritten in place — don't resurrect old text.
        if !msg.id.is_empty() && self.edit_aliases.contains(&msg.id) {
            return;
        }
        if !msg.id.is_empty() && !self.message_ids.insert(msg.id.clone()) {
            return;
        }
        // Server echo of our own send: drop the optimistic local-* row so we
        // don't show the same line twice (local unsigned + signed echo).
        if !msg.is_system && !msg.id.starts_with("local-") {
            self.consume_local_echo(&msg);
        }
        let ts = msg.timestamp.timestamp();
        if !msg.is_system && ts > self.last_activity {
            self.last_activity = ts;
        }
        // Insert chronologically so CHATHISTORY backfill lands above live lines.
        if let Some(back) = self.messages.back() {
            if msg.timestamp < back.timestamp {
                let idx = self
                    .messages
                    .iter()
                    .position(|m| msg.timestamp < m.timestamp)
                    .unwrap_or(self.messages.len());
                self.messages.insert(idx, msg);
            } else {
                self.messages.push_back(msg);
            }
        } else {
            self.messages.push_back(msg);
        }
        while self.messages.len() > MAX_MESSAGES {
            if let Some(old) = self.messages.pop_front() {
                self.message_ids.remove(&old.id);
            }
        }
    }

    /// Apply inbound `+draft/edit`. Rewrites the original in place; swaps `id`
    /// to the edit's msgid so further edits chain. Returns true if applied.
    pub fn apply_edit(
        &mut self,
        editor_nick: &str,
        original_msgid: &str,
        new_msgid: Option<&str>,
        new_text: &str,
        embed: Option<Embed>,
        link_meta: Option<LinkMeta>,
    ) -> bool {
        let (old_id, swap_to) = {
            let Some(msg) = self.find_message_mut(original_msgid) else {
                return false;
            };
            if !msg.from.eq_ignore_ascii_case(editor_nick) {
                return false;
            }
            if msg.is_deleted {
                return false;
            }
            let old_id = msg.id.clone();
            msg.text = new_text.to_string();
            msg.is_edited = true;
            msg.embed = embed;
            msg.link_meta = link_meta;
            if msg.edit_of.is_none() {
                msg.edit_of = Some(original_msgid.to_string());
            }
            let swap_to = new_msgid
                .filter(|id| !id.is_empty() && *id != old_id.as_str())
                .map(str::to_string);
            if let Some(id) = swap_to.as_ref() {
                msg.id = id.clone();
            }
            (old_id, swap_to)
        };
        self.edit_aliases.insert(original_msgid.to_string());
        if old_id != original_msgid {
            self.edit_aliases.insert(old_id.clone());
        }
        if let Some(id) = swap_to {
            self.message_ids.remove(&old_id);
            self.message_ids.insert(id);
        }
        true
    }

    /// Soft-delete via `+draft/delete`. Matches current id or edit-chain root.
    pub fn apply_delete(&mut self, deleter_nick: &str, msgid: &str) -> bool {
        let Some(msg) = self
            .messages
            .iter_mut()
            .rev()
            .find(|m| m.id == msgid || m.edit_of.as_deref() == Some(msgid))
        else {
            return false;
        };
        if !msg.from.eq_ignore_ascii_case(deleter_nick) {
            return false;
        }
        msg.is_deleted = true;
        msg.text.clear();
        msg.embed = None;
        msg.link_meta = None;
        msg.reactions.clear();
        true
    }

    /// Absorb another buffer's messages/unread (DID-keyed DM merge).
    fn absorb(&mut self, other: Buffer) {
        self.unread = self.unread.saturating_add(other.unread);
        if other.last_activity > self.last_activity {
            self.last_activity = other.last_activity;
        }
        for m in other.messages {
            self.append(m);
        }
        if self.topic.is_empty() && !other.topic.is_empty() {
            self.topic = other.topic;
        }
        if self.call.is_none() {
            self.call = other.call;
        }
    }

    /// Remove a pending optimistic echo that matches an incoming server message.
    fn consume_local_echo(&mut self, server_msg: &ChatMessage) {
        if server_msg.from.is_empty() {
            return;
        }
        if let Some(pos) = self.messages.iter().rposition(|m| {
            m.id.starts_with("local-")
                && m.from.eq_ignore_ascii_case(&server_msg.from)
                && m.text == server_msg.text
        }) {
            if let Some(old) = self.messages.remove(pos) {
                self.message_ids.remove(&old.id);
            }
        }
    }

    /// True when the buffer has no real (non-system) chat lines yet.
    pub fn has_chat_messages(&self) -> bool {
        self.messages.iter().any(|m| !m.is_system)
    }

    pub fn mark_read(&mut self) {
        self.unread = 0;
    }

    fn find_message_mut(&mut self, msg_id: &str) -> Option<&mut ChatMessage> {
        if msg_id.is_empty() {
            return None;
        }
        self.messages
            .iter_mut()
            .find(|m| m.id == msg_id || m.edit_of.as_deref() == Some(msg_id))
    }

    /// Idempotent add of `from`'s reaction. Empty emoji is ignored.
    pub fn add_reaction(&mut self, msg_id: &str, emoji: &str, from: &str) {
        let emoji = emoji.trim();
        if emoji.is_empty() || from.is_empty() {
            return;
        }
        let Some(msg) = self.find_message_mut(msg_id) else {
            return;
        };
        let nicks = msg.reactions.entry(emoji.to_string()).or_default();
        if !nicks.iter().any(|n| n.eq_ignore_ascii_case(from)) {
            nicks.insert(from.to_string());
        }
    }

    /// Remove `from`'s reaction; drop the emoji entry when empty.
    pub fn remove_reaction(&mut self, msg_id: &str, emoji: &str, from: &str) {
        let emoji = emoji.trim();
        if emoji.is_empty() || from.is_empty() {
            return;
        }
        let Some(msg) = self.find_message_mut(msg_id) else {
            return;
        };
        let Some(nicks) = msg.reactions.get_mut(emoji) else {
            return;
        };
        nicks.retain(|n| !n.eq_ignore_ascii_case(from));
        if nicks.is_empty() {
            msg.reactions.remove(emoji);
        }
    }
}

/// Parse server's `+freeq.at/reactions` value (`emoji1:nick1,nick2;emoji2:nick3`).
/// Malformed segments are skipped.
pub fn parse_reactions_tag(raw: &str) -> HashMap<String, HashSet<String>> {
    let mut out = HashMap::new();
    for seg in raw.split(';') {
        let Some((emoji, nicks)) = seg.split_once(':') else {
            continue;
        };
        let emoji = emoji.trim();
        if emoji.is_empty() {
            continue;
        }
        let set: HashSet<String> = nicks
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect();
        if !set.is_empty() {
            out.insert(emoji.to_string(), set);
        }
    }
    out
}

/// Quick-react set — the seven most common reactions (no dance/music extras).
/// Wire form keeps emoji-presentation VS16 where freeq does (`❤️`); strip for display
/// via [`display_emoji`] so egui's NotoEmoji fallback doesn't draw a tofu box for `U+FE0F`.
pub const QUICK_REACT_EMOJIS: &[&str] = &["👍", "❤️", "😂", "😮", "😢", "🥹", "👎"];

/// Default heart reaction glyph (freeq-android parity: heavy black heart + VS16).
/// Kept as the canonical form for tests / future shortcuts (body double-click
/// react was removed so drag/word selection can own that gesture).
#[allow(dead_code)]
pub const DEFAULT_REACT_EMOJI: &str = "❤️";

/// Category tabs for the full reaction emoji picker (Unicode CLDR groups).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmojiPickerGroup {
    #[default]
    Smileys,
    People,
    Animals,
    Food,
    Travel,
    Activities,
    Objects,
    Symbols,
    Flags,
}

impl EmojiPickerGroup {
    pub const ALL: &'static [EmojiPickerGroup] = &[
        EmojiPickerGroup::Smileys,
        EmojiPickerGroup::People,
        EmojiPickerGroup::Animals,
        EmojiPickerGroup::Food,
        EmojiPickerGroup::Travel,
        EmojiPickerGroup::Activities,
        EmojiPickerGroup::Objects,
        EmojiPickerGroup::Symbols,
        EmojiPickerGroup::Flags,
    ];

    /// Tab glyph (rendered via Twemoji).
    pub fn tab_emoji(self) -> &'static str {
        match self {
            EmojiPickerGroup::Smileys => "😀",
            EmojiPickerGroup::People => "👋",
            EmojiPickerGroup::Animals => "🐶",
            EmojiPickerGroup::Food => "🍔",
            EmojiPickerGroup::Travel => "✈️",
            EmojiPickerGroup::Activities => "⚽",
            EmojiPickerGroup::Objects => "💡",
            EmojiPickerGroup::Symbols => "💯",
            EmojiPickerGroup::Flags => "🏁",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EmojiPickerGroup::Smileys => "Smileys & emotion",
            EmojiPickerGroup::People => "People & body",
            EmojiPickerGroup::Animals => "Animals & nature",
            EmojiPickerGroup::Food => "Food & drink",
            EmojiPickerGroup::Travel => "Travel & places",
            EmojiPickerGroup::Activities => "Activities",
            EmojiPickerGroup::Objects => "Objects",
            EmojiPickerGroup::Symbols => "Symbols",
            EmojiPickerGroup::Flags => "Flags",
        }
    }

    fn unicode_group(self) -> emojis::Group {
        match self {
            EmojiPickerGroup::Smileys => emojis::Group::SmileysAndEmotion,
            EmojiPickerGroup::People => emojis::Group::PeopleAndBody,
            EmojiPickerGroup::Animals => emojis::Group::AnimalsAndNature,
            EmojiPickerGroup::Food => emojis::Group::FoodAndDrink,
            EmojiPickerGroup::Travel => emojis::Group::TravelAndPlaces,
            EmojiPickerGroup::Activities => emojis::Group::Activities,
            EmojiPickerGroup::Objects => emojis::Group::Objects,
            EmojiPickerGroup::Symbols => emojis::Group::Symbols,
            EmojiPickerGroup::Flags => emojis::Group::Flags,
        }
    }

    /// Default-skin-tone emojis in this category (Unicode CLDR order).
    pub fn emojis(self) -> impl Iterator<Item = &'static emojis::Emoji> {
        self.unicode_group().emojis()
    }
}

/// Match an emoji against a picker search query (name, shortcodes, or the glyph itself).
pub fn emoji_matches_search(emoji: &emojis::Emoji, query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() {
        return true;
    }
    if emoji.as_str().contains(q) {
        return true;
    }
    let q = q.to_ascii_lowercase();
    if emoji.name().to_ascii_lowercase().contains(&q) {
        return true;
    }
    emoji.shortcodes().any(|s| s.to_ascii_lowercase().contains(&q))
}

/// Max search hits shown in the picker grid (keeps scroll responsive).
pub const EMOJI_SEARCH_LIMIT: usize = 240;

/// Strip variation selectors that NotoEmoji / egui cannot draw (they show as □).
/// Keeps the wire/storage form unchanged — only for labels in chips / picker buttons.
pub fn display_emoji(emoji: &str) -> String {
    emoji
        .chars()
        .filter(|&c| c != '\u{FE0E}' && c != '\u{FE0F}')
        .collect()
}

/// Popular channels shown on Discover (mirrors freeq-android).
pub const POPULAR_CHANNELS: &[(&str, &str)] = &[
    ("#general", "General discussion"),
    ("#test", "Test channel"),
    ("#freeq", "freeq development & support"),
    ("#dev", "Programming & technology"),
    ("#music", "Music recommendations"),
    ("#random", "Off-topic chat"),
    ("#crypto", "Cryptocurrency discussion"),
    ("#gaming", "Games & gaming"),
];

pub fn default_server() -> String {
    crate::auth::DEFAULT_IRC_SERVER.into()
}

pub fn default_auth_broker() -> String {
    crate::auth::DEFAULT_AUTH_BROKER.into()
}

pub fn default_nick() -> String {
    let n = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        % 10_000) as u32;
    format!("sleek{n:04}")
}

/// Whether host looks like freeq public (prefer WSS on mobile).
pub fn prefer_websocket(server: &str) -> bool {
    let host = server.split(':').next().unwrap_or(server);
    host.eq_ignore_ascii_case("irc.freeq.at")
        || host.ends_with(".freeq.at")
        || host.eq_ignore_ascii_case("freeq.at")
}

pub fn websocket_url_for(server: &str) -> String {
    let host = server.split(':').next().unwrap_or(server);
    format!("wss://{host}/irc")
}

/// Which panel is shown on the connect screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMode {
    /// Primary: Bluesky / AT Protocol via auth broker.
    Bluesky,
    /// Guest nick + server (no SASL).
    Guest,
}

/// Params captured when starting a background PNG encode for upload.
#[derive(Debug, Clone)]
pub struct ComposeEncodeMeta {
    pub upload_id: u64,
    pub target: String,
    pub caption: String,
    pub did: String,
    pub api_base: String,
}

/// Image attached to the compose bar (clipboard paste / file pick).
#[derive(Clone)]
pub struct ComposeImage {
    pub width: usize,
    pub height: usize,
    /// Unpremultiplied RGBA8 pixels (`width * height * 4`).
    pub rgba: Arc<[u8]>,
    texture: Option<TextureHandle>,
}

impl std::fmt::Debug for ComposeImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

impl ComposeImage {
    pub fn from_rgba(width: usize, height: usize, rgba: Arc<[u8]>) -> Self {
        Self {
            width,
            height,
            rgba,
            texture: None,
        }
    }

    /// Lazy GPU texture for the compose preview.
    pub fn texture(&mut self, ctx: &egui::Context) -> &TextureHandle {
        if self.texture.is_none() {
            let color = ColorImage::from_rgba_unmultiplied(
                [self.width, self.height],
                self.rgba.as_ref(),
            );
            self.texture = Some(ctx.load_texture(
                "compose_paste_image",
                color,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.texture.as_ref().expect("texture just inserted")
    }
}

/// Video file attached to the compose bar (original bytes, no re-encode).
#[derive(Clone)]
pub struct ComposeVideo {
    pub bytes: Arc<[u8]>,
    pub content_type: String,
    pub filename: String,
}

impl std::fmt::Debug for ComposeVideo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposeVideo")
            .field("content_type", &self.content_type)
            .field("filename", &self.filename)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl ComposeVideo {
    pub fn new(bytes: Arc<[u8]>, content_type: impl Into<String>, filename: impl Into<String>) -> Self {
        Self {
            bytes,
            content_type: content_type.into(),
            filename: filename.into(),
        }
    }

    pub fn size_label(&self) -> String {
        let n = self.bytes.len();
        if n >= 1024 * 1024 {
            format!("{:.1} MB", n as f32 / (1024.0 * 1024.0))
        } else {
            format!("{} KB", (n / 1024).max(1))
        }
    }
}

/// Pending compose attachment — image (RGBA preview) or video (raw file).
#[derive(Debug, Clone)]
pub enum ComposeAttach {
    Image(ComposeImage),
    Video(ComposeVideo),
}

impl ComposeAttach {
    pub fn is_video(&self) -> bool {
        matches!(self, Self::Video(_))
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Image(_) => "Image",
            Self::Video(_) => "Video",
        }
    }
}

// ── Chat media / OG preview cache ──────────────────────────────────────────

/// Decoded remote image ready for (or already on) the GPU.
#[derive(Clone)]
pub struct CachedPixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Arc<[u8]>,
    texture: Option<TextureHandle>,
}

impl std::fmt::Debug for CachedPixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedPixels")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.rgba.len())
            .finish()
    }
}

impl CachedPixels {
    pub fn new(width: usize, height: usize, rgba: Arc<[u8]>) -> Self {
        Self {
            width,
            height,
            rgba,
            texture: None,
        }
    }

    pub fn texture(&mut self, ctx: &egui::Context, id_salt: &str) -> &TextureHandle {
        if self.texture.is_none() {
            let color = ColorImage::from_rgba_unmultiplied(
                [self.width, self.height],
                self.rgba.as_ref(),
            );
            self.texture = Some(ctx.load_texture(
                format!("chat_img_{id_salt}"),
                color,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.texture.as_ref().expect("texture just inserted")
    }
}

#[derive(Debug, Clone)]
pub enum ImageState {
    Loading,
    Ready(CachedPixels),
    Failed,
}

#[derive(Debug, Clone)]
pub enum LinkState {
    Loading,
    /// Fetched (or tag-provided) OG fields.
    Ready(LinkMeta),
    /// Fetch failed — UI still shows a domain card.
    Failed,
}

/// Work items the UI wants the net thread to perform.
#[derive(Debug, Clone)]
pub enum MediaFetch {
    Image(String),
    /// Full-resolution image for the lightbox (chat thumbs are downscaled).
    ImageFull(String),
    Video(String),
    LinkPreview(String),
}

/// Remote video bytes for the vidya preview player.
#[derive(Clone)]
pub enum VideoState {
    Loading,
    Ready(std::sync::Arc<[u8]>),
    Failed,
}

impl std::fmt::Debug for VideoState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading => write!(f, "Loading"),
            Self::Ready(b) => write!(f, "Ready({} bytes)", b.len()),
            Self::Failed => write!(f, "Failed"),
        }
    }
}

/// In-memory cache of remote images and Open Graph cards, plus a pending queue.
#[derive(Debug, Default)]
pub struct MediaCache {
    pub images: HashMap<String, ImageState>,
    /// Full-resolution images for the lightbox, keyed by the same URL.
    pub full_images: HashMap<String, ImageState>,
    pub videos: HashMap<String, VideoState>,
    /// Per-bubble playback widget state (vidya), keyed by `url\\0msgid`.
    #[cfg(feature = "video-previews")]
    pub video_players: HashMap<String, vidya::VideoPlayerState>,
    pub links: HashMap<String, LinkState>,
    pending: Vec<MediaFetch>,
}

impl MediaCache {
    /// Ensure an image fetch is in flight. Returns current state (if any).
    pub fn touch_image(&mut self, url: &str) -> Option<&ImageState> {
        if url.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        if !self.images.contains_key(url) {
            self.images.insert(url.to_string(), ImageState::Loading);
            self.pending.push(MediaFetch::Image(url.to_string()));
        }
        self.images.get(url)
    }

    /// Ensure a full-resolution image fetch is in flight for the lightbox.
    pub fn touch_image_full(&mut self, url: &str) -> Option<&ImageState> {
        if url.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        if !self.full_images.contains_key(url) {
            self.full_images
                .insert(url.to_string(), ImageState::Loading);
            self.pending.push(MediaFetch::ImageFull(url.to_string()));
        }
        self.full_images.get(url)
    }

    /// Ensure a video fetch is in flight for muted inline playback.
    pub fn touch_video(&mut self, url: &str) -> Option<&VideoState> {
        if url.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        if !self.videos.contains_key(url) {
            self.videos.insert(url.to_string(), VideoState::Loading);
            self.pending.push(MediaFetch::Video(url.to_string()));
        }
        self.videos.get(url)
    }

    /// Ensure an OG fetch is in flight (unless already seeded).
    pub fn touch_link(&mut self, url: &str) -> Option<&LinkState> {
        if url.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        if !self.links.contains_key(url) {
            self.links.insert(url.to_string(), LinkState::Loading);
            self.pending.push(MediaFetch::LinkPreview(url.to_string()));
        }
        self.links.get(url)
    }

    /// Seed OG meta from IRCv3 tags without a network round-trip.
    pub fn seed_link(&mut self, url: &str, meta: LinkMeta) {
        if url.is_empty() {
            return;
        }
        // Prefer non-empty title/desc over a later failed fetch.
        match self.links.get(url) {
            Some(LinkState::Ready(existing))
                if existing.title.is_some() || existing.description.is_some() => {}
            _ => {
                if let Some(ref thumb) = meta.thumb_url {
                    self.touch_image(thumb);
                }
                self.links.insert(url.to_string(), LinkState::Ready(meta));
            }
        }
    }

    pub fn set_image_ready(&mut self, url: String, pixels: CachedPixels) {
        self.images.insert(url, ImageState::Ready(pixels));
    }

    pub fn set_image_failed(&mut self, url: String) {
        self.images.insert(url, ImageState::Failed);
    }

    pub fn set_full_image_ready(&mut self, url: String, pixels: CachedPixels) {
        self.full_images.insert(url, ImageState::Ready(pixels));
    }

    pub fn set_full_image_failed(&mut self, url: String) {
        self.full_images.insert(url, ImageState::Failed);
    }

    pub fn set_video_ready(&mut self, url: String, bytes: std::sync::Arc<[u8]>) {
        self.videos.insert(url, VideoState::Ready(bytes));
    }

    pub fn set_video_failed(&mut self, url: String) {
        self.videos.insert(url, VideoState::Failed);
    }

    pub fn set_link_ready(&mut self, url: String, meta: LinkMeta) {
        if let Some(ref thumb) = meta.thumb_url {
            self.touch_image(thumb);
        }
        self.links.insert(url, LinkState::Ready(meta));
    }

    pub fn set_link_failed(&mut self, url: String) {
        self.links.insert(url, LinkState::Failed);
    }

    /// Drain pending fetch requests for the net bridge.
    pub fn drain_pending(&mut self) -> Vec<MediaFetch> {
        std::mem::take(&mut self.pending)
    }

    pub fn has_loading(&self) -> bool {
        self.images.values().any(|s| matches!(s, ImageState::Loading))
            || self
                .full_images
                .values()
                .any(|s| matches!(s, ImageState::Loading))
            || self.videos.values().any(|s| matches!(s, VideoState::Loading))
            || self.links.values().any(|s| matches!(s, LinkState::Loading))
    }
}

/// HTTPS origin for freeq REST APIs derived from the IRC host:port.
pub fn api_base_for_server(server: &str) -> String {
    let host = server.split(':').next().unwrap_or(server).trim();
    if host.is_empty() {
        "https://irc.freeq.at".into()
    } else {
        format!("https://{host}")
    }
}

/// Peer profile modal (nick tap → Bluesky / WHOIS info).
#[derive(Debug, Default)]
pub struct ProfileGate {
    /// Nick the modal is open for (`None` = closed).
    pub nick: Option<String>,
    /// Peer DID when known (from nick map / WHOIS account).
    pub did: Option<String>,
    /// True while waiting on WHOIS and/or Bluesky `getProfile`.
    pub loading: bool,
    pub error: Option<String>,
    pub profile: Option<crate::bsky::ActorProfile>,
    /// Raw WHOIS reply lines for this nick (guest / extra detail).
    pub whois_lines: Vec<String>,
    /// Monotonic id so stale HTTP responses are ignored.
    pub fetch_id: u64,
    /// True after we dispatched WHOIS for this open.
    pub whois_sent: bool,
    /// True while a Bluesky profile HTTP fetch is in flight.
    pub profile_fetching: bool,
}

impl ProfileGate {
    pub fn is_open(&self) -> bool {
        self.nick.is_some()
    }

    pub fn open(&mut self, nick: String, did: Option<String>) {
        self.nick = Some(nick);
        self.did = did;
        // Only flipped true when a Bluesky HTTP fetch is actually started.
        self.loading = false;
        self.error = None;
        self.profile = None;
        self.whois_lines.clear();
        self.whois_sent = false;
        self.profile_fetching = false;
        self.fetch_id = self.fetch_id.wrapping_add(1);
    }

    pub fn close(&mut self) {
        *self = Self {
            fetch_id: self.fetch_id,
            ..Self::default()
        };
    }

    /// Actor string to pass to Bluesky `getProfile`, if resolvable.
    ///
    /// Prefers an AT Proto DID; falls back to a domain-shaped nick (common
    /// freeq pattern where the IRC nick *is* the Bluesky handle).
    pub fn bluesky_actor(&self) -> Option<&str> {
        if let Some(d) = self
            .did
            .as_deref()
            .filter(|d| crate::bsky::is_atproto_did(d))
        {
            return Some(d);
        }
        self.nick
            .as_deref()
            .filter(|n| crate::bsky::looks_like_bsky_handle(n))
            .map(|n| n.trim().trim_start_matches('@'))
    }

    /// WHOIS finished without an AT identity (or nick is offline).
    pub fn whois_exhausted(&self) -> bool {
        self.whois_lines.iter().any(|l| {
            let lower = l.to_ascii_lowercase();
            lower.contains("no such nick") || lower.contains("no such server")
        })
    }
}

/// Join-gate modal state (policy acceptance before JOIN).
#[derive(Debug, Default)]
pub struct PolicyGate {
    /// Channel the modal is open for (`None` = closed).
    pub channel: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub check: Option<crate::policy::PolicyCheck>,
    pub rules: Option<String>,
    pub accepting: bool,
    pub joining: bool,
    /// Monotonic id so stale HTTP responses are ignored.
    pub fetch_id: u64,
    /// When set, refresh the policy check after ACCEPT / verify.
    pub refresh_after: Option<Instant>,
}

impl PolicyGate {
    pub fn is_open(&self) -> bool {
        self.channel.is_some()
    }

    pub fn open(&mut self, channel: String) {
        self.channel = Some(channel);
        self.loading = true;
        self.error = None;
        self.check = None;
        self.rules = None;
        self.accepting = false;
        self.joining = false;
        self.refresh_after = None;
        self.fetch_id = self.fetch_id.wrapping_add(1);
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug)]
pub struct AppState {
    pub connection: ConnectionState,
    pub nick: String,
    pub server: String,
    pub use_tls: bool,
    pub use_websocket: bool,
    pub did: Option<String>,
    /// Bluesky handle when known (from OAuth callback / session).
    pub handle: Option<String>,
    /// Durable auth-broker token for `/session` refresh (not the SASL web-token).
    pub broker_token: Option<String>,
    pub auth_broker: String,
    pub error: Option<String>,
    pub toast: Option<String>,
    /// When the current toast should auto-dismiss (`None` if no toast).
    pub toast_until: Option<Instant>,
    pub status_line: String,
    /// True while the loopback browser OAuth flow is waiting.
    pub awaiting_oauth: bool,

    pub channels: HashMap<String, Buffer>,
    /// Ordered keys for list UI (channel names / dm keys).
    pub channel_order: Vec<String>,
    /// Nick (lowercase) → peer DID — addressing-grade bindings for DM threads.
    pub nick_to_did: HashMap<String, String>,
    /// Peer DID → display nick (for rendering DID-keyed buffers).
    pub did_to_nick: HashMap<String, String>,
    /// Durable unread index (SQLite). Client-owned; independent of the server.
    pub message_store: crate::message_store::MessageStore,
    /// Open `chathistory` BATCH id → channel (suppress unread until baseline).
    pub history_batches: HashMap<String, String>,
    pub active_channel: Option<String>,
    pub route: Route,
    pub tab: Tab,

    pub compose: String,
    /// Pending image/video attachment (shown above the text field).
    pub compose_attach: Option<ComposeAttach>,
    /// True while a pasted image/video is uploading to freeq.
    pub compose_uploading: bool,
    /// Monotonic id for the in-flight compose upload (cancel / stale finish).
    pub compose_upload_id: u64,
    /// When the current upload (or PNG encode) started — watchdog unlock.
    pub compose_upload_started: Option<Instant>,
    /// Background PNG encode for the in-flight upload (keeps egui unblocked).
    pub compose_encode_rx: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
    /// Captured when encode started; used to dispatch [`crate::net::NetCmd::UploadAndSend`].
    pub compose_encode_meta: Option<ComposeEncodeMeta>,
    /// One-shot: focus the compose / caption TextEdit next frame (after attach).
    pub focus_compose: bool,
    /// Tab-cycle state for compose nick completion (IRC-style).
    pub compose_nick_tab: NickTabComplete,
    /// In-flight OS media file dialog (attach button). Polled each frame.
    pub file_pick_rx: Option<std::sync::mpsc::Receiver<crate::clipboard::PickAttachResult>>,
    /// When `file_pick_rx` was set — used to unlock the compose bar if the OS
    /// dialog dies without a result (e.g. Android cancel with no activity result).
    pub file_pick_started: Option<Instant>,
    /// Remote image + Open Graph link-preview cache for chat embeds.
    pub media: MediaCache,
    /// Nick → Bluesky avatar URL cache (DID-resolved).
    pub avatars: crate::avatar_cache::AvatarCache,
    pub search: String,
    pub join_input: String,
    pub discover_input: String,
    /// Raw IRC command typed in the settings server console.
    pub console_input: String,
    /// Channel member list open (chat detail only).
    pub show_members: bool,
    /// Global message search overlay open (chat detail only).
    pub show_message_search: bool,
    /// Query for the message search overlay.
    pub message_search: String,
    /// One-shot: focus the message search field next frame.
    pub focus_message_search: bool,
    /// Scroll the chat list to this msgid once (from search or a reply preview).
    pub scroll_to_msgid: Option<String>,
    /// Briefly highlight this msgid after navigating from search or a reply.
    pub highlight_msgid: Option<String>,
    /// When `highlight_msgid` should clear.
    pub highlight_until: Option<Instant>,
    /// Message id whose emoji reaction picker dialog is open (active chat only).
    pub react_picker_msg: Option<String>,
    /// Search query for the reaction picker dialog (cleared when it closes).
    pub react_picker_search: String,
    /// Active category tab in the reaction picker dialog.
    pub react_picker_group: EmojiPickerGroup,
    /// Full-screen image lightbox URL (chat embed tap). `None` = closed.
    pub image_lightbox: Option<String>,
    /// msgid currently being edited in the compose bar (`None` = normal send).
    pub editing_msgid: Option<String>,
    /// Message being replied to in the compose bar (`None` = normal send).
    pub replying_to: Option<ReplyTarget>,

    /// Connect form fields (editable while disconnected).
    pub connect_mode: ConnectMode,
    pub form_handle: String,
    pub form_callback: String,
    pub form_nick: String,
    pub form_server: String,
    pub form_tls: bool,
    pub form_websocket: bool,
    /// Bluesky handle typeahead (connect screen).
    pub handle_typeahead: crate::bsky::HandleTypeahead,
    /// Guest session was loaded from disk — consumed once for launch auto-connect.
    pub auto_guest_connect: bool,

    /// Our active AV call (at most one at a time).
    pub local_call: Option<LocalCall>,
    /// Preferred mic mute for the next call (and mirrored while in-call).
    /// Toggleable before start/join so you can join muted. Persisted in prefs.json.
    pub av_pref_muted: bool,
    /// Preferred speaker mute (remote audio off) for the next call / in-call.
    /// Toggleable before start/join. Persisted in prefs.json.
    pub av_pref_speaker_muted: bool,
    /// Preferred camera publish for the next call (MoQ).
    /// Toggleable before start/join; applied when hardware is available.
    /// Persisted in prefs.json (survives logout).
    pub av_pref_camera: bool,
    /// Preferred camera device id (`None` = first available). Persisted.
    pub av_pref_camera_id: Option<String>,
    /// Preferred mic device name (`None` = system default). Persisted.
    pub av_pref_mic_id: Option<String>,
    /// Preferred speaker device name (`None` = system default). Persisted.
    pub av_pref_speaker_id: Option<String>,
    /// Shared latest video frames from the MoQ media plane (desktop).
    pub av_video: Option<VideoFrameStore>,
    /// Live mic level meter (shared with capture thread) while media is Live.
    pub av_mic_level: Option<crate::av::MicLevel>,
    /// GPU textures for call video tiles (`path_key` → texture), refreshed each paint.
    /// Keys are `__local__` or `nick[~instance]` (see [`crate::av::path_key`]).
    /// Wrapped so `AppState: Debug` does not require `TextureHandle: Debug`.
    pub av_video_textures: AvVideoTextures,
    /// Focused/enlarged video tile key (`"__local__"` for self). `None` = equal grid.
    /// Click a tile to focus; click the focused tile again to restore the grid.
    pub av_focused_video: Option<String>,
    /// Cached device lists for AV selectors (refreshed when the call panel opens).
    pub av_device_cameras: Vec<crate::av_media::MediaDevice>,
    pub av_device_mics: Vec<crate::av_media::MediaDevice>,
    pub av_device_speakers: Vec<crate::av_media::MediaDevice>,
    /// In-call / pre-call: show Cam/Mic/Out pickers (collapsed by default mid-call
    /// so mute/leave stay uncluttered; expanded when the user opens Devices).
    pub av_show_devices: bool,
    /// Height of the in-call video stage (drag the handle under the tiles to resize).
    pub av_video_height: f32,

    /// Recently visited channels (MRU first). Persisted in prefs.json and
    /// auto-rejoined on connect — server membership restore is unreliable.
    pub recent_channels: Vec<String>,
    /// Previously used guest nicknames (MRU). Persisted; shown on guest login.
    pub recent_nicks: Vec<String>,
    /// Previously used Bluesky handles (MRU). Persisted; shown on Bluesky login.
    pub recent_handles: Vec<String>,
    /// Recently used reaction emoji (MRU first). Persisted; shown in reaction picker.
    pub recent_emoji: Vec<String>,
    /// Show JOIN / PART / QUIT system lines in channel buffers. Persisted.
    pub show_join_part: bool,

    /// User clicked Disconnect / Cancel / Logout — do not auto-reconnect.
    pub intentional_disconnect: bool,
    /// Consecutive unexpected disconnect / failed-reconnect attempts.
    pub reconnect_attempts: u32,
    /// When set, fire auto-reconnect once `Instant::now() >=` this deadline.
    pub reconnect_at: Option<Instant>,

    /// Channel policy join-gate modal (authenticated users blocked by ACCEPT rules).
    pub policy_gate: PolicyGate,
    /// Peer profile modal (tap nick in channel / member list).
    pub profile_gate: ProfileGate,
    /// MoQ media-plane reconnect while `local_call` is still active.
    pub media_reconnect_attempts: u32,
    pub media_reconnect_at: Option<Instant>,
    /// When the media plane last became `Live`. Used so brief Live→drop
    /// blips do not reset reconnect backoff (that thrashed MoQ every 2s).
    pub media_live_since: Option<Instant>,
}

/// egui texture + last-uploaded frame gen for AV tiles.
#[derive(Default)]
pub struct AvVideoTextures(pub HashMap<String, (TextureHandle, u64)>);

impl std::fmt::Debug for AvVideoTextures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AvVideoTextures")
            .field(&self.0.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl AvVideoTextures {
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn retain(&mut self, mut f: impl FnMut(&String) -> bool) {
        self.0.retain(|k, _| f(k));
    }
}

impl AppState {
    pub fn new() -> Self {
        let nick = default_nick();
        let server = default_server();
        let use_ws = cfg!(target_os = "android");
        let prefs = crate::auth::SavedPrefs::load();
        let mut state = Self {
            connection: ConnectionState::Disconnected,
            nick: nick.clone(),
            server: server.clone(),
            use_tls: true,
            use_websocket: use_ws,
            did: None,
            handle: None,
            broker_token: None,
            auth_broker: default_auth_broker(),
            error: None,
            toast: None,
            toast_until: None,
            status_line: "Not connected".into(),
            awaiting_oauth: false,
            channels: HashMap::new(),
            channel_order: Vec::new(),
            nick_to_did: HashMap::new(),
            did_to_nick: HashMap::new(),
            message_store: crate::message_store::MessageStore::open_default(),
            history_batches: HashMap::new(),
            active_channel: None,
            route: Route::Tabs,
            tab: Tab::Chats,
            compose: String::new(),
            compose_attach: None,
            compose_uploading: false,
            compose_upload_id: 0,
            compose_upload_started: None,
            compose_encode_rx: None,
            compose_encode_meta: None,
            focus_compose: false,
            compose_nick_tab: NickTabComplete::default(),
            file_pick_rx: None,
            file_pick_started: None,
            media: MediaCache::default(),
            avatars: crate::avatar_cache::AvatarCache::default(),
            search: String::new(),
            join_input: String::new(),
            discover_input: String::new(),
            console_input: String::new(),
            show_members: false,
            show_message_search: false,
            message_search: String::new(),
            focus_message_search: false,
            scroll_to_msgid: None,
            highlight_msgid: None,
            highlight_until: None,
            react_picker_msg: None,
            react_picker_search: String::new(),
            react_picker_group: EmojiPickerGroup::default(),
            image_lightbox: None,
            editing_msgid: None,
            replying_to: None,
            connect_mode: ConnectMode::Bluesky,
            form_handle: String::new(),
            form_callback: String::new(),
            form_nick: nick,
            form_server: server,
            form_tls: true,
            form_websocket: use_ws,
            handle_typeahead: crate::bsky::HandleTypeahead::default(),
            auto_guest_connect: false,
            local_call: None,
            av_pref_muted: prefs.av_pref_muted,
            av_pref_speaker_muted: prefs.av_pref_speaker_muted,
            av_pref_camera: prefs.av_pref_camera,
            av_pref_camera_id: prefs.av_pref_camera_id.filter(|s| !s.is_empty()),
            // Clear ALSA virtual PCMs (sysdefault/dmix/…) — they break under
            // PipeWire and used to take down both mic and speaker streams.
            av_pref_mic_id: crate::av_media::sanitize_audio_device_pref(prefs.av_pref_mic_id),
            av_pref_speaker_id: crate::av_media::sanitize_audio_device_pref(
                prefs.av_pref_speaker_id,
            ),
            av_video: None,
            av_mic_level: None,
            av_video_textures: AvVideoTextures::default(),
            av_focused_video: None,
            av_device_cameras: Vec::new(),
            av_device_mics: Vec::new(),
            av_device_speakers: Vec::new(),
            av_show_devices: false,
            av_video_height: 220.0,
            recent_channels: normalize_recent_channels(prefs.recent_channels),
            recent_nicks: normalize_recent_nicks(prefs.recent_nicks),
            recent_handles: normalize_recent_handles(prefs.recent_handles),
            recent_emoji: prefs.recent_emoji,
            show_join_part: prefs.show_join_part,
            intentional_disconnect: false,
            reconnect_attempts: 0,
            reconnect_at: None,
            policy_gate: PolicyGate::default(),
            profile_gate: ProfileGate::default(),
            media_reconnect_attempts: 0,
            media_reconnect_at: None,
            media_live_since: None,
        };
        if let Some(saved) = crate::auth::SavedSession::load() {
            if saved.has_session() {
                state.broker_token = Some(saved.broker_token);
                state.did = if saved.did.is_empty() {
                    None
                } else {
                    Some(saved.did)
                };
                state.handle = if saved.handle.is_empty() {
                    None
                } else {
                    Some(saved.handle.clone())
                };
                if !saved.nick.is_empty() && !saved.nick.starts_with("Guest") {
                    state.nick = saved.nick.clone();
                    state.form_nick = saved.nick;
                }
                if !saved.server.is_empty() {
                    state.server = saved.server.clone();
                    state.form_server = saved.server;
                }
                if let Some(h) = &state.handle {
                    state.form_handle = h.clone();
                    // Ensure saved Bluesky handle appears in login history even
                    // if prefs were lost / written to a legacy path before #42.
                    let prior = state.recent_handles.clone();
                    push_mru(&mut state.recent_handles, h, MAX_RECENT_HANDLES);
                    if state.recent_handles != prior {
                        state.persist_prefs();
                    }
                }
                state.connect_mode = ConnectMode::Bluesky;
            } else if saved.has_guest() {
                // Restore last guest nick/server and auto-connect on launch.
                state.nick = saved.nick.clone();
                state.form_nick = saved.nick.clone();
                // Ensure the remembered guest nick appears in login history even
                // if it was saved before recent_nicks existed.
                let prior = state.recent_nicks.clone();
                push_mru(&mut state.recent_nicks, &saved.nick, MAX_RECENT_NICKS);
                if state.recent_nicks != prior {
                    // Migrated into MRU — persist so the connect screen can show it
                    // after logout without relying on session.json.
                    state.persist_prefs();
                }
                if !saved.server.is_empty() {
                    state.server = saved.server.clone();
                    state.form_server = saved.server;
                }
                state.use_tls = saved.use_tls;
                state.form_tls = saved.use_tls;
                state.use_websocket = saved.use_websocket;
                state.form_websocket = saved.use_websocket;
                state.connect_mode = ConnectMode::Guest;
                state.auto_guest_connect = true;
            }
        }
        // Pre-fill Bluesky handle from prefs if not already set from session.
        if state.form_handle.is_empty() {
            if let Some(handle) = prefs.last_bsky_handle {
                if !handle.is_empty() {
                    state.form_handle = handle;
                }
            } else if let Some(handle) = state.recent_handles.first() {
                state.form_handle = handle.clone();
            }
        }
        state
    }

    pub fn has_saved_session(&self) -> bool {
        self.broker_token
            .as_ref()
            .is_some_and(|t| !t.is_empty())
    }

    /// Remembered guest nick on disk (or about to auto-connect).
    pub fn has_saved_guest(&self) -> bool {
        self.auto_guest_connect
            || crate::auth::SavedSession::load().is_some_and(|s| s.has_guest())
    }

    /// Show a toast that auto-dismisses after a few seconds.
    pub fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast = Some(msg.into());
        self.toast_until = Some(Instant::now() + TOAST_DURATION);
    }

    /// True while the OS attach-media dialog is open (or a chosen file is decoding).
    pub fn file_pick_busy(&self) -> bool {
        self.file_pick_rx.is_some()
    }

    /// Launch the OS image/video file picker (no-op if one is already open).
    pub fn start_file_pick(&mut self) {
        if self.file_pick_rx.is_some() {
            return;
        }
        self.file_pick_rx = Some(crate::clipboard::start_pick_media_file());
        self.file_pick_started = Some(Instant::now());
    }

    /// Clear in-flight pick state (success, cancel, error, or safety timeout).
    fn clear_file_pick(&mut self) {
        self.file_pick_rx = None;
        self.file_pick_started = None;
    }

    /// Drop in-flight upload/encode UI lock (Cancel or watchdog). Does not stop
    /// the network request; a late finish with a stale `upload_id` is ignored.
    pub fn cancel_compose_upload(&mut self) {
        self.compose_uploading = false;
        self.compose_upload_started = None;
        self.compose_encode_rx = None;
        self.compose_encode_meta = None;
        // Invalidate in-flight work so a late UploadFinished is ignored.
        self.compose_upload_id = self.compose_upload_id.wrapping_add(1);
    }

    /// Remove the attached media and unlock compose (✕ / Cancel).
    pub fn clear_compose_attach(&mut self) {
        self.cancel_compose_upload();
        self.compose_attach = None;
    }

    /// Apply a finished OS file-dialog result. Call once per frame from the app loop.
    ///
    /// Returns `true` if a pick is still in progress (caller should repaint).
    pub fn poll_file_pick(&mut self) -> bool {
        let Some(rx) = self.file_pick_rx.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok(Some(attach))) => {
                self.compose_attach = Some(attach);
                self.focus_compose = true;
                self.clear_file_pick();
                false
            }
            Ok(Ok(None)) => {
                // User cancelled / dismissed without picking.
                self.clear_file_pick();
                false
            }
            Ok(Err(e)) => {
                self.show_toast(e);
                self.clear_file_pick();
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Desktop portal dialogs that never appear used to leave attach
                // "busy" for minutes; Android needs longer for the scavenger.
                #[cfg(target_os = "android")]
                const PICK_UI_TIMEOUT: Duration = Duration::from_secs(185);
                #[cfg(not(target_os = "android"))]
                const PICK_UI_TIMEOUT: Duration = Duration::from_secs(45);
                if self
                    .file_pick_started
                    .is_some_and(|t| t.elapsed() > PICK_UI_TIMEOUT)
                {
                    self.clear_file_pick();
                    self.show_toast("File picker closed".to_string());
                    false
                } else {
                    true
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.show_toast("File picker exited unexpectedly".to_string());
                self.clear_file_pick();
                false
            }
        }
    }

    /// Unlock compose if an upload/encode hangs past the HTTP timeout budget.
    ///
    /// Returns `true` when still uploading (caller should keep repainting).
    pub fn tick_compose_upload(&mut self) -> bool {
        if !self.compose_uploading {
            return false;
        }
        // HTTP client timeout is 45s; allow encode + a little slack.
        const UPLOAD_UI_TIMEOUT: Duration = Duration::from_secs(60);
        if self
            .compose_upload_started
            .is_some_and(|t| t.elapsed() > UPLOAD_UI_TIMEOUT)
        {
            self.cancel_compose_upload();
            self.show_toast("Image upload timed out — try again".to_string());
            return false;
        }
        true
    }

    pub fn clear_toast(&mut self) {
        self.toast = None;
        self.toast_until = None;
    }

    /// Drop the toast if its timer has elapsed. Returns `true` if still visible.
    pub fn tick_toast(&mut self) -> bool {
        if self.toast.is_none() {
            return false;
        }
        if self
            .toast_until
            .is_some_and(|until| Instant::now() >= until)
        {
            self.clear_toast();
            return false;
        }
        true
    }

    /// Persist app prefs (AV + chat + recent rooms / nicks / handles) to disk
    /// (independent of session logout).
    pub fn persist_prefs(&self) {
        let mut prefs = crate::auth::SavedPrefs::load();
        prefs.av_pref_muted = self.av_pref_muted;
        prefs.av_pref_speaker_muted = self.av_pref_speaker_muted;
        prefs.av_pref_camera = self.av_pref_camera;
        prefs.av_pref_camera_id = self.av_pref_camera_id.clone();
        prefs.av_pref_mic_id = self.av_pref_mic_id.clone();
        prefs.av_pref_speaker_id = self.av_pref_speaker_id.clone();
        prefs.recent_channels = self.recent_channels.clone();
        prefs.recent_nicks = self.recent_nicks.clone();
        prefs.recent_handles = self.recent_handles.clone();
        prefs.recent_emoji = self.recent_emoji.clone();
        prefs.show_join_part = self.show_join_part;
        if let Some(h) = self.recent_handles.first() {
            prefs.last_bsky_handle = Some(h.clone());
        }
        if let Err(e) = prefs.save() {
            log::warn!("failed to save prefs: {e}");
        }
    }

    /// Refresh cached camera/mic/speaker lists for the device pickers.
    pub fn refresh_av_devices(&mut self) {
        self.av_device_cameras = crate::av_media::list_cameras();
        self.av_device_mics = crate::av_media::list_microphones();
        self.av_device_speakers = crate::av_media::list_speakers();
    }

    /// Alias kept for call sites that only touch AV toggles.
    pub fn persist_av_prefs(&self) {
        self.persist_prefs();
    }

    /// Channels to auto-join after registration (client-side room memory).
    /// Guests always get `#test` in the list so first-run / guest login has a
    /// useful room even when prefs remembered no channels.
    pub fn auto_join_channels(&self) -> Vec<String> {
        let mut chans = if self.recent_channels.is_empty() {
            vec!["#test".into()]
        } else {
            self.recent_channels.clone()
        };
        let is_guest = self.did.is_none() && self.broker_token.is_none();
        if is_guest && !chans.iter().any(|c| c.eq_ignore_ascii_case("#test")) {
            chans.push("#test".into());
        }
        chans
    }

    /// Bluesky handle for session save / prefs — broker token may omit `handle`.
    fn effective_bsky_handle(&self) -> String {
        self.handle
            .as_deref()
            .filter(|h| !h.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| normalize_bsky_handle(&self.form_handle))
    }

    /// Record a Bluesky handle in MRU prefs (survives logout and restart).
    pub fn remember_bsky_handle(&mut self, handle: &str) {
        let h = normalize_bsky_handle(handle);
        if h.is_empty() {
            return;
        }
        let prior = self.recent_handles.clone();
        push_mru(&mut self.recent_handles, &h, MAX_RECENT_HANDLES);
        if self.recent_handles != prior {
            self.persist_prefs();
        }
    }

    /// Record a successfully visited channel and persist.
    /// Channels only — DMs are not auto-joined. No-op if already listed
    /// (avoids reshuffling order / rewriting prefs on every reconnect join).
    pub fn remember_channel(&mut self, channel: &str) {
        let ch = Self::normalize_channel(channel);
        if ch.is_empty() || !(ch.starts_with('#') || ch.starts_with('&')) {
            return;
        }
        // Skip status / special buffers.
        if ch == "*status" {
            return;
        }
        if self
            .recent_channels
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&ch))
        {
            return;
        }
        // New rooms go to the front of the auto-join list.
        self.recent_channels.insert(0, ch);
        const MAX_RECENT: usize = 50;
        if self.recent_channels.len() > MAX_RECENT {
            self.recent_channels.truncate(MAX_RECENT);
        }
        self.persist_prefs();
    }

    /// Drop a channel from the auto-rejoin list (user left / kicked).
    pub fn forget_channel(&mut self, channel: &str) {
        let before = self.recent_channels.len();
        self.recent_channels
            .retain(|c| !c.eq_ignore_ascii_case(channel));
        if self.recent_channels.len() != before {
            // Empty list is fine — `auto_join_channels` falls back to defaults.
            self.persist_prefs();
        }
    }

    pub fn persist_session(&mut self) {
        let now = chrono::Utc::now().timestamp();
        if let Some(broker_token) = self.broker_token.clone() {
            // Never poison storage with a Guest temp nick while DID-authenticated
            // (freeq-android AuthRecoveryTest).
            let handle = self.effective_bsky_handle();
            let nick = if self.did.is_some() && self.nick.starts_with("Guest") {
                handle.clone()
            } else {
                self.nick.clone()
            };
            let session = crate::auth::SavedSession {
                broker_token,
                did: self.did.clone().unwrap_or_default(),
                handle: handle.clone(),
                nick,
                server: self.server.clone(),
                last_login_unix: now,
                guest: false,
                use_tls: self.use_tls,
                use_websocket: self.use_websocket,
            };
            if let Err(e) = session.save() {
                log::warn!("failed to save session: {e}");
            }
            // Remember handle in prefs (survives logout) + MRU history.
            if !handle.is_empty() {
                self.remember_bsky_handle(&handle);
            }
            return;
        }

        // Guest: remember nick + server so next launch reconnects automatically.
        if self.did.is_none() && !self.nick.trim().is_empty() {
            let session = crate::auth::SavedSession {
                broker_token: String::new(),
                did: String::new(),
                handle: String::new(),
                nick: self.nick.clone(),
                server: self.server.clone(),
                last_login_unix: now,
                guest: true,
                use_tls: self.use_tls,
                use_websocket: self.use_websocket,
            };
            if let Err(e) = session.save() {
                log::warn!("failed to save guest session: {e}");
            }
            push_mru(&mut self.recent_nicks, &self.nick, MAX_RECENT_NICKS);
            self.persist_prefs();
        }
    }

    /// Drop broker/DID identity and any remembered guest from memory and disk.
    /// Does not touch connection buffers — use [`Self::logout`] for a full sign-out.
    pub fn clear_auth(&mut self) {
        self.broker_token = None;
        self.did = None;
        self.handle = None;
        self.form_handle.clear();
        self.auto_guest_connect = false;
        crate::auth::SavedSession::clear();
        // Reload MRU lists from disk in case in-memory state was cleared without
        // a matching prefs write (e.g. process death mid-login).
        if self.recent_handles.is_empty() || self.recent_nicks.is_empty() {
            let prefs = crate::auth::SavedPrefs::load();
            if self.recent_handles.is_empty() {
                self.recent_handles = normalize_recent_handles(prefs.recent_handles);
            }
            if self.recent_nicks.is_empty() {
                self.recent_nicks = normalize_recent_nicks(prefs.recent_nicks);
            }
        }
        // Restore remembered handle from prefs / MRU so it's pre-filled for next login.
        if let Some(handle) = self.recent_handles.first().cloned() {
            self.form_handle = handle;
        } else {
            let prefs = crate::auth::SavedPrefs::load();
            if let Some(handle) = prefs.last_bsky_handle {
                if !handle.is_empty() {
                    self.form_handle = handle;
                }
            }
        }
        // Prefill the most recent guest nick when returning to the connect form.
        if let Some(nick) = self.recent_nicks.first() {
            if !nick.is_empty() {
                self.form_nick = nick.clone();
            }
        }
    }

    /// True when a previous Bluesky login is still on disk or in memory.
    pub fn has_cached_identity(&self) -> bool {
        self.has_saved_session()
            || self.did.as_ref().is_some_and(|d| !d.is_empty())
            || self.handle.as_ref().is_some_and(|h| !h.is_empty())
    }

    pub fn is_connected(&self) -> bool {
        self.connection == ConnectionState::Registered
            || self.connection == ConnectionState::Connected
    }

    pub fn total_unread(&self) -> u32 {
        self.channels.values().map(|b| b.unread).sum()
    }

    /// Stable account namespace for the local message index.
    pub fn account_key(&self) -> String {
        self.did
            .clone()
            .unwrap_or_else(|| format!("guest:{}", self.nick.to_lowercase()))
    }

    /// Whether `channel` currently has an in-flight CHATHISTORY batch.
    pub fn channel_in_history_batch(&self, channel: &str) -> bool {
        self.history_batches
            .values()
            .any(|c| c.eq_ignore_ascii_case(channel))
    }

    /// Record a chat message in SQLite and refresh the buffer unread badge.
    pub fn record_message_for_unread(
        &mut self,
        channel: &str,
        msgid: &str,
        ts: i64,
        from_me: bool,
        viewing: bool,
    ) {
        if channel == "*status" || msgid.is_empty() {
            return;
        }
        let account = self.account_key();
        let in_history = self.channel_in_history_batch(channel);
        if let Err(e) =
            self.message_store
                .insert_message(&account, channel, msgid, ts, from_me)
        {
            log::warn!("message_store insert: {e:#}");
            return;
        }
        if viewing {
            if let Err(e) = self.message_store.mark_read(&account, channel, ts) {
                log::warn!("message_store mark_read: {e:#}");
            }
        } else if !from_me && !in_history {
            if let Err(e) = self
                .message_store
                .ensure_live_baseline(&account, channel, msgid)
            {
                log::warn!("message_store baseline: {e:#}");
            }
        }
        self.refresh_unread(channel);
    }

    /// Recompute `Buffer.unread` from SQLite for one channel.
    pub fn refresh_unread(&mut self, channel: &str) {
        let account = self.account_key();
        let n = match self.message_store.unread_count(&account, channel) {
            Ok(n) => n,
            Err(e) => {
                log::warn!("message_store unread: {e:#}");
                return;
            }
        };
        if let Some(buf) = self
            .find_buffer_key(channel)
            .and_then(|k| self.channels.get_mut(&k))
        {
            buf.unread = n;
        }
    }

    pub fn ensure_buffer(&mut self, name: &str) -> &mut Buffer {
        // Prefer an existing case-insensitive match so "Alice" / "alice" share one row.
        if let Some(existing) = self.find_buffer_key(name) {
            return self.channels.get_mut(&existing).expect("key from find");
        }
        let key = name.to_string();
        let account = self.account_key();
        let unread = self
            .message_store
            .unread_count(&account, &key)
            .unwrap_or(0);
        let mut buf = Buffer::new(&key);
        buf.unread = unread;
        self.channels.insert(key.clone(), buf);
        self.channel_order.push(key.clone());
        self.channels.get_mut(&key).expect("just inserted")
    }

    /// Case-insensitive lookup of an existing buffer key.
    pub fn find_buffer_key(&self, name: &str) -> Option<String> {
        if self.channels.contains_key(name) {
            return Some(name.to_string());
        }
        self.channels
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .cloned()
    }

    /// Canonical DM buffer key: peer DID when known, else the nick (or DID input).
    /// Channels pass through unchanged.
    pub fn dm_buffer_key(&self, peer: &str) -> String {
        if peer.starts_with('#') || peer.starts_with('&') || peer == "*status" {
            return peer.to_string();
        }
        freeq_sdk::address::dm_peer_key(peer, |n| {
            self.nick_to_did.get(&n.to_lowercase()).cloned()
        })
    }

    /// Human label for a buffer key (DID → nick when known, else shortened DID).
    pub fn display_name_for(&self, key: &str) -> String {
        if !freeq_sdk::address::is_did(key) {
            return key.to_string();
        }
        if let Some(nick) = self.did_to_nick.get(key) {
            return nick.clone();
        }
        if let Some((nick, _)) = self
            .nick_to_did
            .iter()
            .find(|(_, did)| did.as_str() == key)
        {
            // Prefer original casing if a buffer still carries it.
            if let Some(buf_key) = self.find_buffer_key(nick) {
                if !freeq_sdk::address::is_did(&buf_key) {
                    return buf_key;
                }
            }
            return nick.clone();
        }
        shorten_did(key)
    }

    /// Record nick↔DID and fold any nick-keyed DM thread into the DID-keyed one.
    /// Returns true when a buffer merge happened (caller may need to refresh UI route).
    pub fn adopt_dm_binding(&mut self, nick: &str, did: &str) -> bool {
        if !freeq_sdk::address::is_did(did) || nick.eq_ignore_ascii_case(did) {
            return false;
        }
        if nick.starts_with('#') || nick.starts_with('&') {
            return false;
        }
        self.nick_to_did
            .insert(nick.to_lowercase(), did.to_string());
        self.did_to_nick.insert(did.to_string(), nick.to_string());
        self.merge_dm_buffers(nick, did)
    }

    /// Fold nick-keyed DM buffer into DID-keyed one (freeq-android DidDisplay.mergeDmBuffers).
    fn merge_dm_buffers(&mut self, nick: &str, did: &str) -> bool {
        let Some(nick_key) = self.find_buffer_key(nick) else {
            return false;
        };
        if freeq_sdk::address::is_did(&nick_key) {
            return false;
        }
        if nick_key == did {
            return false;
        }

        let Some(nick_buf) = self.channels.remove(&nick_key) else {
            return false;
        };
        self.channel_order
            .retain(|n| !n.eq_ignore_ascii_case(&nick_key));

        if let Some(did_buf) = self.channels.get_mut(did) {
            did_buf.absorb(nick_buf);
        } else {
            let mut rekeyed = Buffer::new(did);
            rekeyed.absorb(nick_buf);
            rekeyed.name = did.to_string();
            self.channels.insert(did.to_string(), rekeyed);
            if !self.channel_order.iter().any(|n| n == did) {
                self.channel_order.push(did.to_string());
            }
        }

        // Repoint active chat / route if the user was on the nick thread.
        if self
            .active_channel
            .as_ref()
            .is_some_and(|a| a.eq_ignore_ascii_case(&nick_key))
        {
            self.active_channel = Some(did.to_string());
        }
        if let Route::Chat(ref open) = self.route {
            if open.eq_ignore_ascii_case(&nick_key) {
                self.route = Route::Chat(did.to_string());
            }
        }
        true
    }

    /// Own-echo binding: server echo of our DM carries target=nick, dm_key=DID.
    /// Adopt before routing so the local-echo nick thread folds into the DID thread.
    pub fn adopt_echo_binding(
        &mut self,
        is_self: bool,
        target: &str,
        dm_key: Option<&str>,
    ) -> bool {
        if !is_self {
            return false;
        }
        let Some(did) = dm_key else {
            return false;
        };
        if !freeq_sdk::address::is_did(did) {
            return false;
        }
        if target.starts_with('#') || target.starts_with('&') {
            return false;
        }
        if freeq_sdk::address::is_did(target) {
            return false;
        }
        if target.eq_ignore_ascii_case(did) {
            return false;
        }
        self.adopt_dm_binding(target, did)
    }

    pub fn open_chat(&mut self, name: &str) {
        // Resolve DM peers to their DID key when known so we open the same
        // thread incoming messages land in.
        let key = self.dm_buffer_key(name);
        // Switching buffers: drop pending compose so we don't send into the wrong room.
        if self.active_channel.as_deref() != Some(key.as_str()) {
            self.compose.clear();
            self.clear_compose_attach();
            self.compose_nick_tab.clear();
            self.cancel_edit();
            self.cancel_reply();
        }
        self.ensure_buffer(&key);
        self.active_channel = Some(key.clone());
        self.route = Route::Chat(key.clone());
        // Wide master–detail and phone both land on the Chats tab after open
        // (Discover → channel should not leave the Chats list hidden).
        self.tab = Tab::Chats;
        self.show_members = false;
        self.close_message_search();
        self.close_react_picker();
        self.close_image_lightbox();
        // Drop stale scroll/highlight when opening via the list (search nav
        // re-sets these immediately after `open_chat`).
        self.scroll_to_msgid = None;
        self.clear_search_highlight();
        let account = self.account_key();
        let at = chrono::Utc::now().timestamp();
        if let Err(e) = self.message_store.mark_read(&account, &key, at) {
            log::warn!("message_store mark_read {key}: {e:#}");
        }
        let unread = self
            .message_store
            .unread_count(&account, &key)
            .unwrap_or(0);
        if let Some(buf) = self.channels.get_mut(&key) {
            buf.unread = unread;
        }
    }

    pub fn close_chat(&mut self) {
        self.active_channel = None;
        self.route = Route::Tabs;
        self.tab = Tab::Chats;
        self.show_members = false;
        self.close_message_search();
        self.close_react_picker();
        self.close_image_lightbox();
        self.scroll_to_msgid = None;
        self.clear_search_highlight();
        self.compose.clear();
        self.clear_compose_attach();
        self.compose_nick_tab.clear();
        self.cancel_edit();
        self.cancel_reply();
    }

    /// One step of in-chat back navigation (header ← / Esc / Android back gesture).
    ///
    /// Dismisses the topmost overlay or sheet; returns `true` when the chat page
    /// itself should close (`Route::Tabs`). Reply/edit compose chrome is included
    /// so Esc and system back peel those before leaving the conversation.
    pub fn chat_back_step(&mut self) -> bool {
        if self.image_lightbox.is_some() {
            self.close_image_lightbox();
            false
        } else if self.profile_gate.is_open() {
            self.close_profile_gate();
            false
        } else if self.react_picker_msg.is_some() {
            self.close_react_picker();
            false
        } else if self.show_message_search {
            self.close_message_search();
            false
        } else if self.show_members {
            self.show_members = false;
            false
        } else if self.replying_to.is_some() {
            self.cancel_reply();
            false
        } else if self.editing_msgid.is_some() {
            self.cancel_edit();
            false
        } else {
            true
        }
    }

    /// Open the full-screen image lightbox for a chat embed URL.
    pub fn open_image_lightbox(&mut self, url: String) {
        if url.is_empty() {
            return;
        }
        // Kick off the full-res fetch; the lightbox shows the (downscaled)
        // inline thumb until this lands.
        self.media.touch_image_full(&url);
        self.image_lightbox = Some(url);
    }

    /// Dismiss the image lightbox.
    pub fn close_image_lightbox(&mut self) {
        self.image_lightbox = None;
    }

    /// Open the message search overlay (freeq-android SearchSheet).
    pub fn open_message_search(&mut self) {
        self.show_members = false;
        self.close_react_picker();
        self.show_message_search = true;
        self.focus_message_search = true;
    }

    /// Dismiss the message search overlay and clear its query.
    pub fn close_message_search(&mut self) {
        self.show_message_search = false;
        self.message_search.clear();
        self.focus_message_search = false;
    }

    /// Clear the transient search-hit highlight.
    pub fn clear_search_highlight(&mut self) {
        self.highlight_msgid = None;
        self.highlight_until = None;
    }

    /// Expire a stale search highlight (call each frame from the chat UI).
    pub fn tick_search_highlight(&mut self) {
        if let Some(until) = self.highlight_until {
            if Instant::now() >= until {
                self.clear_search_highlight();
            }
        }
    }

    /// Open a buffer and scroll/highlight a specific message (search hit or reply original).
    pub fn navigate_to_message(&mut self, channel: &str, msgid: &str) {
        // `open_chat` clears scroll/highlight (list opens); re-apply after.
        self.close_message_search();
        self.open_chat(channel);
        if !msgid.is_empty() {
            self.scroll_to_msgid = Some(msgid.to_string());
            self.highlight_msgid = Some(msgid.to_string());
            self.highlight_until = Some(Instant::now() + SEARCH_HIGHLIGHT_DURATION);
        }
    }

    /// Search loaded chat/DM buffers for messages matching `query` (min 2 chars).
    ///
    /// Matches body text or sender nick (case-insensitive). Skips system and
    /// deleted messages. Newest hits first, capped at [`MESSAGE_SEARCH_LIMIT`].
    pub fn search_messages(&self, query: &str) -> Vec<MessageSearchHit> {
        let q = query.trim().to_lowercase();
        if q.chars().count() < 2 {
            return Vec::new();
        }
        let mut matches = Vec::new();
        for key in &self.channel_order {
            let Some(buf) = self.channels.get(key) else {
                continue;
            };
            if buf.name == "*status" {
                continue;
            }
            for msg in &buf.messages {
                if msg.from.is_empty() || msg.is_deleted || msg.is_system {
                    continue;
                }
                if msg.text.to_lowercase().contains(&q) || msg.from.to_lowercase().contains(&q) {
                    matches.push(MessageSearchHit {
                        // Prefer the HashMap key (DID for DMs) so navigate/open_chat
                        // lands in the same buffer search scanned.
                        channel: key.clone(),
                        message: msg.clone(),
                    });
                }
            }
        }
        matches.sort_by(|a, b| b.message.timestamp.cmp(&a.message.timestamp));
        matches.truncate(MESSAGE_SEARCH_LIMIT);
        matches
    }

    /// Open the emoji reaction picker dialog on `msgid` (clears any prior search).
    pub fn open_react_picker(&mut self, msgid: String) {
        self.react_picker_msg = Some(msgid);
        self.react_picker_search.clear();
    }

    /// Dismiss the emoji reaction picker dialog and reset its search field.
    pub fn close_react_picker(&mut self) {
        self.react_picker_msg = None;
        self.react_picker_search.clear();
    }

    /// Record `emoji` as recently used (MRU front, deduplicated, capped).
    pub fn push_recent_emoji(&mut self, emoji: &str) {
        push_mru(&mut self.recent_emoji, emoji, MAX_RECENT_EMOJI);
    }

    /// Open the channel policy join-gate modal and request a fresh check.
    pub fn open_policy_gate(&mut self, channel: &str) -> u64 {
        let ch = Self::normalize_channel(channel);
        self.policy_gate.open(ch);
        self.policy_gate.fetch_id
    }

    pub fn close_policy_gate(&mut self) {
        self.policy_gate.close();
    }

    /// Open the peer profile modal for `nick` (resolves DID from nick map when known).
    pub fn open_profile_gate(&mut self, nick: &str) -> u64 {
        let nick = nick.trim();
        if nick.is_empty() {
            return self.profile_gate.fetch_id;
        }
        let did = self.nick_to_did.get(&nick.to_lowercase()).cloned();
        self.close_react_picker();
        self.profile_gate.open(nick.to_string(), did);
        self.profile_gate.fetch_id
    }

    pub fn close_profile_gate(&mut self) {
        self.profile_gate.close();
    }

    /// Start editing an existing message in the compose bar.
    pub fn begin_edit(&mut self, msgid: String, text: String) {
        self.cancel_reply();
        self.editing_msgid = Some(msgid);
        self.compose = text;
        self.clear_compose_attach();
        self.compose_nick_tab.clear();
        self.focus_compose = true;
        self.close_react_picker();
    }

    /// Cancel an in-progress compose edit.
    pub fn cancel_edit(&mut self) {
        if self.editing_msgid.take().is_some() {
            self.compose.clear();
            self.compose_nick_tab.clear();
        }
    }

    /// Start replying to a message in the compose bar (slide-to-reply / menu).
    pub fn begin_reply(&mut self, msg: &ChatMessage) {
        if msg.is_system || msg.is_deleted || msg.id.is_empty() || msg.id.starts_with("local-") {
            return;
        }
        self.cancel_edit();
        self.clear_compose_attach();
        self.compose_nick_tab.clear();
        self.replying_to = Some(ReplyTarget {
            msgid: msg.id.clone(),
            from: msg.from.clone(),
            preview: msg.preview(),
        });
        self.focus_compose = true;
        self.close_react_picker();
    }

    /// Cancel an in-progress compose reply.
    pub fn cancel_reply(&mut self) {
        self.replying_to = None;
    }

    pub fn sorted_conversations(&self) -> Vec<&Buffer> {
        let mut list: Vec<&Buffer> = self
            .channel_order
            .iter()
            .filter_map(|k| self.channels.get(k))
            .filter(|b| b.name != "*status")
            .collect();
        if self.search.is_empty() {
            // keep all
        } else {
            let q = self.search.to_lowercase();
            list.retain(|b| {
                b.name.to_lowercase().contains(&q)
                    || self.display_name_for(&b.name).to_lowercase().contains(&q)
            });
        }
        list.sort_by(|a, b| b.last_activity.cmp(&a.last_activity).then(a.name.cmp(&b.name)));
        list
    }

    pub fn clear_session(&mut self) {
        self.connection = ConnectionState::Disconnected;
        self.awaiting_oauth = false;
        self.cancel_auto_reconnect();
        // Keep did / broker_token so reconnect can re-auth; use clear_auth for logout.
        self.channels.clear();
        self.channel_order.clear();
        self.nick_to_did.clear();
        self.did_to_nick.clear();
        self.avatars.clear();
        self.active_channel = None;
        self.route = Route::Tabs;
        self.tab = Tab::Chats;
        self.show_members = false;
        self.close_message_search();
        self.close_react_picker();
        self.close_profile_gate();
        self.scroll_to_msgid = None;
        self.clear_search_highlight();
        self.compose.clear();
        self.clear_compose_attach();
        self.compose_nick_tab.clear();
        self.cancel_edit();
        self.cancel_reply();
        self.local_call = None;
        self.clear_av_media();
        self.status_line = "Disconnected".into();
    }

    /// Soft disconnect for unexpected EOF / ping timeout: keep chat buffers so
    /// auto-reconnect can restore without wiping the conversation view.
    pub fn mark_disconnected_for_reconnect(&mut self, reason: &str) {
        self.connection = ConnectionState::Disconnected;
        self.awaiting_oauth = false;
        self.local_call = None;
        self.clear_av_media();
        self.cancel_media_reconnect();
        self.status_line = format!("Disconnected: {reason}");
    }

    pub fn cancel_auto_reconnect(&mut self) {
        self.reconnect_at = None;
        self.reconnect_attempts = 0;
    }

    /// Schedule a MoQ media re-dial while still in an IRC call.
    pub fn schedule_media_reconnect(&mut self) {
        // A long healthy Live means the next drop is a fresh failure.
        const STABLE: Duration = Duration::from_secs(8);
        if self
            .media_live_since
            .is_some_and(|t| t.elapsed() >= STABLE)
        {
            self.media_reconnect_attempts = 0;
        }
        self.media_live_since = None;
        self.media_reconnect_attempts = self.media_reconnect_attempts.saturating_add(1);
        let delay = crate::reconnect::delay_secs(self.media_reconnect_attempts);
        self.media_reconnect_at = Some(Instant::now() + Duration::from_secs(delay));
        log::info!(
            "av-media: scheduled MoQ re-dial in {delay}s (attempt {})",
            self.media_reconnect_attempts
        );
    }

    /// Stop a pending re-dial timer and reset backoff (call leave / stable end).
    pub fn cancel_media_reconnect(&mut self) {
        self.media_reconnect_at = None;
        self.media_reconnect_attempts = 0;
        self.media_live_since = None;
    }

    /// Live again — cancel any pending timer; keep attempt count so a 1s Live
    /// blip cannot reset backoff to 2s (that thrashed MoQ forever).
    pub fn on_media_live(&mut self) {
        self.media_reconnect_at = None;
        if self.media_live_since.is_none() {
            self.media_live_since = Some(Instant::now());
        }
    }

    /// Clear backoff after media has been Live long enough (called from UI tick).
    pub fn poll_media_live_stability(&mut self) {
        const STABLE: Duration = Duration::from_secs(8);
        if self.media_reconnect_attempts == 0 {
            return;
        }
        if self
            .media_live_since
            .is_some_and(|t| t.elapsed() >= STABLE)
        {
            self.media_reconnect_attempts = 0;
        }
    }

    /// True when we are mid MoQ recovery (suppress "connected" toast spam).
    pub fn media_recovering(&self) -> bool {
        self.media_reconnect_attempts > 0 || self.media_reconnect_at.is_some()
    }

    /// Schedule the next auto-reconnect attempt (exponential backoff).
    pub fn schedule_auto_reconnect(&mut self) {
        self.reconnect_attempts = self.reconnect_attempts.saturating_add(1);
        let delay = crate::reconnect::delay_secs(self.reconnect_attempts);
        self.reconnect_at = Some(Instant::now() + Duration::from_secs(delay));
        self.status_line = format!("Reconnecting in {delay}s…");
    }

    /// Drop MoQ video store, textures, mic meter, and focus (call ended or media stopped).
    pub fn clear_av_media(&mut self) {
        self.av_video = None;
        self.av_mic_level = None;
        self.av_video_textures.clear();
        self.av_focused_video = None;
        // Next call starts with a clean control strip (Devices collapsed).
        self.av_show_devices = false;
    }

    /// Full sign-out: disconnect buffers + wipe cached account / guest so the
    /// next connect is a fresh guest (no DID / broker_token / remembered nick).
    pub fn logout(&mut self) {
        self.clear_session();
        self.clear_auth();
        // Prefer a remembered guest nick for the form; otherwise a fresh default.
        // Don't leave form_nick as a Bluesky handle (e.g. nandi.uk) after clear.
        let nick = self
            .recent_nicks
            .first()
            .cloned()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(default_nick);
        self.nick = nick.clone();
        self.form_nick = nick;
        self.connect_mode = ConnectMode::Guest;
        self.auto_guest_connect = false;
        self.status_line = "Signed out — connect as guest or sign in again".into();
        self.show_toast("Cleared saved account");
    }

    pub fn normalize_channel(input: &str) -> String {
        let t = input.trim();
        if t.is_empty() {
            return String::new();
        }
        if t.starts_with('#') || t.starts_with('&') {
            t.to_string()
        } else {
            format!("#{t}")
        }
    }
}

/// Dedupe + normalize a loaded recent-channels list; ensure default lobby exists.
fn normalize_recent_channels(raw: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for ch in raw {
        let n = AppState::normalize_channel(&ch);
        if n.is_empty() || !(n.starts_with('#') || n.starts_with('&')) {
            continue;
        }
        if out.iter().any(|c| c.eq_ignore_ascii_case(&n)) {
            continue;
        }
        out.push(n);
    }
    if out.is_empty() {
        out.push("#test".into());
    }
    out
}

const MAX_RECENT_NICKS: usize = 8;
const MAX_RECENT_HANDLES: usize = 8;
pub const MAX_RECENT_EMOJI: usize = 16;

/// Insert `value` at the front of an MRU list (case-insensitive dedupe + cap).
fn push_mru(list: &mut Vec<String>, value: &str, max: usize) {
    let v = value.trim();
    if v.is_empty() {
        return;
    }
    list.retain(|x| !x.eq_ignore_ascii_case(v));
    list.insert(0, v.to_string());
    if list.len() > max {
        list.truncate(max);
    }
}

fn normalize_recent_nicks(raw: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for nick in raw {
        push_mru(&mut out, &nick, MAX_RECENT_NICKS);
    }
    out
}

fn normalize_bsky_handle(raw: &str) -> String {
    raw.trim().trim_start_matches('@').trim().to_string()
}

fn normalize_recent_handles(raw: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for handle in raw {
        push_mru(&mut out, &normalize_bsky_handle(&handle), MAX_RECENT_HANDLES);
    }
    out
}

/// Compact a DID for display: `did:plc:k2n3e2vsihf3farequ44t5j7` → `plc:k2n3…t5j7`.
fn shorten_did(s: &str) -> String {
    if !freeq_sdk::address::is_did(s) {
        return s.to_string();
    }
    let rest = &s[4..]; // after "did:"
    let Some(colon) = rest.find(':') else {
        return s.to_string();
    };
    let method = &rest[..colon];
    let id = &rest[colon + 1..];
    if id.len() <= 12 {
        format!("{method}:{id}")
    } else {
        let head: String = id.chars().take(4).collect();
        let tail: String = id
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("{method}:{head}…{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_emoji_strips_variation_selectors() {
        assert_eq!(display_emoji("❤️"), "❤");
        assert_eq!(display_emoji("👍"), "👍");
        assert_eq!(display_emoji("☺\u{FE0F}"), "☺");
        assert_eq!(display_emoji(DEFAULT_REACT_EMOJI), "❤");
        // Quick set: seven common reactions (no dance/music).
        assert_eq!(QUICK_REACT_EMOJIS.len(), 7);
        assert_eq!(QUICK_REACT_EMOJIS[1], "❤️");
    }

    #[test]
    fn emoji_picker_search_matches_name_and_shortcode() {
        let rocket = emojis::get("🚀").expect("rocket");
        assert!(emoji_matches_search(rocket, "rocket"));
        assert!(emoji_matches_search(rocket, "ROCKET"));
        assert!(emoji_matches_search(rocket, "🚀"));
        assert!(!emoji_matches_search(rocket, "banana"));

        // Smileys group is non-empty; red heart is available for reactions.
        assert!(EmojiPickerGroup::Smileys.emojis().count() > 50);
        let heart = emojis::get(DEFAULT_REACT_EMOJI).expect("default heart in unicode set");
        assert!(
            heart.as_str().contains('\u{2764}'),
            "heart glyph present: {:?}",
            heart.as_str()
        );
        assert!(emoji_matches_search(heart, "heart"));
        assert!(emoji_matches_search(heart, "red"));
    }

    #[test]
    fn parse_reactions_tag_canonical() {
        let m = parse_reactions_tag("👍:alice,bob;❤️:carol");
        assert_eq!(m.len(), 2);
        assert!(m["👍"].contains("alice"));
        assert!(m["👍"].contains("bob"));
        assert_eq!(m["❤️"], HashSet::from(["carol".into()]));
    }

    #[test]
    fn parse_reactions_tag_skips_junk() {
        let m = parse_reactions_tag("notacolon;:no_emoji;👍:;👎:dave");
        assert_eq!(m.len(), 1);
        assert_eq!(m["👎"], HashSet::from(["dave".into()]));
    }

    #[test]
    fn add_reaction_idempotent_and_case_insensitive() {
        let mut buf = Buffer::new("#t");
        buf.append(ChatMessage {
            id: "m1".into(),
            from: "alice".into(),
            text: "hi".into(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        });
        buf.add_reaction("m1", "👍", "Bob");
        buf.add_reaction("m1", "👍", "bob"); // same nick
        buf.add_reaction("m1", "", "carol"); // empty emoji ignored
        let msg = buf.messages.iter().find(|m| m.id == "m1").unwrap();
        assert_eq!(msg.reactions["👍"].len(), 1);
        assert!(msg.has_reaction_from("👍", "BOB"));
    }

    #[test]
    fn remove_reaction_drops_empty_emoji() {
        let mut buf = Buffer::new("#t");
        buf.append(ChatMessage {
            id: "m1".into(),
            from: "alice".into(),
            text: "hi".into(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        });
        buf.add_reaction("m1", "🎉", "me");
        buf.add_reaction("m1", "🎉", "other");
        buf.remove_reaction("m1", "🎉", "me");
        let msg = buf.messages.iter().find(|m| m.id == "m1").unwrap();
        assert_eq!(msg.reactions["🎉"], HashSet::from(["other".into()]));
        buf.remove_reaction("m1", "🎉", "other");
        let msg = buf.messages.iter().find(|m| m.id == "m1").unwrap();
        assert!(!msg.reactions.contains_key("🎉"));
    }

    fn sample_msg(id: &str, from: &str, text: &str, ts: DateTime<Local>) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            from: from.into(),
            text: text.into(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: ts,
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        }
    }

    #[test]
    fn message_search_matches_text_and_nick_newest_first() {
        let mut state = AppState::new();
        let t0 = Local::now() - chrono::Duration::minutes(2);
        let t1 = Local::now() - chrono::Duration::minutes(1);
        let t2 = Local::now();
        state.ensure_buffer("#a");
        state.ensure_buffer("#b");
        state.channels.get_mut("#a").unwrap().append(sample_msg("1", "alice", "hello world", t0));
        state.channels.get_mut("#a").unwrap().append(sample_msg("2", "bob", "nope", t1));
        state.channels.get_mut("#b").unwrap().append(sample_msg("3", "carol", "HELLO again", t2));
        state.channels.get_mut("#b").unwrap().append(ChatMessage {
            id: "sys".into(),
            from: String::new(),
            text: "hello system".into(),
            is_system: true,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: t2,
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        });

        assert!(state.search_messages("h").is_empty()); // min 2 chars
        let hits = state.search_messages("hello");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].message.id, "3"); // newest first
        assert_eq!(hits[1].message.id, "1");

        let nick_hits = state.search_messages("bob");
        assert_eq!(nick_hits.len(), 1);
        assert_eq!(nick_hits[0].message.id, "2");
    }

    #[test]
    fn open_chat_switches_to_chats_tab() {
        let mut state = AppState::new();
        state.tab = Tab::Discover;
        state.open_chat("#room");
        assert_eq!(state.tab, Tab::Chats);
        assert!(matches!(state.route, Route::Chat(ref c) if c == "#room"));
        assert_eq!(state.active_channel.as_deref(), Some("#room"));
    }

    #[test]
    fn navigate_to_message_opens_chat_and_sets_scroll() {
        let mut state = AppState::new();
        state.ensure_buffer("#room");
        state.navigate_to_message("#room", "msg-42");
        assert!(matches!(state.route, Route::Chat(ref c) if c == "#room"));
        assert!(!state.show_message_search);
        assert_eq!(state.scroll_to_msgid.as_deref(), Some("msg-42"));
        assert_eq!(state.highlight_msgid.as_deref(), Some("msg-42"));
        assert!(state.highlight_until.is_some());
    }

    #[test]
    fn chat_back_step_peels_overlays_then_signals_leave() {
        let mut state = AppState::new();
        state.open_chat("#room");
        state.open_image_lightbox("https://example.com/a.png".into());
        assert!(!state.chat_back_step());
        assert!(state.image_lightbox.is_none());

        state.open_profile_gate("alice");
        assert!(state.profile_gate.is_open());
        assert!(!state.chat_back_step());
        assert!(!state.profile_gate.is_open());

        state.open_react_picker("m1".into());
        assert!(!state.chat_back_step());
        assert!(state.react_picker_msg.is_none());

        state.open_message_search();
        assert!(!state.chat_back_step());
        assert!(!state.show_message_search);

        state.show_members = true;
        assert!(!state.chat_back_step());
        assert!(!state.show_members);

        state.begin_reply(&ChatMessage {
            id: "m1".into(),
            from: "alice".into(),
            text: "hi".into(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        });
        assert!(state.replying_to.is_some());
        assert!(!state.chat_back_step());
        assert!(state.replying_to.is_none());

        state.begin_edit("m1".into(), "hi".into());
        assert!(state.editing_msgid.is_some());
        assert!(!state.chat_back_step());
        assert!(state.editing_msgid.is_none());

        assert!(state.chat_back_step());
    }

    #[test]
    fn profile_gate_handle_nick_is_bluesky_actor_without_did() {
        let mut gate = ProfileGate::default();
        gate.open("chadfowler.com".into(), None);
        assert!(!gate.loading);
        assert_eq!(gate.bluesky_actor(), Some("chadfowler.com"));
        gate.whois_lines
            .push("chadfowler.com: No such nick".into());
        assert!(gate.whois_exhausted());
    }

    #[test]
    fn profile_gate_plain_nick_needs_did_for_bluesky() {
        let mut gate = ProfileGate::default();
        gate.open("alice".into(), None);
        assert!(gate.bluesky_actor().is_none());
        gate.did = Some("did:plc:abc".into());
        assert_eq!(gate.bluesky_actor(), Some("did:plc:abc"));
    }

    #[test]
    fn apply_edit_rewrites_in_place_and_delete_matches_root() {
        let mut buf = Buffer::new("#t");
        buf.append(ChatMessage {
            id: "orig".into(),
            from: "alice".into(),
            text: "v1".into(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        });
        assert!(buf.apply_edit("alice", "orig", Some("edit1"), "v2", None, None));
        let msg = buf.messages.front().unwrap();
        assert_eq!(msg.id, "edit1");
        assert_eq!(msg.text, "v2");
        assert!(msg.is_edited);
        assert_eq!(msg.edit_of.as_deref(), Some("orig"));
        // History replay of the pre-edit row is dropped.
        buf.append(ChatMessage {
            id: "orig".into(),
            from: "alice".into(),
            text: "v1".into(),
            is_system: false,
            is_action: false,
            is_edited: false,
            is_deleted: false,
            timestamp: Local::now(),
            reply_to: None,
            edit_of: None,
            is_signed: false,
            embed: None,
            link_meta: None,
            reactions: HashMap::new(),
        });
        assert_eq!(buf.messages.len(), 1);
        assert!(buf.apply_delete("alice", "orig"));
        assert!(buf.messages.front().unwrap().is_deleted);
    }

    #[test]
    fn cancel_compose_upload_unlocks_and_invalidates_id() {
        let mut state = AppState::new();
        state.compose_upload_id = 7;
        state.compose_uploading = true;
        state.compose_upload_started = Some(Instant::now());
        state.compose_attach = Some(ComposeAttach::Image(ComposeImage::from_rgba(
            1,
            1,
            std::sync::Arc::from([0, 0, 0, 255]),
        )));

        let before = state.compose_upload_id;
        state.cancel_compose_upload();
        assert!(!state.compose_uploading);
        assert!(state.compose_upload_started.is_none());
        assert_ne!(state.compose_upload_id, before);
        // Attachment kept so the user can retry after Cancel upload.
        assert!(state.compose_attach.is_some());

        state.clear_compose_attach();
        assert!(state.compose_attach.is_none());
        assert!(!state.compose_uploading);
    }

    #[test]
    fn remember_bsky_handle_normalizes_and_dedupes() {
        let mut state = AppState::new();
        state.recent_handles.clear();
        state.remember_bsky_handle("@alice.bsky.social");
        assert_eq!(state.recent_handles, vec!["alice.bsky.social".to_string()]);
        state.remember_bsky_handle("  alice.bsky.social ");
        assert_eq!(state.recent_handles, vec!["alice.bsky.social".to_string()]);
        state.remember_bsky_handle("bob.bsky.social");
        assert_eq!(
            state.recent_handles,
            vec![
                "bob.bsky.social".to_string(),
                "alice.bsky.social".to_string(),
            ]
        );
    }

    #[test]
    fn effective_bsky_handle_falls_back_to_form_handle() {
        let mut state = AppState::new();
        state.handle = None;
        state.form_handle = "typed.bsky.social".into();
        assert_eq!(state.effective_bsky_handle(), "typed.bsky.social");
        state.handle = Some("broker.bsky.social".into());
        assert_eq!(state.effective_bsky_handle(), "broker.bsky.social");
    }

    #[test]
    fn persist_session_uses_form_handle_when_broker_omits_handle() {
        let mut state = AppState::new();
        state.recent_handles.clear();
        state.broker_token = Some("bt".into());
        state.did = Some("did:plc:test".into());
        state.nick = "alice".into();
        state.form_handle = "alice.bsky.social".into();
        state.persist_session();
        assert_eq!(
            state.recent_handles.first().map(String::as_str),
            Some("alice.bsky.social")
        );
        assert_eq!(state.effective_bsky_handle(), "alice.bsky.social");
    }
}
