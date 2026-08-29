//! Local SQLite index of messages for durable unread counts.
//!
//! Counts are client-owned: we store msgid + timestamp per account/channel and
//! a last-read watermark. CHATHISTORY establishes a baseline so cold history is
//! not treated as unread; live traffic after that advances the badge.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    account TEXT NOT NULL,
    channel TEXT NOT NULL COLLATE NOCASE,
    msgid TEXT NOT NULL,
    ts INTEGER NOT NULL,
    from_me INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account, channel, msgid)
);
CREATE INDEX IF NOT EXISTS idx_messages_account_channel_ts
    ON messages(account, channel, ts);
CREATE TABLE IF NOT EXISTS read_state (
    account TEXT NOT NULL,
    channel TEXT NOT NULL COLLATE NOCASE,
    last_read_ts INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account, channel)
);
CREATE TABLE IF NOT EXISTS channel_counts (
    account TEXT NOT NULL,
    channel TEXT NOT NULL COLLATE NOCASE,
    count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account, channel)
);
CREATE TABLE IF NOT EXISTS channel_activity (
    account TEXT NOT NULL,
    channel TEXT NOT NULL COLLATE NOCASE,
    last_used_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account, channel)
);
"#;

#[derive(Debug)]
pub struct MessageStore {
    conn: Connection,
}

impl MessageStore {
    pub fn open_default() -> Self {
        let path = crate::auth::storage_dir().join("messages.db");
        match Self::open(&path) {
            Ok(store) => store,
            Err(e) => {
                log::warn!(
                    "message_store: open {} failed ({e:#}); using in-memory fallback",
                    path.display()
                );
                Self::open_in_memory().expect("in-memory sqlite")
            }
        }
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open sqlite {}", path.display()))?;
        Self::from_conn(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Idempotent insert. Empty msgid is ignored.
    pub fn insert_message(
        &self,
        account: &str,
        channel: &str,
        msgid: &str,
        ts: i64,
        from_me: bool,
    ) -> Result<()> {
        if msgid.is_empty() || msgid.starts_with("local-") || msgid.starts_with("sys-") {
            return Ok(());
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO messages(account, channel, msgid, ts, from_me)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![account, channel, msgid, ts, from_me as i32],
        )?;
        Ok(())
    }

    /// Advance last-read to at least `at_ts` (and to the latest stored message).
    pub fn mark_read(&self, account: &str, channel: &str, at_ts: i64) -> Result<()> {
        let max_msg: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(ts), 0) FROM messages WHERE account=?1 AND channel=?2",
                params![account, channel],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let ts = at_ts.max(max_msg);
        self.conn.execute(
            "INSERT INTO read_state(account, channel, last_read_ts)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(account, channel) DO UPDATE SET
               last_read_ts = MAX(read_state.last_read_ts, excluded.last_read_ts)",
            params![account, channel, ts],
        )?;
        Ok(())
    }

    /// After a CHATHISTORY batch: if this channel has no watermark yet, catch
    /// up to the latest stored message so cold history is not unread.
    pub fn baseline_after_history(&self, account: &str, channel: &str) -> Result<()> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM read_state WHERE account=?1 AND channel=?2",
                params![account, channel],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if exists {
            return Ok(());
        }
        let max_ts: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(ts), 0) FROM messages WHERE account=?1 AND channel=?2",
            params![account, channel],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO read_state(account, channel, last_read_ts) VALUES (?1, ?2, ?3)",
            params![account, channel, max_ts],
        )?;
        Ok(())
    }

    /// Before counting a live message: if there is no watermark, set it to the
    /// max timestamp of *other* stored messages (history), or 0 if none — so
    /// this live message can count as unread without resurrecting history.
    pub fn ensure_live_baseline(
        &self,
        account: &str,
        channel: &str,
        exclude_msgid: &str,
    ) -> Result<()> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM read_state WHERE account=?1 AND channel=?2",
                params![account, channel],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if exists {
            return Ok(());
        }
        let max_other: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(ts), 0) FROM messages
             WHERE account=?1 AND channel=?2 AND msgid!=?3",
            params![account, channel, exclude_msgid],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO read_state(account, channel, last_read_ts) VALUES (?1, ?2, ?3)",
            params![account, channel, max_other],
        )?;
        Ok(())
    }

    /// Unread = non-own messages strictly after the watermark.
    /// No watermark yet → 0 (baseline not established).
    pub fn unread_count(&self, account: &str, channel: &str) -> Result<u32> {
        let last_read: Option<i64> = self
            .conn
            .query_row(
                "SELECT last_read_ts FROM read_state WHERE account=?1 AND channel=?2",
                params![account, channel],
                |r| r.get(0),
            )
            .optional()?;
        let Some(last_read) = last_read else {
            return Ok(0);
        };
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages
             WHERE account=?1 AND channel=?2 AND from_me=0 AND ts > ?3",
            params![account, channel, last_read],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as u32)
    }

    /// Persist the client-owned activity count for a channel.
    pub fn set_channel_count(&self, account: &str, channel: &str, count: u32) -> Result<()> {
        self.conn.execute(
            "INSERT INTO channel_counts(account, channel, count) VALUES (?1, ?2, ?3)
             ON CONFLICT(account, channel) DO UPDATE SET count=excluded.count",
            params![account, channel, count],
        )?;
        Ok(())
    }

    /// Load a previously persisted client-owned activity count.
    pub fn channel_count(&self, account: &str, channel: &str) -> Result<u32> {
        Ok(self
            .conn
            .query_row(
                "SELECT count FROM channel_counts WHERE account=?1 AND channel=?2",
                params![account, channel],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or_default()
            .max(0) as u32)
    }

    /// Persist the last time a channel was opened or received activity.
    pub fn set_channel_activity(
        &self,
        account: &str,
        channel: &str,
        last_used_ms: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO channel_activity(account, channel, last_used_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(account, channel) DO UPDATE SET
               last_used_ms=MAX(channel_activity.last_used_ms, excluded.last_used_ms)",
            params![account, channel, last_used_ms],
        )?;
        Ok(())
    }

    /// Load a channel's persisted last-use time.
    pub fn channel_activity(&self, account: &str, channel: &str) -> Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_used_ms FROM channel_activity
                 WHERE account=?1 AND channel=?2",
                params![account, channel],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }

    /// Clear every activity count for one account.
    pub fn clear_channel_counts(&self, account: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE channel_counts SET count=0 WHERE account=?1",
            params![account],
        )?;
        Ok(())
    }
}

