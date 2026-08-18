//! Public Bluesky AppView helpers (no auth).
//!
//! Used for Bluesky login handle typeahead via
//! `app.bsky.actor.searchActorsTypeahead` — same endpoint as common AT Proto
//! login autocomplete examples (e.g. Atmosphere Conf `ActorAutocomplete`).

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::time::Duration;

/// Public AppView base (unauthenticated typeahead / search).
pub const PUBLIC_APPVIEW: &str = "https://public.api.bsky.app";

/// Debounce before firing a typeahead request while the user types.
pub const TYPEAHEAD_DEBOUNCE: Duration = Duration::from_millis(300);

/// Minimum normalized query length before we hit the network.
pub const TYPEAHEAD_MIN_CHARS: usize = 2;

/// Max suggestions requested from AppView.
pub const TYPEAHEAD_LIMIT: u32 = 8;

/// One actor from `searchActorsTypeahead`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleSuggestion {
    pub did: String,
    pub handle: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TypeaheadResponse {
    #[serde(default)]
    actors: Vec<TypeaheadActor>,
}

#[derive(Debug, Deserialize)]
struct TypeaheadActor {
    did: String,
    handle: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
}

/// Trim whitespace and a leading `@` (users often type `@alice…`).
pub fn normalize_handle_query(raw: &str) -> String {
    raw.trim().trim_start_matches('@').trim().to_string()
}

/// Whether `raw` is long enough to justify a typeahead fetch.
pub fn should_typeahead(raw: &str) -> bool {
    normalize_handle_query(raw).chars().count() >= TYPEAHEAD_MIN_CHARS
}

/// Public Bluesky profile fields (from `app.bsky.actor.getProfile`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorProfile {
    pub did: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<String>,
    pub followers_count: Option<u64>,
    pub follows_count: Option<u64>,
    pub posts_count: Option<u64>,
}

/// Web URL for a Bluesky profile (`handle` or DID).
pub fn bluesky_profile_url(actor: &str) -> String {
    format!("https://bsky.app/profile/{}", actor.trim().trim_start_matches('@'))
}

/// Whether `did` is a Bluesky-resolvable AT Proto identity (not a guest `did:key`).
pub fn is_atproto_did(did: &str) -> bool {
    let d = did.trim();
    (d.starts_with("did:plc:") || d.starts_with("did:web:")) && !d.is_empty()
}

