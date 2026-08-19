//! freeq auth-broker login (mirrors freeq-android).
//!
//! Flow:
//! 1. **Desktop:** open `{auth_broker}/auth/login?handle=…&return_to=http://127.0.0.1:PORT`
//!    and capture the `#oauth=` handoff on loopback.
//!    **Android:** open `…&mobile=1` so the broker redirects to `freeq://auth?…`;
//!    `SleekActivity` (intent-filter on scheme `freeq`, `singleTask`) delivers the
//!    URI via JNI — no paste, no manual task switch.
//! 2. Broker issues a short-lived SASL `web-token` + durable `broker_token`.
//! 3. Connect to IRC with `ConnectConfig.web_token` (SASL ATPROTO-CHALLENGE / web-token).
//! 4. On reconnect, `POST {auth_broker}/session` with `broker_token` mints a fresh web-token.
//!
//! The OAuth-issued web-token is **single-use** and must not be cached across
//! reconnects (freeq-android AuthRecoveryTest). Only tokens from `/session` are
//! cacheable; we skip that cache entirely and always hit `/session` when needed.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Production auth broker (same default as freeq-android `AUTH_BROKER_BASE`).
pub const DEFAULT_AUTH_BROKER: &str = "https://auth.freeq.at";

/// Production IRC server host:port.
pub const DEFAULT_IRC_SERVER: &str = "irc.freeq.at:6697";

/// Result of a successful broker login (or `/session` refresh).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    /// One-shot SASL web-token (do not persist for reconnect).
    pub token: String,
    /// Long-lived broker session handle — persist this.
    pub broker_token: String,
    pub nick: String,
    pub did: String,
    #[serde(default)]
    pub handle: String,
}

fn default_true() -> bool {
    true
}

fn default_recent_channels() -> Vec<String> {
    // Lobby + test room so guest / first-run channel lists are useful out of the box.
    vec!["#general".into(), "#test".into()]
}

/// Disk-persisted app preferences (independent of auth session).
///
/// Survives logout / guest clear so mic/camera intent and recently visited
/// rooms stay across launches. Channel membership is client-side because the
/// server does not reliably restore joins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPrefs {
    /// Join / start next call with mic muted.
    #[serde(default)]
    pub av_pref_muted: bool,
    /// Join / start next call with speaker (remote audio) muted.
    #[serde(default)]
    pub av_pref_speaker_muted: bool,
    /// Publish camera when hardware is available (desktop MoQ).
    #[serde(default = "default_true")]
    pub av_pref_camera: bool,
    /// Preferred camera id (`CameraInfo.id`). Empty / missing = first available.
    #[serde(default)]
    pub av_pref_camera_id: Option<String>,
    /// Preferred microphone name. Empty / missing = system default.
    #[serde(default)]
    pub av_pref_mic_id: Option<String>,
    /// Preferred speaker name. Empty / missing = system default.
    #[serde(default)]
    pub av_pref_speaker_id: Option<String>,
    /// Channels the user has visited / joined — auto-rejoined on connect.
    /// MRU order (most recent first). Defaults: `#general`, `#test`.
    #[serde(default = "default_recent_channels")]
    pub recent_channels: Vec<String>,
    /// Most recently used Bluesky handle (survives logout, pre-filled on connect).
    #[serde(default)]
    pub last_bsky_handle: Option<String>,
    /// Previously used Bluesky handles — MRU, shown on the connect screen.
    /// `last_bsky_handle` stays as the primary prefill (first entry when present).
    #[serde(default)]
    pub recent_handles: Vec<String>,
    /// Previously used guest nicknames — MRU, shown on the guest connect screen.
    #[serde(default)]
    pub recent_nicks: Vec<String>,
    /// When false, JOIN / PART / QUIT presence lines are not appended to chat.
    /// Member lists still update. Defaults to true (show).
    #[serde(default = "default_true")]
    pub show_join_part: bool,
    /// Recently used reaction emoji (MRU first). Persisted; shown at the top of
    /// the reaction picker so frequently-used emoji are immediately accessible.
    #[serde(default)]
    pub recent_emoji: Vec<String>,
}

