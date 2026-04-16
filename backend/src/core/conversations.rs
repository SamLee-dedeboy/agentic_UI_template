//! SQLite-backed conversation persistence.
//!
//! Two tables:
//!
//! ```sql
//! conversations(id PK, session_cookie, created_at, last_active_at)
//! messages(id PK, conversation_id FK, ts, kind, content_json)
//! ```
//!
//! A *conversation* is a single Claude subprocess lifetime keyed by the
//! client-generated `session_id` (same UUID we pass through to
//! `claude --session-id`). A *message* is either the user turn that
//! started a streaming run, or a stream-json event emitted by Claude.
//! We store stream events verbatim (raw JSON) so forks can evolve
//! rendering without needing a schema migration.
//!
//! The store is intentionally tiny — no SQL query builders, no ORM. Forks
//! that need richer persistence (per-user accounts, tool-call history
//! indexed separately, soft deletes) should extend this module or
//! replace it wholesale; the consumer surface is just a handful of `pub`
//! methods.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Resolve the SQLite path from the environment.
///
/// - `APP_DB_PATH` if set, verbatim.
/// - Else `$HOME/.claude-ui-app/app.db` (created on demand).
/// - Else `./claude-ui-app.db` relative to cwd as a last resort.
pub fn resolve_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("APP_DB_PATH") {
        return PathBuf::from(p);
    }
    if let Some(home) = dirs::home_dir() {
        let dir = home.join(".claude-ui-app");
        let _ = std::fs::create_dir_all(&dir);
        return dir.join("app.db");
    }
    PathBuf::from("./claude-ui-app.db")
}

#[derive(Clone)]
pub struct ConversationStore {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationRow {
    pub id: String,
    pub session_cookie: String,
    pub created_at: i64,
    pub last_active_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: i64,
    pub conversation_id: String,
    pub ts: i64,
    pub kind: String,
    pub content_json: serde_json::Value,
}

impl ConversationStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite db at {path:?}"))?;
        ensure_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Idempotent upsert. Creates the conversation row if missing, bumps
    /// `last_active_at` if present. Guards against cookie swapping: if a
    /// row exists with a *different* `session_cookie`, returns an error
    /// rather than silently reassigning ownership.
    pub async fn ensure_conversation(
        &self,
        conversation_id: &str,
        session_cookie: &str,
    ) -> Result<()> {
        let now = now_secs();
        let conn = self.conn.lock().await;
        let existing: Option<String> = conn
            .query_row(
                "SELECT session_cookie FROM conversations WHERE id = ?1",
                params![conversation_id],
                |r| r.get(0),
            )
            .ok();
        match existing {
            Some(owner) if owner != session_cookie => {
                anyhow::bail!(
                    "conversation {} belongs to a different guest session",
                    conversation_id
                );
            }
            Some(_) => {
                conn.execute(
                    "UPDATE conversations SET last_active_at = ?1 WHERE id = ?2",
                    params![now, conversation_id],
                )?;
            }
            None => {
                conn.execute(
                    "INSERT INTO conversations (id, session_cookie, created_at, last_active_at) \
                     VALUES (?1, ?2, ?3, ?3)",
                    params![conversation_id, session_cookie, now],
                )?;
            }
        }
        Ok(())
    }

