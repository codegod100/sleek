//! freeq auth-broker login (mirrors freeq-android).
//!
//! Flow:
//! 1. Open `{auth_broker}/auth/login?handle=…&return_to=http://127.0.0.1:PORT`
//!    (or `mobile=1` → `freeq://auth?…` which we also parse when pasted).
//! 2. Broker redirects with a short-lived SASL `web-token` + durable `broker_token`.
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

/// Disk-persisted session (no single-use web-token).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SavedSession {
    pub broker_token: String,
    pub did: String,
    pub handle: String,
    pub nick: String,
    pub server: String,
    #[serde(default)]
    pub last_login_unix: i64,
}

impl SavedSession {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("sleek")
            .join("session.json")
    }

    pub fn load() -> Option<Self> {
        load_session(&Self::path())
    }

    pub fn save(&self) -> Result<()> {
        save_session(&Self::path(), self)
    }

    pub fn clear() {
        let p = Self::path();
        let _ = std::fs::remove_file(p);
    }

    pub fn has_session(&self) -> bool {
        !self.broker_token.is_empty()
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

/// Run a one-shot loopback OAuth capture server and open the broker login URL.
///
/// Blocks until tokens arrive or `timeout` elapses. Intended to run on the
/// network tokio runtime (or a blocking thread).
pub async fn bluesky_login_loopback(
    auth_broker: &str,
    handle: &str,
    timeout: Duration,
) -> Result<AuthTokens> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind loopback")?;
    listener
        .set_nonblocking(true)
        .context("set_nonblocking")?;
    let port = listener.local_addr()?.port();
    let return_to = format!("http://127.0.0.1:{port}");
    let url = login_url(auth_broker, handle, Some(&return_to));

    // Open the system browser (no-op failure is non-fatal — user can paste).
    if let Err(e) = open::that(&url) {
        log::warn!("failed to open browser: {e}; url={url}");
    }

    let deadline = Instant::now() + timeout;
    let listener = tokio::task::spawn_blocking(move || -> Result<AuthTokens> {
        loop {
            if Instant::now() > deadline {
                bail!("Sign-in timed out — try again or paste the freeq://auth link");
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
}