impl Default for SavedPrefs {
    fn default() -> Self {
        Self {
            av_pref_muted: false,
            av_pref_speaker_muted: false,
            av_pref_camera: true,
            av_pref_camera_id: None,
            av_pref_mic_id: None,
            av_pref_speaker_id: None,
            recent_channels: default_recent_channels(),
            last_bsky_handle: None,
            recent_handles: Vec::new(),
            recent_nicks: Vec::new(),
            show_join_part: true,
            recent_emoji: Vec::new(),
        }
    }
}

/// Writable app config directory (`~/.config/sleek` on desktop; app files on Android).
pub(crate) fn storage_dir() -> PathBuf {
    #[cfg(target_os = "android")]
    {
        if let Some(dir) = crate::android_media::app_storage_dir() {
            return dir;
        }
        log::warn!("android: app storage dir unavailable; prefs may not persist");
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("sleek")
}

#[cfg(target_os = "android")]
fn android_legacy_storage_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(".").join("sleek")];
    if let Some(app) = crate::android_media::android_app_handle() {
        if let Some(ext) = app.external_data_path() {
            let ext_root = ext.join("sleek");
            if !roots.iter().any(|r| r == &ext_root) {
                roots.push(ext_root);
            }
        }
    }
    roots
}

/// Before #42, Android wrote prefs/session to the process cwd (`./sleek/`) because
/// `dirs::config_dir()` is unavailable. Migrate those files into the app files dir.
#[cfg(target_os = "android")]
fn migrate_legacy_android_file(legacy: &Path, target: &Path) -> bool {
    migrate_storage_file(legacy, target)
}

/// Move or copy `legacy` to `target` when the target does not exist yet.
fn migrate_storage_file(legacy: &Path, target: &Path) -> bool {
    if target.exists() || !legacy.exists() {
        return false;
    }
    if let Some(parent) = target.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    match std::fs::rename(legacy, target) {
        Ok(()) => true,
        Err(_) => {
            if std::fs::copy(legacy, target).is_ok() {
                let _ = std::fs::remove_file(legacy);
                true
            } else {
                false
            }
        }
    }
}

#[cfg(target_os = "android")]
fn ensure_android_storage_migrated(file_name: &str) {
    let target = storage_dir().join(file_name);
    for root in android_legacy_storage_roots() {
        let legacy = root.join(file_name);
        if legacy == target {
            continue;
        }
        if migrate_legacy_android_file(&legacy, &target) {
            log::info!(
                "android: migrated {} -> {}",
                legacy.display(),
                target.display()
            );
            break;
        }
    }
}

impl SavedPrefs {
    pub fn path() -> PathBuf {
        storage_dir().join("prefs.json")
    }

    pub fn load() -> Self {
        #[cfg(target_os = "android")]
        ensure_android_storage_migrated("prefs.json");

        let mut prefs = load_prefs(&Self::path()).unwrap_or_default();
        // Drop virtual camera prefs by name/id substring (OBS / loopback).
        // Device paths like `/dev/video10` are scrubbed at dial time once we
        // can match against the live camera list.
        if let Some(id) = prefs.av_pref_camera_id.as_deref() {
            let s = id.to_ascii_lowercase();
            if s.contains("virtual")
                || s.contains("obs")
                || s.contains("loopback")
                || s.contains("v4l2loopback")
            {
                log::info!("prefs: clearing virtual camera id {id:?}");
                prefs.av_pref_camera_id = None;
                let _ = prefs.save();
            }
        }
        // Migrate singular last_bsky_handle into the MRU list when needed.
        if let Some(handle) = prefs.last_bsky_handle.clone() {
            if !handle.is_empty()
                && !prefs
                    .recent_handles
                    .iter()
                    .any(|h| h.eq_ignore_ascii_case(&handle))
            {
                prefs.recent_handles.insert(0, handle);
            }
        } else if let Some(first) = prefs.recent_handles.first().cloned() {
            prefs.last_bsky_handle = Some(first);
        }
        prefs
    }

    pub fn save(&self) -> Result<()> {
        save_prefs(&Self::path(), self)
    }
}

