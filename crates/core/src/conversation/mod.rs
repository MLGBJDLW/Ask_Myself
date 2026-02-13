//! Conversation persistence — types and CRUD for conversations, messages, and agent configs.

pub mod memory;

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
    pub created_at: String,
    pub updated_at: String,
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
    pub token_count: u32,
    pub created_at: String,
    pub sort_order: i64,
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
    pub created_at: String,
    pub updated_at: String,
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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn new_id() -> String {
    Uuid::new_v4().to_string()
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
        let conn = self.conn();
        conn.execute(
            "INSERT INTO conversations (id, provider, model, system_prompt)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![&id, &input.provider, &input.model, system_prompt],
        )?;
        drop(conn);
        self.get_conversation(&id)
    }

    /// List conversations ordered by most-recently updated first.
    pub fn list_conversations(&self) -> Result<Vec<Conversation>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, provider, model, system_prompt, created_at, updated_at
             FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                provider: row.get(2)?,
                model: row.get(3)?,
                system_prompt: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
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
            "SELECT id, title, provider, model, system_prompt, created_at, updated_at
             FROM conversations WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    provider: row.get(2)?,
                    model: row.get(3)?,
                    system_prompt: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
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

    /// Update the title of a conversation (also bumps `updated_at`).
    pub fn update_conversation_title(
        &self,
        id: &str,
        title: &str,
    ) -> Result<(), CoreError> {
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

        let conn = self.conn();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_call_id, tool_calls_json, token_count, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                &msg.id,
                &msg.conversation_id,
                role_str,
                &msg.content,
                &msg.tool_call_id,
                &tool_calls_json,
                msg.token_count,
                msg.sort_order,
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
            "SELECT id, conversation_id, role, content, tool_call_id, tool_calls_json, token_count, created_at, sort_order
             FROM messages WHERE conversation_id = ?1 ORDER BY sort_order ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![conversation_id], |row| {
            let role_str: String = row.get(2)?;
            let tool_calls_json: Option<String> = row.get(5)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                role_str,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                tool_calls_json,
                row.get::<_, u32>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (id, conv_id, role_str, content, tool_call_id, tc_json, token_count, created_at, sort_order) = row?;
            let tool_calls: Vec<ToolCallRequest> = match tc_json {
                Some(json) => serde_json::from_str(&json)?,
                None => Vec::new(),
            };
            results.push(ConversationMessage {
                id,
                conversation_id: conv_id,
                role: str_to_role(&role_str),
                content,
                tool_call_id,
                tool_calls,
                token_count,
                created_at,
                sort_order,
            });
        }
        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// AgentConfig CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// Upsert an agent config. Returns the persisted row.
    pub fn save_agent_config(
        &self,
        input: &SaveAgentConfigInput,
    ) -> Result<AgentConfig, CoreError> {
        let id = input.id.clone().unwrap_or_else(new_id);
        let conn = self.conn();
        conn.execute(
            "INSERT INTO agent_configs (id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
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
                updated_at = datetime('now')",
            rusqlite::params![
                &id,
                &input.name,
                &input.provider,
                &input.api_key,
                &input.base_url,
                &input.model,
                input.temperature,
                input.max_tokens,
                input.context_window,
                input.is_default as i32,
            ],
        )?;
        drop(conn);
        self.get_agent_config(&id)
    }

    /// List all agent configs.
    pub fn list_agent_configs(&self) -> Result<Vec<AgentConfig>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default, created_at, updated_at
             FROM agent_configs ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
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
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get a single agent config by id.
    pub fn get_agent_config(&self, id: &str) -> Result<AgentConfig, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default, created_at, updated_at
             FROM agent_configs WHERE id = ?1",
            rusqlite::params![id],
            |row| {
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
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("AgentConfig {id}"))
            }
            other => CoreError::Database(other),
        })
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
            "SELECT id, name, provider, api_key, base_url, model, temperature, max_tokens, context_window, is_default, created_at, updated_at
             FROM agent_configs WHERE is_default = 1 LIMIT 1",
            [],
            |row| {
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
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            },
        );
        match result {
            Ok(config) => Ok(Some(config)),
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
    pub fn unlink_source(
        &self,
        conversation_id: &str,
        source_id: &str,
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM conversation_sources
             WHERE conversation_id = ?1 AND source_id = ?2",
            rusqlite::params![conversation_id, source_id],
        )?;
        Ok(())
    }

    /// Get all source IDs linked to a conversation.
    pub fn get_linked_sources(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<String>, CoreError> {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_crud() {
        let db = Database::open_memory().unwrap();

        // Create
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: Some("You are helpful.".into()),
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
    fn test_message_crud() {
        let db = Database::open_memory().unwrap();
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
            })
            .unwrap();

        let msg = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::User,
            content: "Hello!".into(),
            tool_call_id: None,
            tool_calls: vec![],
            token_count: 2,
            created_at: String::new(),
            sort_order: 0,
        };
        db.add_message(&msg).unwrap();

        // Add assistant message with tool calls
        let tc = crate::llm::ToolCallRequest {
            id: "call_1".into(),
            name: "search".into(),
            arguments: r#"{"q":"rust"}"#.into(),
        };
        let msg2 = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![tc],
            token_count: 10,
            created_at: String::new(),
            sort_order: 1,
        };
        db.add_message(&msg2).unwrap();

        let messages = db.get_messages(&conv.id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, Role::User);
        assert_eq!(messages[1].tool_calls.len(), 1);
        assert_eq!(messages[1].tool_calls[0].name, "search");
    }

    #[test]
    fn test_message_cascade_delete() {
        let db = Database::open_memory().unwrap();
        let conv = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                system_prompt: None,
            })
            .unwrap();
        let msg = ConversationMessage {
            id: new_id(),
            conversation_id: conv.id.clone(),
            role: Role::User,
            content: "test".into(),
            tool_call_id: None,
            tool_calls: vec![],
            token_count: 1,
            created_at: String::new(),
            sort_order: 0,
        };
        db.add_message(&msg).unwrap();

        db.delete_conversation(&conv.id).unwrap();
        // Messages should be gone (CASCADE)
        let messages = db.get_messages(&conv.id).unwrap();
        assert!(messages.is_empty());
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
            })
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.api_key, "sk-test2");

        // Delete
        db.delete_agent_config(&config.id).unwrap();
        assert!(db.get_agent_config(&config.id).is_err());
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
}
