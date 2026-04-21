//! Per-conversation agent scratchpad — a self-maintained notebook the agent
//! writes via `update_scratchpad` and reads at the start of every turn via
//! the system prompt.
//!
//! Unlike `user_memories`, this store is **conversation-scoped** and is
//! intended to survive conversation compaction (the scratchpad is stored in
//! SQLite, not in the message list).

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

/// Hard cap on stored scratchpad content (~1 000 tokens worth).
/// Keeps the prompt budget predictable.
pub const MAX_SCRATCHPAD_CHARS: usize = 4_000;

/// Marker prepended to a scratchpad after FIFO truncation.
const TRIM_MARKER: &str = "...[older notes trimmed]\n\n";

/// A conversation's private agent-maintained notebook.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentScratchpad {
    pub conversation_id: String,
    pub content: String,
    /// SQLite `datetime('now')` string, same convention as other rows.
    pub updated_at: String,
}

impl Database {
    /// Fetch the scratchpad for a conversation, if any.
    pub fn get_agent_scratchpad(
        &self,
        conversation_id: &str,
    ) -> Result<Option<AgentScratchpad>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT conversation_id, content, updated_at
             FROM agent_scratchpad WHERE conversation_id = ?1",
        )?;
        let mut rows = stmt.query([conversation_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(AgentScratchpad {
                conversation_id: row.get(0)?,
                content: row.get(1)?,
                updated_at: row.get(2)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Insert or replace the scratchpad for a conversation.
    ///
    /// Content is FIFO-truncated to `MAX_SCRATCHPAD_CHARS` with a marker
    /// to bound prompt growth.
    pub fn upsert_agent_scratchpad(
        &self,
        conversation_id: &str,
        content: &str,
    ) -> Result<(), CoreError> {
        let trimmed = enforce_cap(content);
        let conn = self.conn();
        conn.execute(
            "INSERT INTO agent_scratchpad (conversation_id, content, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(conversation_id) DO UPDATE SET
               content = excluded.content,
               updated_at = datetime('now')",
            rusqlite::params![conversation_id, trimmed],
        )?;
        Ok(())
    }

    /// Remove the scratchpad row entirely.
    pub fn clear_agent_scratchpad(&self, conversation_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM agent_scratchpad WHERE conversation_id = ?1",
            rusqlite::params![conversation_id],
        )?;
        Ok(())
    }
}

/// FIFO-truncate content to fit within `MAX_SCRATCHPAD_CHARS`.
pub(crate) fn enforce_cap(content: &str) -> String {
    if content.len() <= MAX_SCRATCHPAD_CHARS {
        return content.to_string();
    }
    // Drop leading bytes so only the most recent fit.
    let overflow = content
        .len()
        .saturating_sub(MAX_SCRATCHPAD_CHARS.saturating_sub(TRIM_MARKER.len()));
    // Align to a char boundary.
    let mut cut = overflow;
    while cut < content.len() && !content.is_char_boundary(cut) {
        cut += 1;
    }
    format!("{}{}", TRIM_MARKER, &content[cut..])
}

/// Render the scratchpad section for injection into the system prompt.
///
/// Returns an empty string when the conversation id is missing (e.g. one-shot
/// non-conversation calls), so the caller can blindly include it in the
/// dynamic sections list.
pub fn build_agent_scratchpad_prompt_section(
    db: &Database,
    conversation_id: Option<&str>,
) -> String {
    let Some(cid) = conversation_id else {
        return String::new();
    };
    let content = match db.get_agent_scratchpad(cid) {
        Ok(Some(s)) => s.content,
        Ok(None) => String::new(),
        Err(_) => return String::new(),
    };
    let body = if content.trim().is_empty() {
        "(empty — use update_scratchpad to record important findings)".to_string()
    } else {
        content
    };
    format!("## Agent Scratchpad (self-maintained)\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn test_db() -> Database {
        Database::open_memory().expect("open db")
    }

    #[test]
    fn upsert_and_get_roundtrip() {
        let db = test_db();
        db.upsert_agent_scratchpad("c1", "hello").unwrap();
        let s = db.get_agent_scratchpad("c1").unwrap().unwrap();
        assert_eq!(s.content, "hello");
        assert_eq!(s.conversation_id, "c1");
    }

    #[test]
    fn upsert_replaces_existing() {
        let db = test_db();
        db.upsert_agent_scratchpad("c1", "v1").unwrap();
        db.upsert_agent_scratchpad("c1", "v2").unwrap();
        let s = db.get_agent_scratchpad("c1").unwrap().unwrap();
        assert_eq!(s.content, "v2");
    }

    #[test]
    fn clear_removes_row() {
        let db = test_db();
        db.upsert_agent_scratchpad("c1", "v1").unwrap();
        db.clear_agent_scratchpad("c1").unwrap();
        assert!(db.get_agent_scratchpad("c1").unwrap().is_none());
    }

    #[test]
    fn enforces_cap_fifo() {
        let big = "x".repeat(MAX_SCRATCHPAD_CHARS + 500);
        let out = enforce_cap(&big);
        assert!(out.len() <= MAX_SCRATCHPAD_CHARS);
        assert!(out.starts_with("...[older notes trimmed]"));
    }

    #[test]
    fn short_content_not_trimmed() {
        let out = enforce_cap("small note");
        assert_eq!(out, "small note");
    }

    #[test]
    fn prompt_section_empty_when_unset() {
        let db = test_db();
        let section = build_agent_scratchpad_prompt_section(&db, Some("cX"));
        assert!(section.contains("empty"));
    }

    #[test]
    fn prompt_section_empty_when_no_conversation_id() {
        let db = test_db();
        let section = build_agent_scratchpad_prompt_section(&db, None);
        assert!(section.is_empty());
    }

    #[test]
    fn prompt_section_shows_content() {
        let db = test_db();
        db.upsert_agent_scratchpad("c1", "plan: step 1").unwrap();
        let section = build_agent_scratchpad_prompt_section(&db, Some("c1"));
        assert!(section.contains("plan: step 1"));
        assert!(section.starts_with("## Agent Scratchpad"));
    }
}