fn load_prefs(path: &Path) -> Option<SavedPrefs> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_prefs(path: &Path, prefs: &SavedPrefs) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(prefs)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Disk-persisted session (no single-use web-token).
///
/// Two shapes:
/// - **Bluesky**: non-empty `broker_token` (+ did/handle/nick).
/// - **Guest**: `guest: true`, empty broker/did, remembered nick + server for
///   auto-reconnect on next launch.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedSession {
    #[serde(default)]
    pub broker_token: String,
    #[serde(default)]
    pub did: String,
    #[serde(default)]
    pub handle: String,
    #[serde(default)]
    pub nick: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub last_login_unix: i64,
    /// Last successful connect was guest (no SASL). Used for auto guest login.
    #[serde(default)]
    pub guest: bool,
    #[serde(default = "default_true")]
    pub use_tls: bool,
    #[serde(default)]
    pub use_websocket: bool,
}

impl SavedSession {
    pub fn path() -> PathBuf {
        storage_dir().join("session.json")
    }

    pub fn load() -> Option<Self> {
        #[cfg(target_os = "android")]
        ensure_android_storage_migrated("session.json");

        load_session(&Self::path())
    }

    pub fn save(&self) -> Result<()> {
        save_session(&Self::path(), self)
    }

    pub fn clear() {
        let p = Self::path();
        let _ = std::fs::remove_file(p);
    }

    /// Durable Bluesky / auth-broker session.
    pub fn has_session(&self) -> bool {
        !self.broker_token.is_empty()
    }

    /// Remembered guest nick for auto-connect (no broker token).
    pub fn has_guest(&self) -> bool {
        self.guest && !self.nick.is_empty() && self.broker_token.is_empty()
    }
}

pub fn load_session(path: &Path) -> Option<SavedSession> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_session(path: &Path, session: &SavedSession) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(session)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Build the Bluesky/ATProto broker login URL.
///
/// `return_to` should be a loopback origin allowed by the broker
/// (`http://127.0.0.1` / `http://localhost`, any port). When `None`, use
/// `mobile=1` so the broker redirects to `freeq://auth?…` (pasteable).
pub fn login_url(auth_broker: &str, handle: &str, return_to: Option<&str>) -> String {
    let base = auth_broker.trim_end_matches('/');
    let handle = handle.trim().trim_start_matches('@');
    let encoded = urlencoding::encode(handle);
    match return_to {
        Some(rt) => {
            let rt_enc = urlencoding::encode(rt);
            format!("{base}/auth/login?handle={encoded}&return_to={rt_enc}")
        }
        None => format!("{base}/auth/login?handle={encoded}&mobile=1"),
    }
}

/// Parse a `freeq://auth?token=…&broker_token=…&nick=…&did=…&handle=…` callback.
pub fn parse_freeq_auth_url(url: &str) -> Result<AuthTokens> {
    let url = url.trim();
    let rest = url
        .strip_prefix("freeq://auth?")
        .or_else(|| url.strip_prefix("freeq://auth/?"))
        .or_else(|| {
            // Also accept bare query strings pasted from the browser.
            if url.contains("token=") && url.contains("broker_token=") {
                Some(url.trim_start_matches('?'))
            } else {
                None
            }
        })
        .context("not a freeq://auth callback URL")?;

    let mut token = None;
    let mut broker_token = None;
    let mut nick = None;
    let mut did = None;
    let mut handle = String::new();

    for pair in rest.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("");
        let decoded = urlencoding::decode(val)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| val.to_string());
        match key {
            "token" => token = Some(decoded),
            "broker_token" => broker_token = Some(decoded),
            "nick" => nick = Some(decoded),
            "did" => did = Some(decoded),
            "handle" => handle = decoded,
            "error" => bail!("OAuth error: {decoded}"),
            _ => {}
        }
    }

    Ok(AuthTokens {
        token: token.context("missing token")?,
        broker_token: broker_token.context("missing broker_token")?,
        nick: nick.context("missing nick")?,
        did: did.context("missing did")?,
        handle,
    })
}

