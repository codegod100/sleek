//! freeq AV call helpers — signaling state + SFU URL construction.
//!
//! Call *signaling* rides IRC TAGMSGs (`freeq_sdk::av`). Media rides MoQ
//! (Media over QUIC) through the freeq SFU at `{origin}/av/moq`.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use freeq_sdk::av::{AvAction, AvState};

/// Active call on a channel (from `+freeq.at/av-state` broadcasts).
#[derive(Debug, Clone, Default)]
pub struct ChannelCall {
    pub session_id: String,
    pub title: Option<String>,
    pub participants: u32,
    pub last_actor: Option<String>,
}

/// Our participation in a call (local device state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalCall {
    pub channel: String,
    pub session_id: String,
    pub instance: String,
    /// SFU JWT from `+freeq.at/av-token` (if the server minted one).
    pub token: Option<String>,
    pub muted: bool,
    /// Camera publish enabled (desktop MoQ). False when no camera / toggled off.
    pub camera: bool,
    /// True when the media plane opened a capture device (desktop).
    pub has_camera: bool,
    pub media: MediaStatus,
    /// True after we sent av-start and are waiting for av-state=started.
    pub awaiting_start: bool,
}

/// One decoded remote video frame (RGBA8), shared UI ↔ media task.
#[derive(Clone)]
pub struct RgbaVideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

impl fmt::Debug for RgbaVideoFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RgbaVideoFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba_len", &self.rgba.len())
            .finish()
    }
}

/// Latest-frame map for remote participants (and optional local preview).
///
/// The media task writes; the UI thread snapshots for texture upload.
/// Intermediate frames are dropped so a 30 fps encode path cannot flood egui.
#[derive(Clone, Default)]
pub struct VideoFrameStore {
    frames: Arc<Mutex<HashMap<String, RgbaVideoFrame>>>,
}

impl fmt::Debug for VideoFrameStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.frames.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("VideoFrameStore")
            .field("len", &n)
            .finish()
    }
}

impl VideoFrameStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, nick: impl Into<String>, width: u32, height: u32, rgba: Arc<[u8]>) {
        if width == 0 || height == 0 {
            return;
        }
        let expected = (width as usize).saturating_mul(height as usize).saturating_mul(4);
        if rgba.len() != expected {
            return;
        }
        if let Ok(mut g) = self.frames.lock() {
            g.insert(
                nick.into(),
                RgbaVideoFrame {
                    width,
                    height,
                    rgba,
                },
            );
        }
    }

    pub fn remove(&self, nick: &str) {
        if let Ok(mut g) = self.frames.lock() {
            g.remove(nick);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut g) = self.frames.lock() {
            g.clear();
        }
    }

    /// Snapshot of all latest frames (for UI paint).
    pub fn snapshot(&self) -> Vec<(String, RgbaVideoFrame)> {
        self.frames
            .lock()
            .map(|g| g.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.lock().map(|g| g.is_empty()).unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaStatus {
    /// Not dialing the SFU yet (or signaling-only platforms).
    Idle,
    /// MoQ connect in flight.
    Connecting,
    /// Publishing / receiving media.
    Live,
    /// Connect failed; call signaling may still be active.
    Failed(String),
    /// Legacy: native MoQ was unavailable (pre-Android media plane).
    /// Kept for status matching; no longer emitted by the dial path.
    BrowserOnly,
}

impl MediaStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Idle => "Not connected".into(),
            Self::Connecting => "Connecting media…".into(),
            Self::Live => "In call".into(),
            Self::Failed(e) => format!("Media: {e}"),
            Self::BrowserOnly => "Open in browser for media".into(),
        }
    }
}