/// Whether `nick` looks like a Bluesky handle we can pass to `getProfile`
/// (e.g. `alice.bsky.social`, `chadfowler.com`) when WHOIS/DID is unavailable.
pub fn looks_like_bsky_handle(nick: &str) -> bool {
    let n = nick.trim().trim_start_matches('@');
    if n.is_empty() || n.len() > 253 {
        return false;
    }
    if n.starts_with("did:") || n.starts_with('#') || n.contains([' ', '/', '\\']) {
        return false;
    }
    // Domain-shaped: at least one dot, no leading/trailing dots, ASCII-ish labels.
    let Some((left, right)) = n.rsplit_once('.') else {
        return false;
    };
    !left.is_empty()
        && !right.is_empty()
        && right.chars().all(|c| c.is_ascii_alphanumeric())
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// Fetch a public Bluesky profile by handle or DID (no auth).
pub async fn fetch_actor_profile(actor: &str) -> Result<ActorProfile> {
    let actor = actor.trim().trim_start_matches('@');
    if actor.is_empty() {
        bail!("empty actor");
    }
    let profile = freeq_sdk::pds::fetch_profile(actor)
        .await
        .with_context(|| format!("fetch profile {actor}"))?;
    Ok(ActorProfile {
        did: profile.did,
        handle: profile.handle,
        display_name: profile
            .display_name
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        description: profile
            .description
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        avatar: profile
            .avatar
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        followers_count: profile.followers_count,
        follows_count: profile.follows_count,
        posts_count: profile.posts_count,
    })
}

/// Prefix search for Bluesky handles / display names. Does not require auth.
pub async fn search_actors_typeahead(
    query: &str,
    limit: u32,
) -> Result<Vec<HandleSuggestion>> {
    let q = normalize_handle_query(query);
    if q.chars().count() < TYPEAHEAD_MIN_CHARS {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 100);
    let url = format!(
        "{}/xrpc/app.bsky.actor.searchActorsTypeahead?q={}&limit={}",
        PUBLIC_APPVIEW,
        urlencoding::encode(&q),
        limit
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .context("reqwest client")?;
    let resp = client.get(&url).send().await.context("typeahead request")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("typeahead HTTP {status}: {body}");
    }
    let parsed: TypeaheadResponse = resp.json().await.context("typeahead json")?;
    Ok(parsed
        .actors
        .into_iter()
        .filter(|a| !a.handle.is_empty() && !a.did.is_empty())
        .map(|a| HandleSuggestion {
            did: a.did,
            handle: a.handle,
            display_name: a
                .display_name
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        })
        .collect())
}

/// UI + debounce state for the Bluesky handle field on the connect screen.
#[derive(Debug, Clone)]
pub struct HandleTypeahead {
    pub suggestions: Vec<HandleSuggestion>,
    /// Dropdown visible (false after pick / Escape / empty query).
    pub open: bool,
    pub loading: bool,
    /// Keyboard highlight into `suggestions` (`None` = none).
    pub selected: Option<usize>,
    /// Normalized query last observed from the text field.
    pub last_query: String,
    /// Normalized query waiting for debounce / in flight.
    pub pending_query: String,
    /// When to send the next fetch (`None` = nothing scheduled).
    pub fetch_at: Option<std::time::Instant>,
    /// Monotonic id so stale responses are ignored.
    pub request_id: u64,
    /// Request id currently in flight (`None` if idle).
    pub inflight_id: Option<u64>,
}

impl Default for HandleTypeahead {
    fn default() -> Self {
        Self {
            suggestions: Vec::new(),
            open: false,
            loading: false,
            selected: None,
            last_query: String::new(),
            pending_query: String::new(),
            fetch_at: None,
            request_id: 0,
            inflight_id: None,
        }
    }
}

impl HandleTypeahead {
    /// Sync debounce state from the current handle field value.
    pub fn sync_from_input(&mut self, raw: &str) {
        let q = normalize_handle_query(raw);
        if q == self.last_query {
            return;
        }
        self.last_query = q.clone();
        self.selected = None;

        if !should_typeahead(&q) {
            self.pending_query.clear();
            self.fetch_at = None;
            self.suggestions.clear();
            self.open = false;
            self.loading = self.inflight_id.is_some();
            return;
        }

        // Exact match already listed — keep suggestions but don't refetch noise.
        if self
            .suggestions
            .iter()
            .any(|s| s.handle.eq_ignore_ascii_case(&q))
            && self.pending_query.eq_ignore_ascii_case(&q)
        {
            self.open = true;
            return;
        }

        self.pending_query = q;
        self.fetch_at = Some(std::time::Instant::now() + TYPEAHEAD_DEBOUNCE);
        self.open = true;
    }

    /// If debounce elapsed, return `(request_id, query)` to fetch.
    pub fn take_ready_fetch(&mut self) -> Option<(u64, String)> {
        let at = self.fetch_at?;
        if std::time::Instant::now() < at {
            return None;
        }
        self.fetch_at = None;
        let q = self.pending_query.clone();
        if !should_typeahead(&q) {
            return None;
        }
        self.request_id = self.request_id.wrapping_add(1);
        let id = self.request_id;
        self.inflight_id = Some(id);
        self.loading = true;
        Some((id, q))
    }

    /// Apply a completed typeahead response (ignore stale request ids).
    pub fn apply_results(
        &mut self,
        request_id: u64,
        query: String,
        actors: Vec<HandleSuggestion>,
    ) {
        if self.inflight_id != Some(request_id) {
            return;
        }
        self.inflight_id = None;
        self.loading = self.fetch_at.is_some();
        // Drop results if the user already moved on past this query.
        let current = normalize_handle_query(&self.last_query);
        if !current.is_empty()
            && !current.eq_ignore_ascii_case(&query)
            && !self.pending_query.eq_ignore_ascii_case(&query)
        {
            return;
        }
        self.suggestions = actors;
        self.selected = if self.suggestions.is_empty() {
            None
        } else {
            Some(0)
        };
        self.open = !self.suggestions.is_empty() && should_typeahead(&self.last_query);
    }

    pub fn apply_failed(&mut self, request_id: u64) {
        if self.inflight_id != Some(request_id) {
            return;
        }
        self.inflight_id = None;
        self.loading = self.fetch_at.is_some();
        // Keep prior suggestions; just stop the spinner.
    }

    pub fn move_selection(&mut self, delta: i32) {
        let n = self.suggestions.len();
        if n == 0 {
            self.selected = None;
            return;
        }
        let cur = self.selected.unwrap_or(0) as i32;
        let next = (cur + delta).rem_euclid(n as i32) as usize;
        self.selected = Some(next);
        self.open = true;
    }

    /// Fill `handle` from the highlighted (or first) suggestion and close.
    pub fn accept_selected(&mut self, handle_out: &mut String) -> bool {
        let idx = self.selected.or(if self.suggestions.is_empty() {
            None
        } else {
            Some(0)
        });
        let Some(i) = idx else {
            return false;
        };
        let Some(actor) = self.suggestions.get(i) else {
            return false;
        };
        *handle_out = actor.handle.clone();
        self.last_query = normalize_handle_query(handle_out);
        self.pending_query = self.last_query.clone();
        self.fetch_at = None;
        self.open = false;
        self.selected = None;
        self.suggestions.clear();
        true
    }

    pub fn pick(&mut self, handle: &str, handle_out: &mut String) {
        *handle_out = handle.to_string();
        self.last_query = normalize_handle_query(handle_out);
        self.pending_query = self.last_query.clone();
        self.fetch_at = None;
        self.open = false;
        self.selected = None;
        self.suggestions.clear();
    }

    pub fn dismiss(&mut self) {
        self.open = false;
        self.selected = None;
    }

    pub fn needs_repaint(&self) -> bool {
        self.fetch_at.is_some() || self.inflight_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_at_and_whitespace() {
        assert_eq!(normalize_handle_query("  @Alice.bsky.social "), "Alice.bsky.social");
        assert_eq!(normalize_handle_query("bob"), "bob");
        assert_eq!(normalize_handle_query(""), "");
    }

    #[test]
    fn bluesky_profile_url_strips_at() {
        assert_eq!(
            bluesky_profile_url("@alice.bsky.social"),
            "https://bsky.app/profile/alice.bsky.social"
        );
        assert_eq!(
            bluesky_profile_url("did:plc:abc"),
            "https://bsky.app/profile/did:plc:abc"
        );
    }

    #[test]
    fn is_atproto_did_rejects_did_key() {
        assert!(is_atproto_did("did:plc:abcdef"));
        assert!(is_atproto_did("did:web:example.com"));
        assert!(!is_atproto_did("did:key:z6Mk"));
        assert!(!is_atproto_did(""));
        assert!(!is_atproto_did("alice"));
    }

    #[test]
    fn looks_like_bsky_handle_accepts_domains() {
        assert!(looks_like_bsky_handle("chadfowler.com"));
        assert!(looks_like_bsky_handle("@alice.bsky.social"));
        assert!(looks_like_bsky_handle("a.b"));
        assert!(!looks_like_bsky_handle("alice"));
        assert!(!looks_like_bsky_handle("#general"));
        assert!(!looks_like_bsky_handle("did:plc:x"));
        assert!(!looks_like_bsky_handle("foo bar.com"));
    }

    #[test]
    fn should_typeahead_requires_min_chars() {
        assert!(!should_typeahead(""));
        assert!(!should_typeahead("@a"));
        assert!(!should_typeahead(" a "));
        assert!(should_typeahead("al"));
        assert!(should_typeahead("@al"));
        assert!(should_typeahead("  @alice "));
    }

    #[test]
    fn sync_schedules_debounce_and_clears_short_query() {
        let mut t = HandleTypeahead::default();
        t.sync_from_input("a");
        assert!(t.fetch_at.is_none());
        assert!(!t.open);

        t.sync_from_input("al");
        assert!(t.fetch_at.is_some());
        assert!(t.open);
        assert_eq!(t.pending_query, "al");

        t.sync_from_input("@");
        assert!(t.fetch_at.is_none());
        assert!(!t.open);
        assert!(t.suggestions.is_empty());
    }

    #[test]
    fn take_ready_fetch_increments_request_id() {
        let mut t = HandleTypeahead::default();
        t.sync_from_input("alice");
        t.fetch_at = Some(std::time::Instant::now() - Duration::from_millis(1));
        let (id1, q) = t.take_ready_fetch().expect("ready");
        assert_eq!(q, "alice");
        assert_eq!(id1, 1);
        assert_eq!(t.inflight_id, Some(1));
        assert!(t.loading);
        assert!(t.take_ready_fetch().is_none());
    }

    #[test]
    fn apply_results_ignores_stale_and_accepts_current() {
        let mut t = HandleTypeahead::default();
        t.sync_from_input("alice");
        t.fetch_at = Some(std::time::Instant::now() - Duration::from_millis(1));
        let (id, _) = t.take_ready_fetch().unwrap();

        t.apply_results(
            id.wrapping_sub(1),
            "alice".into(),
            vec![HandleSuggestion {
                did: "did:plc:x".into(),
                handle: "alice.bsky.social".into(),
                display_name: Some("Alice".into()),
            }],
        );
        assert!(t.suggestions.is_empty());
        assert!(t.loading);

        t.apply_results(
            id,
            "alice".into(),
            vec![HandleSuggestion {
                did: "did:plc:x".into(),
                handle: "alice.bsky.social".into(),
                display_name: Some("Alice".into()),
            }],
        );
        assert_eq!(t.suggestions.len(), 1);
        assert_eq!(t.selected, Some(0));
        assert!(t.open);
        assert!(!t.loading);
    }

    #[test]
    fn accept_selected_fills_handle_and_closes() {
        let mut t = HandleTypeahead {
            suggestions: vec![
                HandleSuggestion {
                    did: "did:1".into(),
                    handle: "one.bsky.social".into(),
                    display_name: None,
                },
                HandleSuggestion {
                    did: "did:2".into(),
                    handle: "two.bsky.social".into(),
                    display_name: Some("Two".into()),
                },
            ],
            open: true,
            selected: Some(1),
            ..Default::default()
        };
        let mut handle = String::from("tw");
        assert!(t.accept_selected(&mut handle));
        assert_eq!(handle, "two.bsky.social");
        assert!(!t.open);
        assert!(t.suggestions.is_empty());
    }

    #[test]
    fn move_selection_wraps() {
        let mut t = HandleTypeahead {
            suggestions: vec![
                HandleSuggestion {
                    did: "did:1".into(),
                    handle: "a.bsky.social".into(),
                    display_name: None,
                },
                HandleSuggestion {
                    did: "did:2".into(),
                    handle: "b.bsky.social".into(),
                    display_name: None,
                },
            ],
            selected: Some(0),
            open: true,
            ..Default::default()
        };
        t.move_selection(-1);
        assert_eq!(t.selected, Some(1));
        t.move_selection(1);
        assert_eq!(t.selected, Some(0));
    }
}