/// Decode the base64url JSON payload from a `#oauth=` web redirect fragment.
pub fn parse_oauth_fragment_payload(b64: &str) -> Result<AuthTokens> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64.trim())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(b64.trim()))
        .context("invalid oauth fragment base64")?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).context("oauth JSON")?;
    Ok(AuthTokens {
        token: v
            .get("token")
            .and_then(|x| x.as_str())
            .context("missing token")?
            .to_string(),
        broker_token: v
            .get("broker_token")
            .and_then(|x| x.as_str())
            .context("missing broker_token")?
            .to_string(),
        nick: v
            .get("nick")
            .and_then(|x| x.as_str())
            .context("missing nick")?
            .to_string(),
        did: v
            .get("did")
            .and_then(|x| x.as_str())
            .context("missing did")?
            .to_string(),
        handle: v
            .get("handle")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// Mint a fresh web-token from a durable `broker_token` (freeq-android `/session`).
pub async fn fetch_broker_session(auth_broker: &str, broker_token: &str) -> Result<AuthTokens> {
    let base = auth_broker.trim_end_matches('/');
    let url = format!("{base}/session");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("reqwest client")?;

    // A few retries — DPoP nonce rotation / transient 502s (mirrors Android).
    let mut last_err = None;
    for attempt in 0..3u32 {
        let resp = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "broker_token": broker_token }))
            .send()
            .await;
        match resp {
            Ok(r) => {
                let status = r.status();
                if status.as_u16() == 502 && attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                    continue;
                }
                if status.as_u16() == 401 {
                    bail!("Session expired — please sign in again");
                }
                if !status.is_success() {
                    let body = r.text().await.unwrap_or_default();
                    bail!("Broker returned {status}: {body}");
                }
                let v: serde_json::Value = r.json().await.context("broker session JSON")?;
                return Ok(AuthTokens {
                    token: v
                        .get("token")
                        .and_then(|x| x.as_str())
                        .context("missing token")?
                        .to_string(),
                    broker_token: broker_token.to_string(),
                    nick: v
                        .get("nick")
                        .and_then(|x| x.as_str())
                        .context("missing nick")?
                        .to_string(),
                    did: v
                        .get("did")
                        .and_then(|x| x.as_str())
                        .context("missing did")?
                        .to_string(),
                    handle: v
                        .get("handle")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
            }
        }
    }
    Err(last_err
        .map(|e| anyhow::anyhow!(e))
        .unwrap_or_else(|| anyhow::anyhow!("broker /session failed")))
}

/// HTML served on loopback so the browser can post `#oauth=` (fragments never
/// reach the server on the initial GET).
const CAPTURE_HTML: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Sleek auth</title>
<style>
body{font-family:system-ui,sans-serif;background:#1a1a2e;color:#e8e8f0;
display:flex;align-items:center;justify-content:center;height:100vh;margin:0}
.box{text-align:center;max-width:28rem;padding:2rem}
h1{color:#8b7cf6;font-size:1.25rem} p{color:#a0a0b8;line-height:1.5}
</style></head>
<body><div class="box"><h1>Sleek</h1><p id="m">Finishing sign-in…</p></div>
<script>
(async () => {
  const m = document.getElementById('m');
  const h = location.hash || '';
  if (!h.startsWith('#oauth=')) {
    m.textContent = 'Missing oauth payload. Close this tab and try again in Sleek.';
    return;
  }
  const b64 = h.slice(7);
  try {
    const r = await fetch('/capture', { method: 'POST', body: b64 });
    if (!r.ok) throw new Error(await r.text());
    m.textContent = 'Signed in. You can close this window and return to Sleek.';
  } catch (e) {
    m.textContent = 'Failed to hand off session: ' + e;
  }
})();
</script></body></html>"#;

/// Open `url` in the platform browser.
fn open_system_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        crate::android_media::open_url(url).map_err(|e| anyhow::anyhow!(e))
    }
    #[cfg(not(target_os = "android"))]
    {
        open_desktop_browser(url)
    }
}