/// Parse the winning session id from a start-collision reason string.
///
/// freeq formats these as:
/// `Channel #test already has an active session: 01KYRJEW9XCSYB2T10HM9RE1VN`
/// Prefer `+freeq.at/av-id` when present; this is a belt-and-suspenders fallback.
pub fn session_id_from_collision_reason(reason: &str) -> Option<String> {
    const MARKER: &str = "already has an active session:";
    let idx = reason.to_ascii_lowercase().find(MARKER)?;
    // Use original slice with same index (marker is ASCII).
    let rest = reason.get(idx + MARKER.len()..)?.trim();
    let id = rest.split_whitespace().next()?.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Parse the channel name from a start-collision reason (`Channel #foo already…`).
pub fn channel_from_collision_reason(reason: &str) -> Option<String> {
    let rest = reason
        .strip_prefix("Channel ")
        .or_else(|| reason.strip_prefix("channel "))?;
    let ch = rest.split_whitespace().next()?.trim();
    if ch.starts_with('#') || ch.starts_with('&') {
        Some(ch.to_string())
    } else {
        None
    }
}

/// Apply a parsed av-state to channel call state. Returns `true` if the call ended.
pub fn apply_av_state(call: &mut Option<ChannelCall>, st: &AvState) -> bool {
    match st.action {
        AvAction::Ended => {
            *call = None;
            true
        }
        AvAction::Started | AvAction::Joined | AvAction::Left => {
            let mut c = call.take().unwrap_or_default();
            c.session_id = st.session_id.clone();
            if let Some(n) = st.participants {
                c.participants = n;
            } else if st.action == AvAction::Started && c.participants == 0 {
                c.participants = 1;
            }
            if st.actor.is_some() {
                c.last_actor = st.actor.clone();
            }
            if st.title.is_some() {
                c.title = st.title.clone();
            }
            *call = Some(c);
            false
        }
    }
}

/// Human-readable system line for an av-state change.
pub fn av_state_message(st: &AvState) -> String {
    let who = st.actor.as_deref().unwrap_or("someone");
    let n = st
        .participants
        .map(|p| format!(" · {p} in call"))
        .unwrap_or_default();
    match st.action {
        AvAction::Started => {
            let title = st
                .title
                .as_ref()
                .filter(|t| !t.is_empty())
                .map(|t| format!(" “{t}”"))
                .unwrap_or_default();
            format!("Call started by {who}{title}{n}")
        }
        AvAction::Joined => format!("{who} joined the call{n}"),
        AvAction::Left => format!("{who} left the call{n}"),
        AvAction::Ended => format!("Call ended{n}"),
    }
}

/// Build the MoQ SFU dial URL from the IRC server host.
///
/// Examples:
/// - `irc.freeq.at:6697` → `https://irc.freeq.at/av/moq`
/// - `wss://irc.freeq.at/irc` → `https://irc.freeq.at/av/moq`
///
/// When `jwt` is set, appends `?jwt=…` (required when the SFU enforces tokens).
pub fn sfu_moq_url(server: &str, jwt: Option<&str>) -> Result<url::Url, String> {
    let trimmed = server.trim();
    if trimmed.is_empty() {
        return Err("server is empty".into());
    }
    let normalized = if trimmed.starts_with("ws://")
        || trimmed.starts_with("wss://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        trimmed.to_string()
    } else {
        // host:port from the connect form — freeq public hosts are TLS.
        let host = trimmed.split(':').next().unwrap_or(trimmed);
        if host.eq_ignore_ascii_case("localhost") || host.starts_with("127.") {
            format!("http://{trimmed}")
        } else {
            format!("https://{host}")
        }
    };
    let mut u: url::Url = normalized
        .parse()
        .map_err(|e| format!("parse server for SFU: {e}"))?;
    match u.scheme() {
        "https" | "wss" => {
            u.set_scheme("https").ok();
        }
        "http" | "ws" => {
            u.set_scheme("http").ok();
        }
        other => return Err(format!("unsupported scheme for SFU: {other}")),
    }
    if u.host_str().map(|h| h.is_empty()).unwrap_or(true) {
        return Err("server URL has no host".into());
    }
    u.set_path("/av/moq");
    u.set_query(None);
    if let Some(tok) = jwt.filter(|t| !t.is_empty()) {
        u.set_query(Some(&format!("jwt={tok}")));
    }
    Ok(u)
}

/// Whether we should dial the MoQ SFU now.
///
/// Production freeq SFUs require `?jwt=…`. Dialing without a token opens a
/// connection that the SFU immediately closes — moq-lite then floods
/// `transport error err=connection closed` once per live stream. Local/dev
/// SFUs (localhost / 127.*) are allowed without a JWT.
pub fn can_dial_sfu(server: &str, jwt: Option<&str>) -> bool {
    if jwt.map(|t| !t.is_empty()).unwrap_or(false) {
        return true;
    }
    let trimmed = server.trim();
    let host = if let Ok(u) = url::Url::parse(trimmed) {
        u.host_str().unwrap_or("").to_string()
    } else if trimmed.starts_with("ws://")
        || trimmed.starts_with("wss://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        // Malformed absolute URL — treat as remote (need JWT).
        String::new()
    } else {
        trimmed
            .split('/')
            .next()
            .unwrap_or(trimmed)
            .split(':')
            .next()
            .unwrap_or(trimmed)
            .to_string()
    };
    host.eq_ignore_ascii_case("localhost") || host.starts_with("127.")
}

/// MoQ broadcast path: `{session}/{nick}~{instance}`.
pub fn broadcast_path(session_id: &str, nick: &str, instance: &str) -> String {
    if instance.is_empty() {
        format!("{session_id}/{nick}")
    } else {
        format!("{session_id}/{nick}~{instance}")
    }
}

/// Display nick from a broadcast path.
pub fn path_nick(path: &str) -> &str {
    let last = path.rsplit('/').next().unwrap_or(path);
    last.split('~').next().unwrap_or(last)
}

/// Whether an announced path belongs to this session and is not us.
pub fn should_tap(path: &str, session_id: &str, our_broadcast: &str, my_nick: &str) -> bool {
    if path == our_broadcast {
        return false;
    }
    // Session prefix filter (unscoped SFU announces everything).
    let prefix = format!("{session_id}/");
    if !path.starts_with(&prefix) && path != session_id {
        // Also accept relative paths when scoped (`nick~inst` only).
        if path.contains('/') {
            return false;
        }
    }
    let nick = path_nick(path);
    if nick.eq_ignore_ascii_case(my_nick) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{
        can_dial_sfu, channel_from_collision_reason, session_id_from_collision_reason,
    };

    #[test]
    fn collision_reason_parses_session_and_channel() {
        let reason = "Channel #test already has an active session: 01KYRJEW9XCSYB2T10HM9RE1VN";
        assert_eq!(
            session_id_from_collision_reason(reason).as_deref(),
            Some("01KYRJEW9XCSYB2T10HM9RE1VN")
        );
        assert_eq!(
            channel_from_collision_reason(reason).as_deref(),
            Some("#test")
        );
    }

    #[test]
    fn collision_reason_unknown_shape() {
        assert!(session_id_from_collision_reason("nope").is_none());
        assert!(channel_from_collision_reason("busy").is_none());
    }

    #[test]
    fn can_dial_sfu_remote_without_jwt_refused() {
        assert!(!can_dial_sfu("wss://chat.example.com", None));
        assert!(!can_dial_sfu("wss://chat.example.com", Some("")));
        assert!(!can_dial_sfu("https://freeq.example/irc", None));
        assert!(!can_dial_sfu("chat.example.com:8443", None));
    }

    #[test]
    fn can_dial_sfu_localhost_without_jwt_allowed() {
        assert!(can_dial_sfu("ws://localhost:4443", None));
        assert!(can_dial_sfu("http://localhost/av", None));
        assert!(can_dial_sfu("localhost", None));
        assert!(can_dial_sfu("ws://127.0.0.1:4443", None));
        assert!(can_dial_sfu("http://127.0.0.1", None));
        assert!(can_dial_sfu("127.0.0.1:8080", None));
    }

    #[test]
    fn can_dial_sfu_with_jwt_always_allowed() {
        assert!(can_dial_sfu("wss://chat.example.com", Some("eyJhbGciOiJIUzI1NiJ9.e30.x")));
        assert!(can_dial_sfu("https://remote.example", Some("tok")));
        assert!(can_dial_sfu("ws://localhost:4443", Some("tok")));
        assert!(can_dial_sfu("127.0.0.1", Some("tok")));
    }
}