/// Badge label: show exact count up to 99, then `99+`.
pub fn unread_label(n: u32) -> String {
    if n > 99 {
        "99+".into()
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_baseline_then_live_unread() {
        let store = MessageStore::open_in_memory().unwrap();
        let acct = "did:plc:test";
        let ch = "#general";

        store
            .insert_message(acct, ch, "h1", 100, false)
            .unwrap();
        store
            .insert_message(acct, ch, "h2", 200, false)
            .unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 0);

        store.baseline_after_history(acct, ch).unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 0);

        store
            .insert_message(acct, ch, "live1", 300, false)
            .unwrap();
        store.ensure_live_baseline(acct, ch, "live1").unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 1);

        store.mark_read(acct, ch, 300).unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 0);
    }

    #[test]
    fn live_without_history_counts() {
        let store = MessageStore::open_in_memory().unwrap();
        let acct = "guest:sleek";
        let ch = "#test";

        store
            .insert_message(acct, ch, "m1", 50, false)
            .unwrap();
        store.ensure_live_baseline(acct, ch, "m1").unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 1);

        store
            .insert_message(acct, ch, "m2", 60, true)
            .unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 1);
    }

    #[test]
    fn reconnect_history_does_not_clobber_watermark() {
        let store = MessageStore::open_in_memory().unwrap();
        let acct = "did:plc:a";
        let ch = "#room";

        store.insert_message(acct, ch, "old", 10, false).unwrap();
        store.baseline_after_history(acct, ch).unwrap();
        store.insert_message(acct, ch, "new", 20, false).unwrap();
        store.ensure_live_baseline(acct, ch, "new").unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 1);

        // Replay history including the unread message.
        store.insert_message(acct, ch, "old", 10, false).unwrap();
        store.insert_message(acct, ch, "new", 20, false).unwrap();
        store.baseline_after_history(acct, ch).unwrap();
        assert_eq!(store.unread_count(acct, ch).unwrap(), 1);
    }

    #[test]
    fn unread_label_caps() {
        assert_eq!(unread_label(0), "0");
        assert_eq!(unread_label(99), "99");
        assert_eq!(unread_label(100), "99+");
    }

    #[test]
    fn channel_counts_are_durable_and_account_scoped() {
        let store = MessageStore::open_in_memory().unwrap();
        store.set_channel_count("alice", "#general", 3).unwrap();
        store.set_channel_count("bob", "#general", 7).unwrap();

        assert_eq!(store.channel_count("alice", "#general").unwrap(), 3);
        assert_eq!(store.channel_count("alice", "#missing").unwrap(), 0);

        store.clear_channel_counts("alice").unwrap();
        assert_eq!(store.channel_count("alice", "#general").unwrap(), 0);
        assert_eq!(store.channel_count("bob", "#general").unwrap(), 7);
    }
}