/// Desktop open: prefer `$BROWSER`, then the configured URL handler, then Chromium.
///
/// Codespace / desktop-lite has no default browser; Chromium from nix needs
/// `--no-sandbox` because user namespaces are blocked.
#[cfg(not(target_os = "android"))]
fn open_desktop_browser(url: &str) -> Result<()> {
    use std::process::Command;

    // 1) Explicit BROWSER (scripts/vnc-browser.sh on Codespaces).
    if let Ok(browser) = std::env::var("BROWSER") {
        if !browser.is_empty() {
            // Do not wait — browser stays open for OAuth.
            match Command::new(&browser).arg(url).spawn() {
                Ok(_) => return Ok(()),
                Err(e) => log::warn!("BROWSER={browser} failed: {e}"),
            }
        }
    }

    // 2) Prefer the desktop URL handler. This uses the user's configured
    // browser (for example the installed Chrome Flatpak), whereas probing
    // Chromium binaries first can select a browser that is unavailable or
    // cannot start in restricted VM environments.
    if let Ok(opener) = std::env::var("SLEEK_XDG_OPEN") {
        if !opener.is_empty() {
            match Command::new(&opener).arg(url).spawn() {
                Ok(_) => return Ok(()),
                Err(e) => log::warn!("SLEEK_XDG_OPEN={opener} failed: {e}"),
            }
        }
    } else if which_bin("xdg-open").is_some() {
        match Command::new("xdg-open").arg(url).spawn() {
            Ok(_) => return Ok(()),
            Err(e) => log::debug!("xdg-open failed: {e}"),
        }
    }

    // 3) Chromium / Chrome with container-friendly flags (VNC / Docker).
    let chromes = ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable"];
    for bin in chromes {
        if which_bin(bin).is_some() {
            let profile = std::env::var("SLEEK_BROWSER_PROFILE")
                .unwrap_or_else(|_| "/tmp/sleek-chromium".into());
            let _ = std::fs::create_dir_all(&profile);
            match Command::new(bin)
                .args([
                    "--no-sandbox",
                    "--disable-gpu",
                    "--disable-dev-shm-usage",
                    "--new-window",
                ])
                .arg(format!("--user-data-dir={profile}"))
                .arg(url)
                .spawn()
            {
                Ok(_) => return Ok(()),
                Err(e) => log::debug!("spawn {bin}: {e}"),
            }
        }
    }

    // 4) Firefox (nix profile); sandbox often fails in Codespaces.
    if which_bin("firefox").is_some() {
        match Command::new("firefox")
            .env("MOZ_DISABLE_CONTENT_SANDBOX", "1")
            .env("MOZ_ENABLE_WAYLAND", "0")
            .args(["--no-remote", "--new-window", url])
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(e) => log::debug!("spawn firefox: {e}"),
        }
    }

    // 5) Generic opener (xdg-open / open).
    open::that(url).context("open browser")
}

#[cfg(not(target_os = "android"))]
fn which_bin(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let p = dir.join(name);
            p.is_file().then_some(p)
        })
    })
}

/// Open the broker login URL with `mobile=1` (Android deep-link path).
///
/// The auth broker redirects to `freeq://auth?…`; `SleekActivity` captures that
/// intent and the UI polls [`crate::android_media::take_pending_deep_link`].
/// Does **not** wait for tokens — return is immediate after browser open.
#[cfg(target_os = "android")]
pub async fn bluesky_login_mobile(
    auth_broker: &str,
    handle: &str,
    mut on_status: impl FnMut(String),
) -> Result<()> {
    let url = login_url(auth_broker, handle, None);
    log::info!("bluesky mobile login url: {url}");
    match open_system_browser(&url) {
        Ok(()) => on_status(format!(
            "Browser opened — complete Bluesky sign-in; Sleek will resume via freeq://\n{url}"
        )),
        Err(e) => {
            log::warn!("failed to open browser: {e}; url={url}");
            on_status(format!(
                "Could not open browser ({e}). Open this URL, then return to Sleek:\n{url}"
            ));
        }
    }
    Ok(())
}

