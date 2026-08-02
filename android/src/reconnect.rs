//! Auto-reconnect schedule after unexpected disconnects (EOF, ping timeout, …).
//!
//! Mirrors freeq-android `ReconnectBackoff`: `2^attempts` seconds, clamped at 30.

/// Delay before the next reconnect attempt.
///
/// `attempts` is 1-based (increment before calling). Zero → 0.
pub fn delay_secs(attempts: u32) -> u64 {
    if attempts == 0 {
        return 0;
    }
    // freeq-android: `1L shl minOf(attempts, 5)` then clamp to 30.
    // attempts=1 → 2s, 2 → 4s, …, 5+ → 30s.
    let shift = attempts.min(5);
    (1u64 << shift).min(30)
}

/// Whether an unexpected disconnect should trigger auto-reconnect.
pub fn should_auto_reconnect(intentional: bool, nick_empty: bool, reason: &str) -> bool {
    if intentional || nick_empty {
        return false;
    }
    // User-initiated quit from the net layer — never bounce back.
    reason != "quit"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_attempt_waits_two_seconds() {
        assert_eq!(delay_secs(1), 2);
    }

    #[test]
    fn doubles_each_attempt() {
        assert_eq!(delay_secs(1), 2);
        assert_eq!(delay_secs(2), 4);
        assert_eq!(delay_secs(3), 8);
        assert_eq!(delay_secs(4), 16);
    }

    #[test]
    fn caps_at_thirty_seconds() {
        assert_eq!(delay_secs(5), 30);
        assert_eq!(delay_secs(6), 30);
        assert_eq!(delay_secs(20), 30);
        assert_eq!(delay_secs(u32::MAX), 30);
    }

    #[test]
    fn zero_attempts_returns_zero() {
        assert_eq!(delay_secs(0), 0);
    }

    #[test]
    fn should_reconnect_on_eof() {
        assert!(should_auto_reconnect(false, false, "EOF"));
        assert!(should_auto_reconnect(false, false, "Ping timeout"));
        assert!(should_auto_reconnect(false, false, "event channel closed"));
    }

    #[test]
    fn should_not_reconnect_on_quit_or_intentional() {
        assert!(!should_auto_reconnect(false, false, "quit"));
        assert!(!should_auto_reconnect(true, false, "EOF"));
        assert!(!should_auto_reconnect(false, true, "EOF"));
    }
}
