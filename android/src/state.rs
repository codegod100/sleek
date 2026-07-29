//! Application state — channel buffers, connection, and freeq-inspired models.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Local, Utc};

/// Max messages retained per buffer.
const MAX_MESSAGES: usize = 500;

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
            Tab::Discover => "Find",
            Tab::Settings => "More",
        }
    }
}

/// Navigation within the connected shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Tabs,
    Chat(String),
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
    pub is_signed: bool,
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
            is_signed: false,
        }
    }

    pub fn time_label(&self) -> String {
        self.timestamp.format("%H:%M").to_string()
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
        if self.from.is_empty() {
            self.text.clone()
        } else {
            format!("{}: {}", self.from, self.text)
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
    message_ids: HashSet<String>,
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
            message_ids: HashSet::new(),
        }
    }

    pub fn is_channel(&self) -> bool {
        self.name.starts_with('#') || self.name.starts_with('&')
    }

    pub fn display_name(&self) -> &str {
        self.name.as_str()
    }

    pub fn last_preview(&self) -> String {
        self.messages
            .iter()
            .rev()
            .find(|m| !m.is_system || m.from.is_empty())
            .or_else(|| self.messages.back())
            .map(|m| m.preview())
            .unwrap_or_else(|| "No messages yet".into())
    }

    pub fn append(&mut self, msg: ChatMessage) {
        if !msg.id.is_empty() && !self.message_ids.insert(msg.id.clone()) {
            return;
        }
        let ts = msg.timestamp.timestamp();
        if !msg.is_system && ts > self.last_activity {
            self.last_activity = ts;
        }
        self.messages.push_back(msg);
        while self.messages.len() > MAX_MESSAGES {
            if let Some(old) = self.messages.pop_front() {
                self.message_ids.remove(&old.id);
            }
        }
    }

    pub fn mark_read(&mut self) {
        self.unread = 0;
    }
}

/// Popular channels shown on Discover (mirrors freeq-android).
pub const POPULAR_CHANNELS: &[(&str, &str)] = &[
    ("#general", "General discussion"),
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
    pub status_line: String,
    /// True while the loopback browser OAuth flow is waiting.
    pub awaiting_oauth: bool,

    pub channels: HashMap<String, Buffer>,
    /// Ordered keys for list UI (channel names / dm keys).
    pub channel_order: Vec<String>,
    pub active_channel: Option<String>,
    pub route: Route,
    pub tab: Tab,

    pub compose: String,
    pub search: String,
    pub join_input: String,
    pub discover_input: String,

    /// Connect form fields (editable while disconnected).
    pub connect_mode: ConnectMode,
    pub form_handle: String,
    pub form_callback: String,
    pub form_nick: String,
    pub form_server: String,
    pub form_tls: bool,
    pub form_websocket: bool,
}

impl AppState {
    pub fn new() -> Self {
        let nick = default_nick();
        let server = default_server();
        let use_ws = cfg!(target_os = "android");
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
            status_line: "Not connected".into(),
            awaiting_oauth: false,
            channels: HashMap::new(),
            channel_order: Vec::new(),
            active_channel: None,
            route: Route::Tabs,
            tab: Tab::Chats,
            compose: String::new(),
            search: String::new(),
            join_input: String::new(),
            discover_input: String::new(),
            connect_mode: ConnectMode::Bluesky,
            form_handle: String::new(),
            form_callback: String::new(),
            form_nick: nick,
            form_server: server,
            form_tls: true,
            form_websocket: use_ws,
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
                }
            }
        }
        state
    }

    pub fn has_saved_session(&self) -> bool {
        self.broker_token
            .as_ref()
            .is_some_and(|t| !t.is_empty())
    }

    pub fn persist_session(&self) {
        let Some(broker_token) = self.broker_token.clone() else {
            return;
        };
        // Never poison storage with a Guest temp nick while DID-authenticated
        // (freeq-android AuthRecoveryTest).
        let nick = if self.did.is_some() && self.nick.starts_with("Guest") {
            self.handle.clone().unwrap_or_default()
        } else {
            self.nick.clone()
        };
        let session = crate::auth::SavedSession {
            broker_token,
            did: self.did.clone().unwrap_or_default(),
            handle: self.handle.clone().unwrap_or_default(),
            nick,
            server: self.server.clone(),
            last_login_unix: chrono::Utc::now().timestamp(),
        };
        if let Err(e) = session.save() {
            log::warn!("failed to save session: {e}");
        }
    }

    pub fn clear_auth(&mut self) {
        self.broker_token = None;
        self.did = None;
        self.handle = None;
        crate::auth::SavedSession::clear();
    }

    pub fn is_connected(&self) -> bool {
        self.connection == ConnectionState::Registered
            || self.connection == ConnectionState::Connected
    }

    pub fn total_unread(&self) -> u32 {
        self.channels.values().map(|b| b.unread).sum()
    }

    pub fn ensure_buffer(&mut self, name: &str) -> &mut Buffer {
        let key = name.to_string();
        if !self.channels.contains_key(&key) {
            self.channels.insert(key.clone(), Buffer::new(&key));
            if !self.channel_order.iter().any(|n| n.eq_ignore_ascii_case(&key)) {
                self.channel_order.push(key.clone());
            }
        }
        self.channels.get_mut(&key).expect("just inserted")
    }

    pub fn open_chat(&mut self, name: &str) {
        self.ensure_buffer(name);
        self.active_channel = Some(name.to_string());
        self.route = Route::Chat(name.to_string());
        if let Some(buf) = self.channels.get_mut(name) {
            buf.mark_read();
        }
    }

    pub fn close_chat(&mut self) {
        self.active_channel = None;
        self.route = Route::Tabs;
        self.tab = Tab::Chats;
    }

    pub fn sorted_conversations(&self) -> Vec<&Buffer> {
        let mut list: Vec<&Buffer> = self
            .channel_order
            .iter()
            .filter_map(|k| self.channels.get(k))
            .collect();
        if self.search.is_empty() {
            // keep all
        } else {
            let q = self.search.to_lowercase();
            list.retain(|b| b.name.to_lowercase().contains(&q));
        }
        list.sort_by(|a, b| b.last_activity.cmp(&a.last_activity).then(a.name.cmp(&b.name)));
        list
    }

    pub fn clear_session(&mut self) {
        self.connection = ConnectionState::Disconnected;
        self.awaiting_oauth = false;
        // Keep did / broker_token so reconnect can re-auth; use clear_auth for logout.
        self.channels.clear();
        self.channel_order.clear();
        self.active_channel = None;
        self.route = Route::Tabs;
        self.tab = Tab::Chats;
        self.compose.clear();
        self.status_line = "Disconnected".into();
    }

    pub fn logout(&mut self) {
        self.clear_session();
        self.clear_auth();
        self.status_line = "Signed out".into();
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
