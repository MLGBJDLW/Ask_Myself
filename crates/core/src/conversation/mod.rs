//! Conversation persistence — types and CRUD for conversations, messages, and agent configs.

pub mod memory;
pub mod summarizer;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{Role, ToolCallRequest};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A conversation session with an LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub system_prompt: String,
    pub collection_context: Option<CollectionContext>,
    pub project_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Structured collection context attached to a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionContext {
    pub title: String,
    pub description: Option<String>,
    pub query_text: Option<String>,
    #[serde(default)]
    pub source_ids: Vec<String>,
}

/// A single image attachment sent with a user message.
///
/// Persisted alongside the message row as a JSON blob in the
/// `image_attachments_json` column (see migration `v040`). The field name on
/// the wire is `imageAttachments` to match the frontend DTO.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAttachment {
    pub base64_data: String,
    pub media_type: String,
    pub original_name: String,
}

/// A single message within a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    pub conversation_id: String,
    pub role: Role,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_calls: Vec<ToolCallRequest>,
    pub artifacts: Option<serde_json::Value>,
    pub token_count: u32,
    pub created_at: String,
    pub sort_order: i64,
    pub thinking: Option<String>,
    /// Image attachments sent with this user message. Persisted nullably;
    /// legacy rows (pre-`v040`) and non-user messages will be `None`.
    #[serde(default)]
    pub image_attachments: Option<Vec<ImageAttachment>>,
}

/// Saved LLM provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub context_window: Option<i64>,
    pub is_default: bool,
    pub reasoning_enabled: Option<bool>,
    pub thinking_budget: Option<i64>,
    pub reasoning_effort: Option<String>,
    pub max_iterations: Option<i64>,
    /// Optional cheaper model for summarization (e.g. "gpt-4o-mini").
    pub summarization_model: Option<String>,
    /// Optional provider override for summarization (e.g. "open_ai").
    pub summarization_provider: Option<String>,
    /// Optional whitelist of delegated tool names that subagents may use.
    pub subagent_allowed_tools: Option<Vec<String>>,
    /// Optional whitelist of enabled skill IDs that delegated subagents may inherit.
    pub subagent_allowed_skill_ids: Option<Vec<String>>,
    /// Maximum number of subagents that may run concurrently.
    pub subagent_max_parallel: Option<i64>,
    /// Maximum number of subagent or adjudication calls allowed per turn.
    pub subagent_max_calls_per_turn: Option<i64>,
    /// Soft total token budget for subagent and adjudication work per turn.
    pub subagent_token_budget: Option<i64>,
    pub tool_timeout_secs: Option<i64>,
    pub agent_timeout_secs: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// Statistics about conversations and messages in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationStats {
    pub total_conversations: usize,
    pub total_messages: usize,
    pub oldest_conversation: Option<String>,
    pub db_size_bytes: u64,
}

/// A search result from cross-conversation message search.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSearchResult {
    pub conversation_id: String,
    pub conversation_title: Option<String>,
    pub message_preview: String,
    pub message_role: String,
    pub timestamp: String,
    pub relevance_score: f64,
}

/// A persisted user turn with its route and trace lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTurn {
    pub id: String,
    pub conversation_id: String,
    pub user_message_id: String,
    pub assistant_message_id: Option<String>,
    pub status: String,
    pub route_kind: Option<String>,
    pub trace: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

/// A snapshot of conversation state before compaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub conversation_id: String,
    pub label: String,
    pub message_count: u32,
    pub estimated_tokens: u32,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Input types
// ---------------------------------------------------------------------------

/// Input for creating a new conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationInput {
    pub provider: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub collection_context: Option<CollectionContext>,
    pub project_id: Option<String>,
}