/// Run a one-shot loopback OAuth capture server and open the broker login URL.
///
/// Blocks until tokens arrive or `timeout` elapses. Intended to run on the
/// network tokio runtime (or a blocking thread). Used on **desktop** (and as a
/// fallback); Android prefers [`bluesky_login_mobile`] + `freeq://` deep link.
///
/// `on_status` is invoked for progress notes (e.g. browser open failure with the
/// URL so the UI is not stuck on a silent "Opening browser…").
pub async fn bluesky_login_loopback(
    auth_broker: &str,
    handle: &str,
    timeout: Duration,
    mut on_status: impl FnMut(String),
) -> Result<AuthTokens> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind loopback")?;
    listener
        .set_nonblocking(true)
        .context("set_nonblocking")?;
    let port = listener.local_addr()?.port();
    let return_to = format!("http://127.0.0.1:{port}");
    let url = login_url(auth_broker, handle, Some(&return_to));

    // Open the system browser (failure is non-fatal — loopback still waits;
    // user can open the URL manually or paste a freeq://auth link).
    // Desktop: `open` crate. Android: JNI ACTION_VIEW — `open` has no Android
    // backend and used to leave the UI stuck on "Opening browser…".
    log::info!("bluesky login url: {url}");
    match open_system_browser(&url) {
        Ok(()) => on_status(format!("Browser opened — complete sign-in, then return here\n{url}")),
        Err(e) => {
            log::warn!("failed to open browser: {e}; url={url}");
            on_status(format!(
                "Could not open browser automatically ({e}). Open this URL, then return:\n{url}"
            ));
        }
    }

    let deadline = Instant::now() + timeout;
    let timeout_url = url.clone();
    let listener = tokio::task::spawn_blocking(move || -> Result<AuthTokens> {
        loop {
            if Instant::now() > deadline {
                bail!(
                    "Sign-in timed out — open this URL if the browser did not appear, \
                     or paste a freeq://auth link:\n{timeout_url}"
                );
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // Read request (small).
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first = req.lines().next().unwrap_or("");

                    if first.starts_with("POST /capture") {
                        // Body after blank line.
                        let body = req
                            .split("\r\n\r\n")
                            .nth(1)
                            .or_else(|| req.split("\n\n").nth(1))
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        match parse_oauth_fragment_payload(&body) {
                            Ok(tokens) => {
                                let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok";
                                let _ = stream.write_all(resp.as_bytes());
                                return Ok(tokens);
                            }
                            Err(e) => {
                                let msg = format!("bad payload: {e}");
                                let resp = format!(
                                    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{msg}",
                                    msg.len()
                                );
                                let _ = stream.write_all(resp.as_bytes());
                            }
                        }
                    } else {
                        // Serve capture page for any GET.
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                            CAPTURE_HTML.len(),
                            CAPTURE_HTML
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => bail!("loopback accept: {e}"),
            }
        }
    });

    listener.await.context("join loopback task")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_freeq_url() {
        let t = parse_freeq_auth_url(
            "freeq://auth?token=tok1&broker_token=bt1&nick=alice&did=did%3Aplc%3Axyz&handle=alice.bsky.social",
        )
        .unwrap();
        assert_eq!(t.token, "tok1");
        assert_eq!(t.broker_token, "bt1");
        assert_eq!(t.nick, "alice");
        assert_eq!(t.did, "did:plc:xyz");
        assert_eq!(t.handle, "alice.bsky.social");
    }

    #[test]
    fn parse_oauth_payload() {
        use base64::Engine;
        let json = r#"{"token":"t","broker_token":"b","nick":"n","did":"did:plc:x","handle":"h"}"#;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        let t = parse_oauth_fragment_payload(&b64).unwrap();
        assert_eq!(t.token, "t");
        assert_eq!(t.did, "did:plc:x");
    }

    #[test]
    fn login_url_shapes() {
        let u = login_url(DEFAULT_AUTH_BROKER, "@foo.bsky.social", None);
        assert!(u.contains("handle=foo.bsky.social"));
        assert!(u.contains("mobile=1"));
        let u2 = login_url(DEFAULT_AUTH_BROKER, "foo", Some("http://127.0.0.1:9"));
        assert!(u2.contains("return_to="));
        assert!(!u2.contains("mobile=1"));
    }

    #[test]
    fn prefs_defaults_and_roundtrip() {
        let d = SavedPrefs::default();
        assert!(!d.av_pref_muted);
        assert!(!d.av_pref_speaker_muted);
        assert!(d.av_pref_camera);
        assert!(d.show_join_part);
        assert_eq!(
            d.recent_channels,
            vec!["#general".to_string(), "#test".to_string()]
        );
        // Missing camera field should default to true (join with camera ready).
        let partial: SavedPrefs = serde_json::from_str(r#"{"av_pref_muted":true}"#).unwrap();
        assert!(partial.av_pref_muted);
        assert!(!partial.av_pref_speaker_muted);
        assert!(partial.av_pref_camera);
        assert!(partial.show_join_part);
        assert_eq!(
            partial.recent_channels,
            vec!["#general".to_string(), "#test".to_string()]
        );
        let empty: SavedPrefs = serde_json::from_str("{}").unwrap();
        assert!(!empty.av_pref_muted);
        assert!(!empty.av_pref_speaker_muted);
        assert!(empty.av_pref_camera);
        assert!(empty.show_join_part);
        let hide_jp: SavedPrefs =
            serde_json::from_str(r#"{"show_join_part":false}"#).unwrap();
        assert!(!hide_jp.show_join_part);
        assert_eq!(
            empty.recent_channels,
            vec!["#general".to_string(), "#test".to_string()]
        );
        // Explicit list round-trips (escape JSON so `#` is not inside a raw string).
        let with_ch: SavedPrefs = serde_json::from_str(
            "{\"av_pref_muted\":false,\"av_pref_camera\":true,\"recent_channels\":[\"#test\",\"#general\"]}",
        )
        .unwrap();
        assert_eq!(
            with_ch.recent_channels,
            vec!["#test".to_string(), "#general".to_string()]
        );

        let with_hist: SavedPrefs = serde_json::from_str(
            r#"{"recent_nicks":["alice","bob"],"recent_handles":["a.bsky.social"],"last_bsky_handle":"a.bsky.social"}"#,
        )
        .unwrap();
        assert_eq!(with_hist.recent_nicks, vec!["alice".to_string(), "bob".to_string()]);
        assert_eq!(
            with_hist.recent_handles,
            vec!["a.bsky.social".to_string()]
        );
        assert_eq!(
            with_hist.last_bsky_handle.as_deref(),
            Some("a.bsky.social")
        );
    }

    #[test]
    fn prefs_save_load_roundtrip_preserves_handle_history() {
        let dir = std::env::temp_dir().join(format!(
            "sleek-prefs-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.json");

        let mut prefs = SavedPrefs::default();
        prefs.last_bsky_handle = Some("alice.bsky.social".into());
        prefs.recent_handles = vec![
            "alice.bsky.social".into(),
            "bob.bsky.social".into(),
        ];
        save_prefs(&path, &prefs).unwrap();

        let loaded = load_prefs(&path).expect("prefs.json should parse");
        assert_eq!(
            loaded.recent_handles,
            vec![
                "alice.bsky.social".to_string(),
                "bob.bsky.social".to_string(),
            ]
        );
        assert_eq!(
            loaded.last_bsky_handle.as_deref(),
            Some("alice.bsky.social")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_storage_file_moves_legacy_prefs() {
        let dir = std::env::temp_dir().join(format!(
            "sleek-migrate-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let legacy_dir = dir.join("legacy");
        let target_dir = dir.join("target");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy = legacy_dir.join("prefs.json");
        let target = target_dir.join("prefs.json");
        std::fs::write(&legacy, r#"{"recent_handles":["a.bsky.social"]}"#).unwrap();

        assert!(migrate_storage_file(&legacy, &target));
        assert!(!legacy.exists());
        assert!(target.exists());
        let loaded = load_prefs(&target).unwrap();
        assert_eq!(loaded.recent_handles, vec!["a.bsky.social".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_storage_file_skips_when_target_exists() {
        let dir = std::env::temp_dir().join(format!(
            "sleek-migrate-skip-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("legacy.json");
        let target = dir.join("target.json");
        std::fs::write(&legacy, "{}").unwrap();
        std::fs::write(&target, "{}").unwrap();

        assert!(!migrate_storage_file(&legacy, &target));
        assert!(legacy.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
