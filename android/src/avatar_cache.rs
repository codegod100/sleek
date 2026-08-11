//! Nick → Bluesky avatar URL cache (freeq-web4 / freeq-android parity).
//!
//! Resolve via Bluesky AppView `getProfile` using:
//! 1. A server-bound AT Proto DID (`did:plc:` / `did:web:`) when known
//! 2. Else the IRC nick itself when it is already a Bluesky handle
//!    (e.g. `chadfowler.com`, `alice.bsky.social`) — freeq's common pattern
//!
//! Never invent `<bare-nick>.bsky.social` from a freely chosen nick (that would
//! show a stranger's photo). Guests / `did:key` without a handle-shaped nick
//! keep the colored-initial fallback.

use std::collections::{HashMap, HashSet};

use crate::bsky::{self, ActorProfile};

/// Pending Bluesky profile fetch for an avatar (`nick` + AppView `actor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarFetch {
    pub nick: String,
    /// DID or handle passed to `app.bsky.actor.getProfile`.
    pub actor: String,
}

/// In-memory nick → avatar URL cache with a pending-fetch queue for the net layer.
#[derive(Debug, Default)]
pub struct AvatarCache {
    /// Lowercased nick → avatar CDN URL when known.
    urls: HashMap<String, String>,
    /// Lowercased nicks with a fetch in flight.
    pending: HashSet<String>,
    /// Lowercased nicks that failed or resolved with no avatar (do not retry).
    settled: HashSet<String>,
    /// Fetches to dispatch this frame.
    to_fetch: Vec<AvatarFetch>,
}

impl AvatarCache {
    /// Cached avatar URL for `nick`, if resolved.
    pub fn avatar_url(&self, nick: &str) -> Option<&str> {
        self.urls.get(&nick.to_lowercase()).map(|s| s.as_str())
    }

    /// Bluesky AppView actor for `nick`: AT Proto DID preferred, else handle-shaped nick.
    ///
    /// Mirrors freeq-web4 `resolve_nick_did` / `looks_like_actor`.
    pub fn resolve_actor(nick: &str, did: Option<&str>) -> Option<String> {
        if let Some(d) = did.map(str::trim).filter(|d| bsky::is_atproto_did(d)) {
            return Some(d.to_string());
        }
        let n = nick.trim().trim_start_matches('@');
        if bsky::looks_like_bsky_handle(n) {
            return Some(n.to_string());
        }
        None
    }

    /// Queue a Bluesky `getProfile` when we have a resolvable actor for `nick`.
    ///
    /// No-ops without an actor, while a fetch is in flight, or after a prior
    /// settle. A call with no DID and a non-handle nick is a no-op (and NOT
    /// marked settled) so a later DID from an account-tag is never pre-empted.
    pub fn prefetch(&mut self, nick: &str, did: Option<&str>) {
        let key = nick.trim().to_lowercase();
        if key.is_empty() {
            return;
        }
        // Skip guest / ephemeral nicks — they are not Bluesky accounts.
        if key.starts_with("guest") || key.starts_with("web") {
            return;
        }
        if self.urls.contains_key(&key) || self.pending.contains(&key) || self.settled.contains(&key)
        {
            return;
        }
        let Some(actor) = Self::resolve_actor(nick, did) else {
            return;
        };
        self.pending.insert(key);
        self.to_fetch.push(AvatarFetch {
            nick: nick.trim().to_string(),
            actor,
        });
    }

    /// Drain fetches queued since the last drain (for `NetCmd` dispatch).
    pub fn drain_pending(&mut self) -> Vec<AvatarFetch> {
        std::mem::take(&mut self.to_fetch)
    }

    /// Apply a finished Bluesky profile fetch for `nick`.
    pub fn apply_result(&mut self, nick: &str, result: Result<ActorProfile, String>) {
        let key = nick.to_lowercase();
        self.pending.remove(&key);
        match result {
            Ok(profile) => {
                if let Some(url) = profile
                    .avatar
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                {
                    self.urls.insert(key.clone(), url.clone());
                    self.settled.remove(&key);
                    // Also index by handle so later nick↔handle aliases hit.
                    let handle = profile.handle.trim().trim_start_matches('@');
                    if !handle.is_empty() && handle.to_lowercase() != key {
                        self.urls.insert(handle.to_lowercase(), url);
                    }
                } else {
                    // Resolved but no avatar — don't hammer AppView.
                    self.settled.insert(key);
                }
            }
            Err(_) => {
                self.settled.insert(key);
            }
        }
    }