    /// Append one message. `kind` is free-form; we use `"user"` for user
    /// prompts and `"stream"` for Claude stream-json events, but forks
    /// can add their own categories (`"tool_ui_call"` etc.) without a
    /// migration since `content_json` is opaque.
    pub async fn append_message(
        &self,
        conversation_id: &str,
        kind: &str,
        content: &serde_json::Value,
    ) -> Result<i64> {
        let now = now_secs();
        let body = serde_json::to_string(content)?;
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO messages (conversation_id, ts, kind, content_json) \
             VALUES (?1, ?2, ?3, ?4)",
            params![conversation_id, now, kind, body],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Conversations owned by `session_cookie`, newest-first.
    pub async fn list_for_cookie(&self, session_cookie: &str) -> Result<Vec<ConversationRow>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, session_cookie, created_at, last_active_at \
             FROM conversations WHERE session_cookie = ?1 ORDER BY last_active_at DESC",
        )?;
        let rows = stmt
            .query_map(params![session_cookie], |r| {
                Ok(ConversationRow {
                    id: r.get(0)?,
                    session_cookie: r.get(1)?,
                    created_at: r.get(2)?,
                    last_active_at: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Replay stored messages, oldest-first. `cookie_guard` enforces that
    /// the requesting guest owns the conversation — returns an empty list
    /// otherwise (not an error: we don't want to leak existence).
    pub async fn load_messages(
        &self,
        conversation_id: &str,
        cookie_guard: &str,
    ) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock().await;
        let owner: Option<String> = conn
            .query_row(
                "SELECT session_cookie FROM conversations WHERE id = ?1",
                params![conversation_id],
                |r| r.get(0),
            )
            .ok();
        if owner.as_deref() != Some(cookie_guard) {
            return Ok(Vec::new());
        }
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, ts, kind, content_json \
             FROM messages WHERE conversation_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![conversation_id], |r| {
                let content_str: String = r.get(4)?;
                let content = serde_json::from_str::<serde_json::Value>(&content_str)
                    .unwrap_or(serde_json::Value::Null);
                Ok(MessageRow {
                    id: r.get(0)?,
                    conversation_id: r.get(1)?,
                    ts: r.get(2)?,
                    kind: r.get(3)?,
                    content_json: content,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS conversations (
             id              TEXT    PRIMARY KEY,
             session_cookie  TEXT    NOT NULL,
             created_at      INTEGER NOT NULL,
             last_active_at  INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS conversations_by_cookie
             ON conversations(session_cookie, last_active_at DESC);

         CREATE TABLE IF NOT EXISTS messages (
             id              INTEGER PRIMARY KEY AUTOINCREMENT,
             conversation_id TEXT    NOT NULL,
             ts              INTEGER NOT NULL,
             kind            TEXT    NOT NULL,
             content_json    TEXT    NOT NULL,
             FOREIGN KEY(conversation_id) REFERENCES conversations(id)
         );
         CREATE INDEX IF NOT EXISTS messages_by_conv
             ON messages(conversation_id, id);",
    )?;
    Ok(())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_store() -> ConversationStore {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        ConversationStore {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    #[tokio::test]
    async fn insert_and_list_roundtrip() {
        let s = mem_store().await;
        s.ensure_conversation("c1", "cookie-a").await.unwrap();
        s.append_message("c1", "user", &serde_json::json!({"text": "hello"}))
            .await
            .unwrap();
        s.append_message(
            "c1",
            "stream",
            &serde_json::json!({"type": "assistant", "content": "hi"}),
        )
        .await
        .unwrap();

        let msgs = s.load_messages("c1", "cookie-a").await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].kind, "user");
        assert_eq!(msgs[1].kind, "stream");

        let convs = s.list_for_cookie("cookie-a").await.unwrap();
        assert_eq!(convs.len(), 1);
        assert_eq!(convs[0].id, "c1");
    }

    #[tokio::test]
    async fn load_messages_enforces_cookie_ownership() {
        let s = mem_store().await;
        s.ensure_conversation("c1", "cookie-a").await.unwrap();
        s.append_message("c1", "user", &serde_json::json!({"t": "x"}))
            .await
            .unwrap();

        // Different cookie → empty (not an error, don't leak existence).
        let msgs = s.load_messages("c1", "cookie-b").await.unwrap();
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn ensure_conversation_rejects_cross_cookie_reuse() {
        let s = mem_store().await;
        s.ensure_conversation("c1", "cookie-a").await.unwrap();
        let err = s
            .ensure_conversation("c1", "cookie-b")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("different guest session"));
    }
}