/// Input for creating / updating an agent config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentConfigInput {
    /// `None` → create new, `Some` → update existing.
    pub id: Option<String>,
    pub name: String,
    pub provider: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub model: String,
    pub temperature: Option<f64>,
    pub max_tokens: Option<i64>,
    pub context_window: Option<i64>,
    pub is_default: bool,
    pub reasoning_enabled: Option<bool>,
    pub thinking_budget: Option<i64>,
    pub reasoning_effort: Option<String>,
    pub max_iterations: Option<i64>,
    /// Optional cheaper model for summarization (e.g. "gpt-4o-mini").
    pub summarization_model: Option<String>,
    /// Optional provider override for summarization (e.g. "open_ai").
    pub summarization_provider: Option<String>,
    /// Optional whitelist of delegated tool names that subagents may use.
    pub subagent_allowed_tools: Option<Vec<String>>,
    /// Optional whitelist of enabled skill IDs that delegated subagents may inherit.
    pub subagent_allowed_skill_ids: Option<Vec<String>>,
    /// Maximum number of subagents that may run concurrently.
    pub subagent_max_parallel: Option<i64>,
    /// Maximum number of subagent or adjudication calls allowed per turn.
    pub subagent_max_calls_per_turn: Option<i64>,
    /// Soft total token budget for subagent and adjudication work per turn.
    pub subagent_token_budget: Option<i64>,
    pub tool_timeout_secs: Option<i64>,
    pub agent_timeout_secs: Option<i64>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn normalize_optional_url(url: Option<&str>) -> Option<String> {
    url.and_then(|value| {
        let trimmed = value.trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

fn role_to_str(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn str_to_role(s: &str) -> Role {
    match s {
        "system" => Role::System,
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    }
}

fn serialize_optional_string_list(value: Option<&[String]>) -> Result<Option<String>, CoreError> {
    match value {
        Some(items) => Ok(Some(serde_json::to_string(items)?)),
        None => Ok(None),
    }
}

fn parse_optional_string_list(value: Option<String>) -> Option<Vec<String>> {
    value.and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
}

fn serialize_collection_context(
    value: Option<&CollectionContext>,
) -> Result<Option<String>, CoreError> {
    match value {
        Some(context) => Ok(Some(serde_json::to_string(context)?)),
        None => Ok(None),
    }
}

pub(crate) fn parse_collection_context(value: Option<String>) -> Option<CollectionContext> {
    value.and_then(|json| serde_json::from_str::<CollectionContext>(&json).ok())
}

/// Truncate a string to `max_chars`, appending "…" if truncated.
fn truncate_preview(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    let mut end = max_chars;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ---------------------------------------------------------------------------
// Conversation CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// Create a new conversation. Returns the persisted row.
    pub fn create_conversation(
        &self,
        input: &CreateConversationInput,
    ) -> Result<Conversation, CoreError> {
        let id = new_id();
        let system_prompt = input.system_prompt.as_deref().unwrap_or("");
        let collection_context_json =
            serialize_collection_context(input.collection_context.as_ref())?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO conversations (id, provider, model, system_prompt, collection_context_json, project_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                &id,
                &input.provider,
                &input.model,
                system_prompt,
                &collection_context_json,
                &input.project_id
            ],
        )?;
        drop(conn);
        self.get_conversation(&id)
    }

    /// List conversations ordered by most-recently updated first.
    pub fn list_conversations(&self) -> Result<Vec<Conversation>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, provider, model, system_prompt, collection_context_json, project_id, created_at, updated_at
             FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                provider: row.get(2)?,
                model: row.get(3)?,
                system_prompt: row.get(4)?,
                collection_context: parse_collection_context(row.get(5)?),
                project_id: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get a single conversation by id.
    pub fn get_conversation(&self, id: &str) -> Result<Conversation, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, title, provider, model, system_prompt, collection_context_json, project_id, created_at, updated_at
             FROM conversations WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    provider: row.get(2)?,
                    model: row.get(3)?,
                    system_prompt: row.get(4)?,
                    collection_context: parse_collection_context(row.get(5)?),
                    project_id: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Conversation {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    /// Delete a conversation (messages are CASCADE-deleted).
    pub fn delete_conversation(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "DELETE FROM conversations WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Conversation {id}")));
        }
        Ok(())
    }

    /// Delete multiple conversations by ID (messages are CASCADE-deleted).
    /// Returns the number of deleted rows. Empty `ids` is a no-op.
    pub fn delete_conversations_batch(&self, ids: &[String]) -> Result<usize, CoreError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn();
        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM conversations WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();
        let affected = conn.execute(&sql, params.as_slice())?;
        Ok(affected)
    }

    /// Delete ALL conversations (messages are CASCADE-deleted).
    /// Returns the number of deleted rows.
    pub fn delete_all_conversations(&self) -> Result<usize, CoreError> {
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM conversations", [])?;
        Ok(affected)
    }

    /// Update the title of a conversation (also bumps `updated_at`).
    pub fn update_conversation_title(&self, id: &str, title: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE conversations SET title = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, title],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Conversation {id}")));
        }
        Ok(())
    }

    /// Update the system prompt of a conversation (also bumps `updated_at`).
    pub fn update_conversation_system_prompt(
        &self,
        id: &str,
        system_prompt: &str,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE conversations SET system_prompt = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, system_prompt],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Conversation {id}")));
        }
        Ok(())
    }

    /// Update the structured collection context for a conversation.
    pub fn update_conversation_collection_context(
        &self,
        id: &str,
        collection_context: Option<&CollectionContext>,
    ) -> Result<(), CoreError> {
        let collection_context_json = serialize_collection_context(collection_context)?;
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE conversations SET collection_context_json = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, collection_context_json],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Conversation {id}")));
        }
        Ok(())
    }

    /// Delete empty conversations older than `days_old` days.
    ///
    /// An "empty" conversation has zero messages. Returns the number of
    /// conversations deleted.
    pub fn cleanup_empty_conversations(&self, days_old: u32) -> Result<usize, CoreError> {
        let conn = self.conn();
        let deleted = conn.execute(
            "DELETE FROM conversations WHERE id IN (
                SELECT c.id FROM conversations c
                LEFT JOIN messages m ON m.conversation_id = c.id
                WHERE c.created_at <= datetime('now', ?1)
                GROUP BY c.id
                HAVING COUNT(m.id) = 0
            )",
            rusqlite::params![format!("-{days_old} days")],
        )?;
        Ok(deleted)
    }

    /// Return high-level statistics about conversations and messages.
    pub fn get_conversation_stats(&self) -> Result<ConversationStats, CoreError> {
        let conn = self.conn();

        let total_conversations: usize =
            conn.query_row("SELECT COUNT(*) FROM conversations", [], |r| r.get(0))?;

        let total_messages: usize =
            conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;

        let oldest_conversation: Option<String> =
            conn.query_row("SELECT MIN(created_at) FROM conversations", [], |r| {
                r.get(0)
            })?;

        let db_size_bytes: u64 = match self.db_path() {
            Some(p) => std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
            None => 0,
        };

        Ok(ConversationStats {
            total_conversations,
            total_messages,
            oldest_conversation,
            db_size_bytes,
        })
    }

    /// Search across all conversations for messages matching a query.
    /// Uses FTS5 on message content with BM25 ranking.
    pub fn search_conversations(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ConversationSearchResult>, CoreError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.conn();

        // Check if FTS table exists
        let fts_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='fts_messages')",
            [],
            |r| r.get(0),
        )?;

        if fts_exists {
            // Tokenize: wrap each word in double-quotes for exact prefix matching
            let fts_query: String = trimmed
                .split_whitespace()
                .map(|w| format!("\"{}\"", w.replace('"', "")))
                .collect::<Vec<_>>()
                .join(" ");

            if fts_query.is_empty() {
                return Ok(Vec::new());
            }

            let mut stmt = conn.prepare(
                "SELECT f.conversation_id, c.title, f.content, f.role,
                        m.created_at, bm25(fts_messages) AS rank
                 FROM fts_messages f
                 JOIN conversations c ON c.id = f.conversation_id
                 JOIN messages m ON m.id = f.message_id
                 WHERE fts_messages MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )?;

            let rows = stmt.query_map(rusqlite::params![&fts_query, limit as i64], |row| {
                let content: String = row.get(2)?;
                let title: Option<String> = row.get(1)?;
                Ok(ConversationSearchResult {
                    conversation_id: row.get(0)?,
                    conversation_title: title.filter(|t| !t.is_empty()),
                    message_preview: truncate_preview(&content, 200),
                    message_role: row.get(3)?,
                    timestamp: row.get(4)?,
                    relevance_score: row.get::<_, f64>(5)?.abs(),
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok(results)
        } else {
            // Fallback: LIKE search
            let pattern = format!("%{}%", trimmed.replace('%', "\\%").replace('_', "\\_"));
            let mut stmt = conn.prepare(
                "SELECT m.conversation_id, c.title, m.content, m.role, m.created_at
                 FROM messages m
                 JOIN conversations c ON c.id = m.conversation_id
                 WHERE m.role IN ('user', 'assistant')
                   AND m.content LIKE ?1 ESCAPE '\\'
                 ORDER BY m.created_at DESC
                 LIMIT ?2",
            )?;

            let rows = stmt.query_map(rusqlite::params![&pattern, limit as i64], |row| {
                let content: String = row.get(2)?;
                let title: Option<String> = row.get(1)?;
                Ok(ConversationSearchResult {
                    conversation_id: row.get(0)?,
                    conversation_title: title.filter(|t| !t.is_empty()),
                    message_preview: truncate_preview(&content, 200),
                    message_role: row.get(3)?,
                    timestamp: row.get(4)?,
                    relevance_score: 1.0,
                })
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok(results)
        }
    }
}

// ---------------------------------------------------------------------------
// Turn CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// Create a new conversation turn for a just-persisted user message.
    pub fn create_conversation_turn(
        &self,
        conversation_id: &str,
        user_message_id: &str,
        route_kind: Option<&str>,
    ) -> Result<ConversationTurn, CoreError> {
        let id = new_id();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO conversation_turns (id, conversation_id, user_message_id, route_kind, status)
             VALUES (?1, ?2, ?3, ?4, 'running')",
            rusqlite::params![&id, conversation_id, user_message_id, route_kind],
        )?;
        drop(conn);
        self.get_conversation_turn(&id)
    }

    /// Update a turn with its final assistant message and optional trace payload.
    pub fn finalize_conversation_turn(
        &self,
        turn_id: &str,
        status: &str,
        assistant_message_id: Option<&str>,
        trace: Option<&serde_json::Value>,
    ) -> Result<(), CoreError> {
        let trace_json = match trace {
            Some(value) => Some(serde_json::to_string(value)?),
            None => None,
        };
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE conversation_turns
             SET status = ?2,
                 assistant_message_id = COALESCE(?3, assistant_message_id),
                 trace_json = COALESCE(?4, trace_json),
                 finished_at = datetime('now'),
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![turn_id, status, assistant_message_id, trace_json],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Conversation turn {turn_id}")));
        }
        Ok(())
    }

    /// Update a running turn trace without finalizing it.
    pub fn update_conversation_turn_trace(
        &self,
        turn_id: &str,
        trace: Option<&serde_json::Value>,
    ) -> Result<(), CoreError> {
        let trace_json = match trace {
            Some(value) => Some(serde_json::to_string(value)?),
            None => None,
        };
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE conversation_turns
             SET trace_json = ?2,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![turn_id, trace_json],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Conversation turn {turn_id}")));
        }
        Ok(())
    }

    /// Update route kind and optional trace for a running turn.
    pub fn update_conversation_turn_progress(
        &self,
        turn_id: &str,
        route_kind: Option<&str>,
        trace: Option<&serde_json::Value>,
    ) -> Result<(), CoreError> {
        let trace_json = match trace {
            Some(value) => Some(serde_json::to_string(value)?),
            None => None,
        };
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE conversation_turns
             SET route_kind = COALESCE(?2, route_kind),
                 trace_json = COALESCE(?3, trace_json),
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![turn_id, route_kind, trace_json],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Conversation turn {turn_id}")));
        }
        Ok(())
    }

    /// Load one turn by id.
    pub fn get_conversation_turn(&self, id: &str) -> Result<ConversationTurn, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, conversation_id, user_message_id, assistant_message_id, status, route_kind, trace_json, created_at, updated_at, finished_at
             FROM conversation_turns
             WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                let trace_json: Option<String> = row.get(6)?;
                Ok(ConversationTurn {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    user_message_id: row.get(2)?,
                    assistant_message_id: row.get(3)?,
                    status: row.get(4)?,
                    route_kind: row.get(5)?,
                    trace: match trace_json {
                        Some(json) => Some(serde_json::from_str(&json).map_err(|err| {
                            rusqlite::Error::FromSqlConversionFailure(
                                6,
                                rusqlite::types::Type::Text,
                                Box::new(err),
                            )
                        })?),
                        None => None,
                    },
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                    finished_at: row.get(9)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Conversation turn {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    /// List turns for a conversation ordered by creation time.
    pub fn get_conversation_turns(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ConversationTurn>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, user_message_id, assistant_message_id, status, route_kind, trace_json, created_at, updated_at, finished_at
             FROM conversation_turns
             WHERE conversation_id = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
            let trace_json: Option<String> = row.get(6)?;
            Ok(ConversationTurn {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                user_message_id: row.get(2)?,
                assistant_message_id: row.get(3)?,
                status: row.get(4)?,
                route_kind: row.get(5)?,
                trace: match trace_json {
                    Some(json) => Some(serde_json::from_str(&json).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?),
                    None => None,
                },
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                finished_at: row.get(9)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// Message CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// Add a message to a conversation.
    pub fn add_message(&self, msg: &ConversationMessage) -> Result<(), CoreError> {
        let role_str = role_to_str(&msg.role);
        let tool_calls_json = if msg.tool_calls.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&msg.tool_calls)?)
        };
        let artifacts_json = match &msg.artifacts {
            Some(value) => Some(serde_json::to_string(value)?),
            None => None,
        };
        let image_attachments_json = match &msg.image_attachments {
            Some(atts) if !atts.is_empty() => Some(serde_json::to_string(atts)?),
            _ => None,
        };

        let conn = self.conn();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_call_id, tool_calls_json, artifacts_json, token_count, sort_order, thinking, image_attachments_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                &msg.id,
                &msg.conversation_id,
                role_str,
                &msg.content,
                &msg.tool_call_id,
                &tool_calls_json,
                &artifacts_json,
                msg.token_count,
                msg.sort_order,
                &msg.thinking,
                &image_attachments_json,
            ],
        )?;

        // Bump the conversation's updated_at.
        conn.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&msg.conversation_id],
        )?;

        Ok(())
    }

    /// Get all messages for a conversation, ordered by `sort_order` ASC.
    pub fn get_messages(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ConversationMessage>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, tool_call_id, tool_calls_json, artifacts_json, token_count, created_at, sort_order, thinking, image_attachments_json
             FROM messages WHERE conversation_id = ?1 ORDER BY sort_order ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
            let role_str: String = row.get(2)?;
            let tool_calls_json: Option<String> = row.get(5)?;
            let artifacts_json: Option<String> = row.get(6)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                role_str,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                tool_calls_json,
                artifacts_json,
                row.get::<_, u32>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (
                id,
                conv_id,
                role_str,
                content,
                tool_call_id,
                tc_json,
                artifacts_json,
                token_count,
                created_at,
                sort_order,
                thinking,
                image_attachments_json,
            ) = row?;
            let tool_calls: Vec<ToolCallRequest> = match tc_json {
                Some(json) => serde_json::from_str(&json)?,
                None => Vec::new(),
            };
            let artifacts = match artifacts_json {
                Some(json) => Some(serde_json::from_str(&json)?),
                None => None,
            };
            let image_attachments = match image_attachments_json {
                Some(json) => serde_json::from_str::<Vec<ImageAttachment>>(&json).ok(),
                None => None,
            };
            results.push(ConversationMessage {
                id,
                conversation_id: conv_id,
                role: str_to_role(&role_str),
                content,
                tool_call_id,
                tool_calls,
                artifacts,
                token_count,
                created_at,
                sort_order,
                thinking,
                image_attachments,
            });
        }
        Ok(results)
    }

    /// Delete all messages for a conversation.
    pub fn delete_messages(&self, conversation_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM messages WHERE conversation_id = ?1",
            rusqlite::params![conversation_id],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Checkpoint CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// Create a checkpoint (snapshot label) before compaction.
    /// Returns the new checkpoint ID.
    pub fn create_checkpoint(
        &self,
        conversation_id: &str,
        label: &str,
        message_count: u32,
        estimated_tokens: u32,
    ) -> Result<String, CoreError> {
        let id = new_id();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO conversation_checkpoints (id, conversation_id, label, message_count, estimated_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![&id, conversation_id, label, message_count, estimated_tokens],
        )?;
        Ok(id)
    }

    /// Bulk-insert messages into the archived_messages table for a checkpoint.
    pub fn archive_messages(
        &self,
        checkpoint_id: &str,
        conversation_id: &str,
        messages: &[ConversationMessage],
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "INSERT INTO archived_messages (id, checkpoint_id, conversation_id, role, content, tool_call_id, tool_calls_json, artifacts_json, token_count, original_sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )?;
        for msg in messages {
            let role_str = role_to_str(&msg.role);
            let tc_json = if msg.tool_calls.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&msg.tool_calls)?)
            };
            let artifacts_json = match &msg.artifacts {
                Some(value) => Some(serde_json::to_string(value)?),
                None => None,
            };
            stmt.execute(rusqlite::params![
                &Uuid::new_v4().to_string(),
                checkpoint_id,
                conversation_id,
                role_str,
                &msg.content,
                &msg.tool_call_id,
                &tc_json,
                &artifacts_json,
                msg.token_count,
                msg.sort_order,
            ])?;
        }
        Ok(())
    }

    /// List checkpoints for a conversation, newest first.
    pub fn list_checkpoints(&self, conversation_id: &str) -> Result<Vec<Checkpoint>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, label, message_count, estimated_tokens, created_at
             FROM conversation_checkpoints
             WHERE conversation_id = ?1
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
            Ok(Checkpoint {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                label: row.get(2)?,
                message_count: row.get(3)?,
                estimated_tokens: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Restore archived messages from a checkpoint.
    pub fn restore_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<Vec<ConversationMessage>, CoreError> {
        let conn = self.conn();

        // First get the conversation_id for this checkpoint.
        let conversation_id: String = conn.query_row(
            "SELECT conversation_id FROM conversation_checkpoints WHERE id = ?1",
            rusqlite::params![checkpoint_id],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, tool_call_id, tool_calls_json, artifacts_json, token_count, created_at, original_sort_order
             FROM archived_messages
             WHERE checkpoint_id = ?1
             ORDER BY original_sort_order ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![checkpoint_id], |row| {
            let role_str: String = row.get(2)?;
            let tc_json: Option<String> = row.get(5)?;
            let artifacts_json: Option<String> = row.get(6)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                role_str,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                tc_json,
                artifacts_json,
                row.get::<_, u32>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (
                id,
                conv_id,
                role_str,
                content,
                tool_call_id,
                tc_json,
                artifacts_json,
                token_count,
                created_at,
                sort_order,
            ) = row?;
            let tool_calls: Vec<ToolCallRequest> = match tc_json {
                Some(json) => serde_json::from_str(&json)?,
                None => Vec::new(),
            };
            let artifacts = match artifacts_json {
                Some(json) => Some(serde_json::from_str(&json)?),
                None => None,
            };
            results.push(ConversationMessage {
                id,
                conversation_id: conv_id,
                role: str_to_role(&role_str),
                content,
                tool_call_id,
                tool_calls,
                artifacts,
                token_count,
                created_at,
                sort_order,
                thinking: None, // Archived messages don't preserve thinking
                image_attachments: None, // Archived messages don't preserve attachments
            });
        }

        let _ = conversation_id; // used for query validation
        Ok(results)
    }

    /// Delete a checkpoint (cascade deletes archived_messages via FK).
    pub fn delete_checkpoint(&self, checkpoint_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        // Delete archived messages first (SQLite FK cascade may not be on).
        conn.execute(
            "DELETE FROM archived_messages WHERE checkpoint_id = ?1",
            rusqlite::params![checkpoint_id],
        )?;
        conn.execute(
            "DELETE FROM conversation_checkpoints WHERE id = ?1",
            rusqlite::params![checkpoint_id],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// AgentConfig CRUD
// ---------------------------------------------------------------------------

/// Decrypt the `api_key` field of an [`AgentConfig`] read from the database.
/// If the value was stored as legacy plaintext (no `enc:v1:` prefix), it is
/// returned unchanged — the caller's next `save_agent_config` will encrypt it.
fn decrypt_agent_config_key(mut config: AgentConfig) -> Result<AgentConfig, CoreError> {
    config.api_key = crate::crypto::decrypt_api_key(&config.api_key)?;
    Ok(config)
}

impl Database {
    /// Upsert an agent config. Returns the persisted row.
    pub fn save_agent_config(
        &self,
        input: &SaveAgentConfigInput,
    ) -> Result<AgentConfig, CoreError> {
        let id = input.id.clone().unwrap_or_else(new_id);
        let normalized_base_url = normalize_optional_url(input.base_url.as_deref());
        let encrypted_api_key = crate::crypto::encrypt_api_key(&input.api_key)?;
        let subagent_allowed_tools_json =
            serialize_optional_string_list(input.subagent_allowed_tools.as_deref())?;
        let subagent_allowed_skill_ids_json =
            serialize_optional_string_list(input.subagent_allowed_skill_ids.as_deref())?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO agent_configs (id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default, reasoning_enabled, thinking_budget, reasoning_effort, max_iterations, summarization_model, summarization_provider, subagent_allowed_tools_json, subagent_allowed_skill_ids_json, subagent_max_parallel, subagent_max_calls_per_turn, subagent_token_budget, tool_timeout_secs, agent_timeout_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                provider = excluded.provider,
                api_key = excluded.api_key,
                base_url = excluded.base_url,
                model = excluded.model,
                temperature = excluded.temperature,
                max_tokens = excluded.max_tokens,
                context_window = excluded.context_window,
                is_default = excluded.is_default,
                reasoning_enabled = excluded.reasoning_enabled,
                thinking_budget = excluded.thinking_budget,
                reasoning_effort = excluded.reasoning_effort,
                max_iterations = excluded.max_iterations,
                summarization_model = excluded.summarization_model,
                summarization_provider = excluded.summarization_provider,
                subagent_allowed_tools_json = excluded.subagent_allowed_tools_json,
                subagent_allowed_skill_ids_json = excluded.subagent_allowed_skill_ids_json,
                subagent_max_parallel = excluded.subagent_max_parallel,
                subagent_max_calls_per_turn = excluded.subagent_max_calls_per_turn,
                subagent_token_budget = excluded.subagent_token_budget,
                tool_timeout_secs = excluded.tool_timeout_secs,
                agent_timeout_secs = excluded.agent_timeout_secs,
                updated_at = datetime('now')",
            rusqlite::params![
                &id,
                &input.name,
                &input.provider,
                &encrypted_api_key,
                &normalized_base_url,
                &input.model,
                input.temperature,
                input.max_tokens,
                input.context_window,
                input.is_default as i32,
                input.reasoning_enabled,
                input.thinking_budget,
                &input.reasoning_effort,
                input.max_iterations,
                &input.summarization_model,
                &input.summarization_provider,
                &subagent_allowed_tools_json,
                &subagent_allowed_skill_ids_json,
                input.subagent_max_parallel,
                input.subagent_max_calls_per_turn,
                input.subagent_token_budget,
                input.tool_timeout_secs,
                input.agent_timeout_secs,
            ],
        )?;
        drop(conn);
        self.get_agent_config(&id)
    }

    /// List all agent configs.
    pub fn list_agent_configs(&self) -> Result<Vec<AgentConfig>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default, reasoning_enabled, thinking_budget, reasoning_effort, created_at, updated_at, max_iterations, summarization_model, summarization_provider, subagent_allowed_tools_json, subagent_allowed_skill_ids_json, subagent_max_parallel, subagent_max_calls_per_turn, subagent_token_budget, tool_timeout_secs, agent_timeout_secs
             FROM agent_configs ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let subagent_allowed_tools_json: Option<String> = row.get(18)?;
            let subagent_allowed_skill_ids_json: Option<String> = row.get(19)?;
            Ok(AgentConfig {
                id: row.get(0)?,
                name: row.get(1)?,
                provider: row.get(2)?,
                api_key: row.get(3)?,
                base_url: row.get(4)?,
                model: row.get(5)?,
                temperature: row.get(6)?,
                max_tokens: row.get(7)?,
                context_window: row.get(8)?,
                is_default: row.get::<_, i32>(9)? != 0,
                reasoning_enabled: row.get(10)?,
                thinking_budget: row.get(11)?,
                reasoning_effort: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
                max_iterations: row.get(15)?,
                summarization_model: row.get(16)?,
                summarization_provider: row.get(17)?,
                subagent_allowed_tools: parse_optional_string_list(subagent_allowed_tools_json),
                subagent_allowed_skill_ids: parse_optional_string_list(
                    subagent_allowed_skill_ids_json,
                ),
                subagent_max_parallel: row.get(20)?,
                subagent_max_calls_per_turn: row.get(21)?,
                subagent_token_budget: row.get(22)?,
                tool_timeout_secs: row.get(23)?,
                agent_timeout_secs: row.get(24)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(decrypt_agent_config_key(row?)?);
        }
        Ok(results)
    }

    /// Get a single agent config by id.
    pub fn get_agent_config(&self, id: &str) -> Result<AgentConfig, CoreError> {
        let conn = self.conn();
        let config = conn.query_row(
            "SELECT id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default, reasoning_enabled, thinking_budget, reasoning_effort, created_at, updated_at, max_iterations, summarization_model, summarization_provider, subagent_allowed_tools_json, subagent_allowed_skill_ids_json, subagent_max_parallel, subagent_max_calls_per_turn, subagent_token_budget, tool_timeout_secs, agent_timeout_secs
             FROM agent_configs WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                let subagent_allowed_tools_json: Option<String> = row.get(18)?;
                let subagent_allowed_skill_ids_json: Option<String> = row.get(19)?;
                Ok(AgentConfig {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    provider: row.get(2)?,
                    api_key: row.get(3)?,
                    base_url: row.get(4)?,
                    model: row.get(5)?,
                    temperature: row.get(6)?,
                    max_tokens: row.get(7)?,
                    context_window: row.get(8)?,
                    is_default: row.get::<_, i32>(9)? != 0,
                    reasoning_enabled: row.get(10)?,
                    thinking_budget: row.get(11)?,
                    reasoning_effort: row.get(12)?,
                    created_at: row.get(13)?,
                    updated_at: row.get(14)?,
                    max_iterations: row.get(15)?,
                    summarization_model: row.get(16)?,
                    summarization_provider: row.get(17)?,
                    subagent_allowed_tools: parse_optional_string_list(subagent_allowed_tools_json),
                    subagent_allowed_skill_ids: parse_optional_string_list(
                        subagent_allowed_skill_ids_json,
                    ),
                    subagent_max_parallel: row.get(20)?,
                    subagent_max_calls_per_turn: row.get(21)?,
                    subagent_token_budget: row.get(22)?,
                    tool_timeout_secs: row.get(23)?,
                    agent_timeout_secs: row.get(24)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("AgentConfig {id}"))
            }
            other => CoreError::Database(other),
        })?;
        decrypt_agent_config_key(config)
    }

    /// Delete an agent config by id.
    pub fn delete_agent_config(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "DELETE FROM agent_configs WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("AgentConfig {id}")));
        }
        Ok(())
    }

    /// Set one config as default (clears all others).
    pub fn set_default_agent_config(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        // Verify it exists first.
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_configs WHERE id = ?1)",
            rusqlite::params![id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(CoreError::NotFound(format!("AgentConfig {id}")));
        }
        conn.execute("UPDATE agent_configs SET is_default = 0", [])?;
        conn.execute(
            "UPDATE agent_configs SET is_default = 1 WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    /// Get the default agent config (if any).
    pub fn get_default_agent_config(&self) -> Result<Option<AgentConfig>, CoreError> {
        let conn = self.conn();
        let result = conn.query_row(
            "SELECT id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default, reasoning_enabled, thinking_budget, reasoning_effort, created_at, updated_at, max_iterations, summarization_model, summarization_provider, subagent_allowed_tools_json, subagent_allowed_skill_ids_json, subagent_max_parallel, subagent_max_calls_per_turn, subagent_token_budget, tool_timeout_secs, agent_timeout_secs
             FROM agent_configs WHERE is_default = 1 LIMIT 1",
            [],
            |row| {
                let subagent_allowed_tools_json: Option<String> = row.get(18)?;
                let subagent_allowed_skill_ids_json: Option<String> = row.get(19)?;
                Ok(AgentConfig {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    provider: row.get(2)?,
                    api_key: row.get(3)?,
                    base_url: row.get(4)?,
                    model: row.get(5)?,
                    temperature: row.get(6)?,
                    max_tokens: row.get(7)?,
                    context_window: row.get(8)?,
                    is_default: row.get::<_, i32>(9)? != 0,
                    reasoning_enabled: row.get(10)?,
                    thinking_budget: row.get(11)?,
                    reasoning_effort: row.get(12)?,
                    created_at: row.get(13)?,
                    updated_at: row.get(14)?,
                    max_iterations: row.get(15)?,
                    summarization_model: row.get(16)?,
                    summarization_provider: row.get(17)?,
                    subagent_allowed_tools: parse_optional_string_list(subagent_allowed_tools_json),
                    subagent_allowed_skill_ids: parse_optional_string_list(
                        subagent_allowed_skill_ids_json,
                    ),
                    subagent_max_parallel: row.get(20)?,
                    subagent_max_calls_per_turn: row.get(21)?,
                    subagent_token_budget: row.get(22)?,
                    tool_timeout_secs: row.get(23)?,
                    agent_timeout_secs: row.get(24)?,
                })
            },
        );
        match result {
            Ok(config) => Ok(Some(decrypt_agent_config_key(config)?)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CoreError::Database(e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Conversation Source Scoping
// ---------------------------------------------------------------------------

impl Database {
    /// Link multiple sources to a conversation.
    pub fn link_sources(
        &self,
        conversation_id: &str,
        source_ids: &[String],
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO conversation_sources (conversation_id, source_id)
             VALUES (?1, ?2)",
        )?;
        for sid in source_ids {
            stmt.execute(rusqlite::params![conversation_id, sid])?;
        }
        Ok(())
    }

    /// Unlink a single source from a conversation.
    pub fn unlink_source(&self, conversation_id: &str, source_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM conversation_sources
             WHERE conversation_id = ?1 AND source_id = ?2",
            rusqlite::params![conversation_id, source_id],
        )?;
        Ok(())
    }

    /// Get all source IDs linked to a conversation.
    pub fn get_linked_sources(&self, conversation_id: &str) -> Result<Vec<String>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT source_id FROM conversation_sources
             WHERE conversation_id = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
            row.get::<_, String>(0)
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Replace all source links for a conversation (delete old, insert new).
    pub fn set_conversation_sources(
        &self,
        conversation_id: &str,
        source_ids: &[String],
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM conversation_sources WHERE conversation_id = ?1",
            rusqlite::params![conversation_id],
        )?;
        if !source_ids.is_empty() {
            let mut stmt = conn.prepare(
                "INSERT INTO conversation_sources (conversation_id, source_id)
                 VALUES (?1, ?2)",
            )?;
            for sid in source_ids {
                stmt.execute(rusqlite::params![conversation_id, sid])?;
            }
        }
        Ok(())
    }
}

pub fn build_source_scope_prompt_section(
    db: &Database,
    source_ids: &[String],
) -> Result<String, CoreError> {
    if source_ids.is_empty() {
        return Ok(String::new());
    }

    let allowed: HashSet<&str> = source_ids.iter().map(String::as_str).collect();
    let mut sources = db.list_sources()?;
    sources.retain(|source| allowed.contains(source.id.as_str()));

    let mut section = String::from(
        "## Active Source Scope\n\nThis conversation is currently limited to the following sources. Treat this as a hard boundary for document retrieval and evidence claims.\n",
    );

    if sources.is_empty() {
        section
            .push_str("\n- Scope is active, but the linked sources are currently unavailable.\n");
    } else {
        section.push('\n');
        for source in sources {
            section.push_str(&format!("- {} (`{}`)\n", source.root_path, source.id));
        }
    }

    section.push_str(
        "\nIf something is missing, say you could not find it in the current source scope unless you explicitly searched all sources.",
    );
    Ok(section)
}

/// Build a structured collection-context prompt section.
pub fn build_collection_context_prompt_section(
    collection_context: Option<&CollectionContext>,
) -> String {
    let Some(context) = collection_context else {
        return String::new();
    };

    let mut section = String::from("## Collection Context\n");
    section.push_str(&format!("Title: {}\n", context.title));
    if let Some(description) = context
        .description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        section.push_str(&format!("Description: {}\n", description));
    }
    if let Some(query_text) = context
        .query_text
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        section.push_str(&format!("Base query: {}\n", query_text));
    }
    if !context.source_ids.is_empty() {
        section.push_str(&format!("Source IDs: {}\n", context.source_ids.join(", ")));
    }
    section.push_str(
        "\nUse this collection and its saved evidence as your primary working set.\n\
If the collection is insufficient, say so explicitly before widening to the full knowledge base.\n\
When widening scope, explain why extra retrieval was needed.",
    );
    section
}

// ---------------------------------------------------------------------------
// LLM-based title generation
// ---------------------------------------------------------------------------

const TITLE_SYSTEM_PROMPT: &str = "You are a conversation title generator. \
Given the user's first message (and optionally the assistant's reply), \
generate a concise, descriptive title in 5-10 words. \
The title should capture the main topic or intent. \
Reply with ONLY the title text. No quotes, no punctuation at the end. \
Use the same language as the user's message.";

/// Generate a conversation title using an LLM provider.
///
/// Sends the first user message (and optionally the start of the assistant
/// reply) to the LLM with a short system prompt asking for a concise title.
/// Falls back to simple truncation if the LLM call fails.
pub async fn generate_title(
    provider: &dyn crate::llm::LlmProvider,
    model: &str,
    user_message: &str,
    assistant_reply: Option<&str>,
) -> String {
    let mut user_content = format!(
        "User message:\n{}",
        truncate_for_title_context(user_message, 500)
    );
    if let Some(reply) = assistant_reply {
        let trimmed = truncate_for_title_context(reply, 300);
        if !trimmed.is_empty() {
            user_content.push_str(&format!("\n\nAssistant reply:\n{}", trimmed));
        }
    }

    let request = crate::llm::CompletionRequest {
        model: model.to_string(),
        messages: vec![
            crate::llm::Message::text(crate::llm::Role::System, TITLE_SYSTEM_PROMPT),
            crate::llm::Message::text(crate::llm::Role::User, &user_content),
        ],
        temperature: Some(0.3),
        max_tokens: Some(60),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
    };

    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        provider.complete(&request),
    )
    .await
    {
        Ok(Ok(response)) => {
            let title = response.content.trim().to_string();
            if title.is_empty() {
                fallback_title(user_message)
            } else {
                title
            }
        }
        _ => fallback_title(user_message),
    }
}

fn truncate_to_char_count(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

/// Truncate text to a maximum character count for title-generation context.
fn truncate_for_title_context(text: &str, max_chars: usize) -> &str {
    truncate_to_char_count(text, max_chars)
}

/// Simple truncation fallback when LLM title generation fails.
fn fallback_title(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.chars().count() <= 50 {
        return trimmed.to_string();
    }
    let truncated = truncate_to_char_count(trimmed, 50);
    // Try to break at a word boundary
    if let Some(pos) = truncated.rfind(' ') {
        if pos > 20 {
            return format!("{}...", &truncated[..pos]);
        }
    }
    format!("{}...", truncated)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    #[test]
    fn test_fallback_title_handles_cjk_without_panicking() {
        let message = "多字节片段".repeat(16);
        let title = fallback_title(&message);

        assert!(title.starts_with("多字节片段"));
        assert!(title.ends_with("..."));
        assert!(title.chars().count() <= 53);
    }

    #[test]
    fn test_truncate_for_title_context_counts_characters() {
        let text = "北".repeat(400);
        let truncated = truncate_for_title_context(&text, 300);
        assert_eq!(truncated.chars().count(), 300);
    }

    #[test]
    fn test_conversation_crud() {
        let db = Database::open_memory().unwrap();

        // Create
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: Some("You are helpful.".into()),
                collection_context: None,
                project_id: None,
            })
            .unwrap();
        assert_eq!(conv.provider, "openai");
        assert_eq!(conv.system_prompt, "You are helpful.");

        // Get
        let fetched = db.get_conversation(&conv.id).unwrap();
        assert_eq!(fetched.id, conv.id);

        // List
        let all = db.list_conversations().unwrap();
        assert_eq!(all.len(), 1);

        // Update title
        db.update_conversation_title(&conv.id, "My Chat").unwrap();
        let updated = db.get_conversation(&conv.id).unwrap();
        assert_eq!(updated.title, "My Chat");

        // Delete
        db.delete_conversation(&conv.id).unwrap();
        assert!(db.get_conversation(&conv.id).is_err());
    }

    #[test]
    fn test_conversation_persists_collection_context() {
        let db = Database::open_memory().unwrap();

        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
                collection_context: Some(CollectionContext {
                    title: "Retry Collection".into(),
                    description: Some("Saved retry evidence".into()),
                    query_text: Some("retry timeout guard".into()),
                    source_ids: vec!["source-1".into(), "source-2".into()],
                }),
                project_id: None,
            })
            .unwrap();

        let fetched = db.get_conversation(&conv.id).unwrap();
        let context = fetched.collection_context.expect("collection context");
        assert_eq!(context.title, "Retry Collection");
        assert_eq!(context.source_ids.len(), 2);
    }

    #[test]
    fn test_conversation_turn_lifecycle() {
        let db = Database::open_memory().unwrap();
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();

        let user_msg = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::User,
            content: "Why did the retry guard fail?".into(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: None,
            token_count: 8,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&user_msg).unwrap();

        let turn = db
            .create_conversation_turn(&conv.id, &user_msg.id, Some("KnowledgeRetrieval"))
            .unwrap();
        assert_eq!(turn.status, "running");
        assert_eq!(turn.route_kind.as_deref(), Some("KnowledgeRetrieval"));

        let trace = serde_json::json!({
            "kind": "turnTrace",
            "routeKind": "KnowledgeRetrieval",
            "items": [{ "kind": "status", "text": "Testing", "tone": "muted" }]
        });
        db.update_conversation_turn_progress(&turn.id, Some("KnowledgeRetrieval"), Some(&trace))
            .unwrap();

        let assistant_msg = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::Assistant,
            content: "It skipped the early return.".into(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: None,
            token_count: 7,
            created_at: String::new(),
            sort_order: 1,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&assistant_msg).unwrap();

        db.finalize_conversation_turn(&turn.id, "success", Some(&assistant_msg.id), Some(&trace))
            .unwrap();

        let fetched = db.get_conversation_turn(&turn.id).unwrap();
        assert_eq!(fetched.status, "success");
        assert_eq!(
            fetched.assistant_message_id.as_deref(),
            Some(assistant_msg.id.as_str())
        );
        assert!(fetched.finished_at.is_some());

        let all = db.get_conversation_turns(&conv.id).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, turn.id);
    }

    #[test]
    fn test_message_crud() {
        let db = Database::open_memory().unwrap();
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();

        let msg = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::User,
            content: "Hello!".into(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: None,
            token_count: 2,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&msg).unwrap();

        // Add assistant message with tool calls
        let tc = crate::llm::ToolCallRequest {
            id: "call_1".into(),
            name: "search".into(),
            arguments: r#"{"q":"rust"}"#.into(),
            thought_signature: None,
        };
        let msg2 = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![tc],
            artifacts: Some(serde_json::json!({ "kind": "plan" })),
            token_count: 10,
            created_at: String::new(),
            sort_order: 1,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&msg2).unwrap();

        let messages = db.get_messages(&conv.id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].tool_calls.len(), 1);
        assert_eq!(messages[1].tool_calls[0].name, "search");
        assert_eq!(messages[1].artifacts.as_ref().unwrap()["kind"], "plan");
    }

    #[test]
    fn test_message_cascade_delete() {
        let db = Database::open_memory().unwrap();
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();
        let msg = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::User,
            content: "test".into(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: None,
            token_count: 1,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&msg).unwrap();

        db.delete_conversation(&conv.id).unwrap();
        // Messages should be gone (CASCADE)
        let messages = db.get_messages(&conv.id).unwrap();
        assert!(messages.is_empty());
    }

    #[test]
    fn test_build_source_scope_prompt_section_lists_linked_sources() {
        let db = Database::open_memory().unwrap();
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        db.add_source(CreateSourceInput {
            root_path: dir_a.path().to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .unwrap();
        db.add_source(CreateSourceInput {
            root_path: dir_b.path().to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .unwrap();

        let sources = db.list_sources().unwrap();
        let section = build_source_scope_prompt_section(&db, &[sources[0].id.clone()]).unwrap();

        assert!(section.contains("## Active Source Scope"));
        assert!(section.contains(dir_a.path().to_string_lossy().as_ref()));
        assert!(!section.contains(dir_b.path().to_string_lossy().as_ref()));
        assert!(section.contains("current source scope"));
    }

    #[test]
    fn test_agent_config_crud() {
        let db = Database::open_memory().unwrap();

        // Save (create)
        let config = db
            .save_agent_config(&SaveAgentConfigInput {
                id: None,
                name: "My GPT-4".into(),
                provider: "openai".into(),
                api_key: "sk-test".into(),
                base_url: None,
                model: "gpt-4o".into(),
                temperature: Some(0.7),
                max_tokens: Some(4096),
                context_window: None,
                is_default: false,
                reasoning_enabled: None,
                thinking_budget: None,
                reasoning_effort: None,
                max_iterations: None,
                summarization_model: None,
                summarization_provider: None,
                subagent_allowed_tools: None,
                subagent_allowed_skill_ids: None,
                subagent_max_parallel: None,
                subagent_max_calls_per_turn: None,
                subagent_token_budget: None,
                tool_timeout_secs: None,
                agent_timeout_secs: None,
            })
            .unwrap();
        assert_eq!(config.name, "My GPT-4");

        // List
        let all = db.list_agent_configs().unwrap();
        assert_eq!(all.len(), 1);

        // Update (upsert)
        let updated = db
            .save_agent_config(&SaveAgentConfigInput {
                id: Some(config.id.clone()),
                name: "Renamed".into(),
                provider: "openai".into(),
                api_key: "sk-test2".into(),
                base_url: None,
                model: "gpt-4o".into(),
                temperature: Some(0.5),
                max_tokens: Some(8192),
                context_window: None,
                is_default: false,
                reasoning_enabled: None,
                thinking_budget: None,
                reasoning_effort: None,
                max_iterations: None,
                summarization_model: None,
                summarization_provider: None,
                subagent_allowed_tools: None,
                subagent_allowed_skill_ids: None,
                subagent_max_parallel: None,
                subagent_max_calls_per_turn: None,
                subagent_token_budget: None,
                tool_timeout_secs: None,
                agent_timeout_secs: None,
            })
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.api_key, "sk-test2");

        // Delete
        db.delete_agent_config(&config.id).unwrap();
        assert!(db.get_agent_config(&config.id).is_err());
    }

    #[test]
    fn test_agent_config_normalizes_base_url() {
        let db = Database::open_memory().unwrap();

        let config = db
            .save_agent_config(&SaveAgentConfigInput {
                id: None,
                name: "Qwen".into(),
                provider: "qwen".into(),
                api_key: "sk-test".into(),
                base_url: Some("  https://dashscope.aliyuncs.com/compatible-mode/v1/  ".into()),
                model: "qwen3.5-plus".into(),
                temperature: Some(0.3),
                max_tokens: Some(256),
                context_window: None,
                is_default: false,
                reasoning_enabled: None,
                thinking_budget: None,
                reasoning_effort: None,
                max_iterations: None,
                summarization_model: None,
                summarization_provider: None,
                subagent_allowed_tools: None,
                subagent_allowed_skill_ids: None,
                subagent_max_parallel: None,
                subagent_max_calls_per_turn: None,
                subagent_token_budget: None,
                tool_timeout_secs: None,
                agent_timeout_secs: None,
            })
            .unwrap();

        assert_eq!(
            config.base_url.as_deref(),
            Some("https://dashscope.aliyuncs.com/compatible-mode/v1")
        );
    }

    #[test]
    fn test_default_agent_config() {
        let db = Database::open_memory().unwrap();

        // No default initially
        assert!(db.get_default_agent_config().unwrap().is_none());

        let c1 = db
            .save_agent_config(&SaveAgentConfigInput {
                id: None,
                name: "Config A".into(),
                provider: "openai".into(),
                api_key: "key-a".into(),
                base_url: None,
                model: "gpt-4o".into(),
                temperature: None,
                max_tokens: None,
                context_window: None,
                is_default: false,
                reasoning_enabled: None,
                thinking_budget: None,
                reasoning_effort: None,
                max_iterations: None,
                summarization_model: None,
                summarization_provider: None,
                subagent_allowed_tools: None,
                subagent_allowed_skill_ids: None,
                subagent_max_parallel: None,
                subagent_max_calls_per_turn: None,
                subagent_token_budget: None,
                tool_timeout_secs: None,
                agent_timeout_secs: None,
            })
            .unwrap();

        let c2 = db
            .save_agent_config(&SaveAgentConfigInput {
                id: None,
                name: "Config B".into(),
                provider: "openai".into(),
                api_key: "key-b".into(),
                base_url: None,
                model: "gpt-4o-mini".into(),
                temperature: None,
                max_tokens: None,
                context_window: None,
                is_default: false,
                reasoning_enabled: None,
                thinking_budget: None,
                reasoning_effort: None,
                max_iterations: None,
                summarization_model: None,
                summarization_provider: None,
                subagent_allowed_tools: None,
                subagent_allowed_skill_ids: None,
                subagent_max_parallel: None,
                subagent_max_calls_per_turn: None,
                subagent_token_budget: None,
                tool_timeout_secs: None,
                agent_timeout_secs: None,
            })
            .unwrap();

        // Set c1 as default
        db.set_default_agent_config(&c1.id).unwrap();
        let def = db.get_default_agent_config().unwrap().unwrap();
        assert_eq!(def.id, c1.id);

        // Switch to c2
        db.set_default_agent_config(&c2.id).unwrap();
        let def = db.get_default_agent_config().unwrap().unwrap();
        assert_eq!(def.id, c2.id);

        // c1 should no longer be default
        let c1_refetch = db.get_agent_config(&c1.id).unwrap();
        assert!(!c1_refetch.is_default);
    }

    #[test]
    fn test_checkpoint_create_and_restore() {
        let db = Database::open_memory().unwrap();

        // Create a conversation with some messages.
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();

        let msgs: Vec<ConversationMessage> = (0..5)
            .map(|i| {
                let msg = ConversationMessage {
                    id: new_id(),
                    conversation_id: conv.id.clone(),
                    role: if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    content: format!("Message {i}"),
                    tool_call_id: None,
                    tool_calls: vec![],
                    artifacts: if i == 1 {
                        Some(serde_json::json!({ "kind": "verification", "overallStatus": "passed" }))
                    } else {
                        None
                    },
                    token_count: 20,
                    created_at: String::new(),
                    sort_order: i,
                    thinking: None,
                    image_attachments: None,
                };
                db.add_message(&msg).unwrap();
                msg
            })
            .collect();

        // Create checkpoint and archive first 3 messages.
        let cp_id = db.create_checkpoint(&conv.id, "auto", 3, 60).unwrap();
        db.archive_messages(&cp_id, &conv.id, &msgs[..3]).unwrap();

        // List checkpoints.
        let checkpoints = db.list_checkpoints(&conv.id).unwrap();
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].label, "auto");
        assert_eq!(checkpoints[0].message_count, 3);
        assert_eq!(checkpoints[0].estimated_tokens, 60);

        // Restore checkpoint.
        let restored = db.restore_checkpoint(&cp_id).unwrap();
        assert_eq!(restored.len(), 3);
        assert_eq!(restored[0].content, "Message 0");
        assert_eq!(restored[1].content, "Message 1");
        assert_eq!(restored[2].content, "Message 2");
        assert_eq!(restored[0].role, Role::User);
        assert_eq!(restored[1].role, Role::Assistant);
        assert_eq!(
            restored[1].artifacts.as_ref().unwrap()["kind"],
            "verification"
        );
    }

    #[test]
    fn test_checkpoint_delete_cascades() {
        let db = Database::open_memory().unwrap();

        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();

        let msg = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::User,
            content: "Hello".into(),
            tool_call_id: None,
            tool_calls: vec![],
            artifacts: None,
            token_count: 5,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&msg).unwrap();

        let cp_id = db.create_checkpoint(&conv.id, "manual", 1, 5).unwrap();
        db.archive_messages(&cp_id, &conv.id, &[msg]).unwrap();

        // Verify restore works.
        let restored = db.restore_checkpoint(&cp_id).unwrap();
        assert_eq!(restored.len(), 1);

        // Delete checkpoint.
        db.delete_checkpoint(&cp_id).unwrap();

        // Restore should now return empty (checkpoint gone).
        let restored = db.restore_checkpoint(&cp_id);
        assert!(restored.is_err()); // Query on non-existent checkpoint fails.

        // List should be empty.
        let checkpoints = db.list_checkpoints(&conv.id).unwrap();
        assert!(checkpoints.is_empty());
    }
}