    /// Seed a known avatar URL (e.g. from the open profile modal).
    pub fn seed(&mut self, nick: &str, avatar_url: &str) {
        let url = avatar_url.trim();
        if url.is_empty() {
            return;
        }
        let key = nick.to_lowercase();
        self.urls.insert(key.clone(), url.to_string());
        self.pending.remove(&key);
        self.settled.remove(&key);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with_avatar(did: &str, handle: &str, avatar: Option<&str>) -> ActorProfile {
        ActorProfile {
            did: did.into(),
            handle: handle.into(),
            display_name: Some("Real".into()),
            description: None,
            avatar: avatar.map(|s| s.into()),
            followers_count: None,
            follows_count: None,
            posts_count: None,
        }
    }

    #[test]
    fn bare_nick_without_did_does_not_queue() {
        let mut cache = AvatarCache::default();
        cache.prefetch("olive", None);
        cache.prefetch("olive", Some(""));
        assert!(cache.drain_pending().is_empty());
        assert!(cache.avatar_url("olive").is_none());
    }

    #[test]
    fn did_key_without_handle_nick_does_not_queue() {
        let mut cache = AvatarCache::default();
        cache.prefetch("olive", Some("did:key:z6MkExample"));
        assert!(cache.drain_pending().is_empty());
    }

    #[test]
    fn verified_did_queues_actor_as_did_not_nick() {
        let mut cache = AvatarCache::default();
        cache.prefetch("olive", Some("did:plc:abc123"));
        let pending = cache.drain_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].nick, "olive");
        assert_eq!(pending[0].actor, "did:plc:abc123");
        // Second prefetch while in-flight is a no-op.
        cache.prefetch("olive", Some("did:plc:abc123"));
        assert!(cache.drain_pending().is_empty());
    }

    #[test]
    fn handle_shaped_nick_queues_without_did() {
        let mut cache = AvatarCache::default();
        cache.prefetch("chadfowler.com", None);
        let pending = cache.drain_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].actor, "chadfowler.com");
    }

    #[test]
    fn did_preferred_over_handle_shaped_nick() {
        let mut cache = AvatarCache::default();
        cache.prefetch("alice.bsky.social", Some("did:plc:real"));
        let pending = cache.drain_pending();
        assert_eq!(pending[0].actor, "did:plc:real");
    }

    #[test]
    fn did_key_falls_through_to_handle_shaped_nick() {
        let actor = AvatarCache::resolve_actor(
            "alice.bsky.social",
            Some("did:key:z6MkExample"),
        );
        assert_eq!(actor.as_deref(), Some("alice.bsky.social"));
    }

    #[test]
    fn apply_result_stores_url_case_insensitive() {
        let mut cache = AvatarCache::default();
        cache.prefetch("Olive", Some("did:plc:abc"));
        let _ = cache.drain_pending();
        cache.apply_result(
            "Olive",
            Ok(profile_with_avatar(
                "did:plc:abc",
                "real.bsky.social",
                Some("https://cdn.example/a.jpg"),
            )),
        );
        assert_eq!(
            cache.avatar_url("olive"),
            Some("https://cdn.example/a.jpg")
        );
        assert_eq!(
            cache.avatar_url("OLIVE"),
            Some("https://cdn.example/a.jpg")
        );
        // Also indexed by Bluesky handle from the profile.
        assert_eq!(
            cache.avatar_url("real.bsky.social"),
            Some("https://cdn.example/a.jpg")
        );
    }

    #[test]
    fn empty_avatar_settles_without_retry() {
        let mut cache = AvatarCache::default();
        cache.prefetch("alice", Some("did:plc:x"));
        let _ = cache.drain_pending();
        cache.apply_result(
            "alice",
            Ok(profile_with_avatar("did:plc:x", "alice.bsky.social", None)),
        );
        assert!(cache.avatar_url("alice").is_none());
        cache.prefetch("alice", Some("did:plc:x"));
        assert!(cache.drain_pending().is_empty());
    }

    #[test]
    fn seed_overrides_settled() {
        let mut cache = AvatarCache::default();
        cache.apply_result("bob", Err("nope".into()));
        cache.seed("bob", "https://cdn.example/b.jpg");
        assert_eq!(cache.avatar_url("bob"), Some("https://cdn.example/b.jpg"));
        // Already have URL — prefetch is a no-op.
        cache.prefetch("bob", Some("did:plc:bob"));
        assert!(cache.drain_pending().is_empty());
    }
}
