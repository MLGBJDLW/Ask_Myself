//! User preference profile — builds personalization context from feedback history.

use crate::conversation::memory::estimate_tokens;
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, Role};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::warn;
use uuid::Uuid;

/// How a memory was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    Manual,
    AutoExtracted,
}

impl std::fmt::Display for MemorySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemorySource::Manual => write!(f, "manual"),
            MemorySource::AutoExtracted => write!(f, "auto_extracted"),
        }
    }
}

fn parse_memory_source(s: &str) -> MemorySource {
    match s {
        "auto_extracted" => MemorySource::AutoExtracted,
        _ => MemorySource::Manual,
    }
}

/// A user-authored persistent memory note.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMemory {
    pub id: String,
    pub content: String,
    pub source: MemorySource,
    pub created_at: String,
    pub updated_at: String,
}

const USER_MEMORY_MAX_CHARS: usize = 240;
const USER_MEMORY_MAX_ITEMS: usize = 200;

const MEMORY_PROMPT_MAX_ITEMS: usize = 6;
const MEMORY_PROMPT_FALLBACK_ITEMS: usize = 2;
const MEMORY_PROMPT_TOKEN_BUDGET: u32 = 200;
const MEMORY_PROMPT_ITEM_MAX_CHARS: usize = 140;

const PREFERENCE_PROMPT_TOKEN_BUDGET: u32 = 160;
const PREFERENCE_TOPIC_MAX_CHARS: usize = 52;

/// Build a concise preference summary from accumulated feedback data.
/// Returns a Markdown section to be appended to the system prompt.
/// If no meaningful feedback exists, returns an empty string.
pub fn build_preference_summary(db: &Database) -> Result<String, CoreError> {
    build_preference_summary_for_query(db, None)
}

/// Query-aware variant of `build_preference_summary`.
/// Applies progressive disclosure so only compact and relevant preference
/// hints are injected into the system prompt.
pub fn build_preference_summary_for_query(
    db: &Database,
    user_query: Option<&str>,
) -> Result<String, CoreError> {
    let (preferred_sources, avoided_sources, preferred_types, top_queries, total) = {
        let conn = db.conn();

        let mut stmt = conn.prepare(
            "SELECT s.root_path, COUNT(*) as cnt
             FROM feedback f
             JOIN chunks c ON f.chunk_id = c.id
             JOIN documents d ON c.document_id = d.id
             JOIN sources s ON d.source_id = s.id
             WHERE f.action IN ('upvote', 'pin')
             GROUP BY s.root_path
             ORDER BY cnt DESC
             LIMIT 5",
        )?;
        let preferred_sources: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let mut stmt2 = conn.prepare(
            "SELECT s.root_path, COUNT(*) as cnt
             FROM feedback f
             JOIN chunks c ON f.chunk_id = c.id
             JOIN documents d ON c.document_id = d.id
             JOIN sources s ON d.source_id = s.id
             WHERE f.action = 'downvote'
             GROUP BY s.root_path
             ORDER BY cnt DESC
             LIMIT 3",
        )?;
        let avoided_sources: Vec<(String, i64)> = stmt2
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let mut stmt3 = conn.prepare(
            "SELECT d.mime_type, COUNT(*) as cnt
             FROM feedback f
             JOIN chunks c ON f.chunk_id = c.id
             JOIN documents d ON c.document_id = d.id
             WHERE f.action IN ('upvote', 'pin')
             GROUP BY d.mime_type
             ORDER BY cnt DESC
             LIMIT 5",
        )?;
        let preferred_types: Vec<(String, i64)> = stmt3
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let mut stmt4 = conn.prepare(
            "SELECT query_text, COUNT(*) as cnt
             FROM feedback
             WHERE action IN ('upvote', 'pin')
             GROUP BY query_text
             ORDER BY cnt DESC
             LIMIT 8",
        )?;
        let top_queries: Vec<(String, i64)> = stmt4
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM feedback", [], |row| row.get(0))?;

        (
            preferred_sources,
            avoided_sources,
            preferred_types,
            top_queries,
            total,
        )
    };

    if total < 3 {
        return Ok(String::new());
    }

    let query_terms = extract_query_terms(user_query.unwrap_or(""));
    let mut hints: Vec<String> = Vec::new();

    let preferred_source_names: Vec<String> = preferred_sources
        .iter()
        .take(2)
        .map(|(path, _)| extract_dir_name(path).to_string())
        .collect();
    if !preferred_source_names.is_empty() {
        hints.push(format!(
            "Prefer sources: {}",
            preferred_source_names.join(", ")
        ));
    }

    let avoided_source_names: Vec<String> = avoided_sources
        .iter()
        .take(2)
        .map(|(path, _)| extract_dir_name(path).to_string())
        .collect();
    if !avoided_source_names.is_empty() {
        hints.push(format!(
            "Lower-confidence sources: {}",
            avoided_source_names.join(", ")
        ));
    }

    let preferred_mime_types: Vec<String> = preferred_types
        .iter()
        .take(3)
        .map(|(mime, _)| compact_text(mime, 24))
        .collect();
    if !preferred_mime_types.is_empty() {
        hints.push(format!(
            "Preferred content types: {}",
            preferred_mime_types.join(", ")
        ));
    }

    if !top_queries.is_empty() {
        let mut ranked_topics: Vec<(i32, usize, String)> = top_queries
            .iter()
            .enumerate()
            .map(|(idx, (query, _))| {
                (
                    text_relevance_score(query, &query_terms),
                    idx,
                    compact_text(query, PREFERENCE_TOPIC_MAX_CHARS),
                )
            })
            .collect();

        if !query_terms.is_empty() {
            ranked_topics.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }

        let mut topics = Vec::new();
        for (score, _idx, topic) in &ranked_topics {
            if !query_terms.is_empty() && *score <= 0 {
                continue;
            }
            topics.push(topic.clone());
            if topics.len() >= 2 {
                break;
            }
        }

        if topics.is_empty() {
            topics.extend(
                ranked_topics
                    .iter()
                    .take(1)
                    .map(|(_, _, topic)| topic.clone()),
            );
        }

        if !topics.is_empty() {
            hints.push(format!("Successful past topics: {}", topics.join("; ")));
        }
    }

    let bullets = budgeted_bullets(hints, PREFERENCE_PROMPT_TOKEN_BUDGET);
    if bullets.is_empty() {
        return Ok(String::new());
    }

    Ok(format!(
        "\n## User Preferences (progressive, from feedback)\n\n{}\n\nUse these only when relevant to the current request.",
        bullets.join("\n")
    ))
}

/// Build a concise summary of user-authored long-term memories.
pub fn build_memory_summary(db: &Database) -> Result<String, CoreError> {
    build_memory_summary_for_query(db, None)
}

/// Query-aware variant of `build_memory_summary`.
/// Selects only a compact relevant subset under a token budget.
pub fn build_memory_summary_for_query(
    db: &Database,
    user_query: Option<&str>,
) -> Result<String, CoreError> {
    let memories = db.list_user_memories()?;
    if memories.is_empty() {
        return Ok(String::new());
    }

    let compact_memories: Vec<String> = memories
        .iter()
        .map(|m| compact_text(&m.content, MEMORY_PROMPT_ITEM_MAX_CHARS))
        .collect();

    let query_terms = extract_query_terms(user_query.unwrap_or(""));
    let mut selected_indices: Vec<usize> = Vec::new();

    if query_terms.is_empty() {
        selected_indices.extend((0..compact_memories.len()).take(MEMORY_PROMPT_FALLBACK_ITEMS));
    } else {
        let mut ranked: Vec<(i32, usize)> = compact_memories
            .iter()
            .enumerate()
            .map(|(idx, content)| (text_relevance_score(content, &query_terms), idx))
            .collect();

        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

        for (score, idx) in ranked {
            if score <= 0 {
                continue;
            }
            selected_indices.push(idx);
            if selected_indices.len() >= MEMORY_PROMPT_MAX_ITEMS {
                break;
            }
        }

        if selected_indices.is_empty() {
            selected_indices.extend((0..compact_memories.len()).take(1));
        }
    }

    let selected_items: Vec<String> = selected_indices
        .into_iter()
        .map(|idx| compact_memories[idx].clone())
        .collect();

    let bullets = budgeted_bullets(selected_items, MEMORY_PROMPT_TOKEN_BUDGET);
    if bullets.is_empty() {
        return Ok(String::new());
    }

    Ok(format!(
        "\n## User Long-Term Memory (local, progressive)\n\n{}\n\nUse only memories relevant to the current request. If user instructions conflict, follow the latest instruction.",
        bullets.join("\n")
    ))
}

/// Returns source root_paths that the user has positively engaged with (upvote/pin).
/// Returns up to `limit` source paths ordered by engagement count.
pub fn get_preferred_source_paths(db: &Database, limit: usize) -> Result<Vec<String>, CoreError> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT s.root_path, COUNT(*) as cnt
         FROM feedback f
         JOIN chunks c ON c.id = f.chunk_id
         JOIN documents d ON c.document_id = d.id
         JOIN sources s ON d.source_id = s.id
         WHERE f.action IN ('upvote', 'pin')
         GROUP BY s.root_path
         ORDER BY cnt DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(rusqlite::params![limit as i64], |row| {
        let path: String = row.get(0)?;
        Ok(path)
    })?;
    let mut paths = Vec::new();
    for row in rows {
        paths.push(row?);
    }
    Ok(paths)
}

impl Database {
    /// List user memory notes, newest first.
    pub fn list_user_memories(&self) -> Result<Vec<UserMemory>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, content, COALESCE(source, 'manual'), created_at, updated_at
             FROM user_memories
             ORDER BY updated_at DESC, created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let source_str: String = row.get(2)?;
            Ok(UserMemory {
                id: row.get(0)?,
                content: row.get(1)?,
                source: parse_memory_source(&source_str),
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Create a new user memory note (manual source).
    pub fn create_user_memory(&self, content: &str) -> Result<UserMemory, CoreError> {
        self.create_user_memory_with_source(content, MemorySource::Manual)
    }

    /// Create a new user memory note with explicit source.
    pub fn create_user_memory_with_source(
        &self,
        content: &str,
        source: MemorySource,
    ) -> Result<UserMemory, CoreError> {
        let normalized = validate_user_memory_content(content)?;

        let id = Uuid::new_v4().to_string();
        let conn = self.conn();

        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM user_memories", [], |row| row.get(0))?;
        if count >= USER_MEMORY_MAX_ITEMS as i64 {
            return Err(CoreError::InvalidInput(format!(
                "Memory limit reached (max {USER_MEMORY_MAX_ITEMS} entries)"
            )));
        }

        conn.execute(
            "INSERT INTO user_memories (id, content, source)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![&id, &normalized, source.to_string()],
        )?;
        drop(conn);
        self.get_user_memory(&id)
    }

    /// Update an existing user memory note.
    pub fn update_user_memory(&self, id: &str, content: &str) -> Result<UserMemory, CoreError> {
        let normalized = validate_user_memory_content(content)?;

        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE user_memories
             SET content = ?2, updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![id, &normalized],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("UserMemory {id}")));
        }
        drop(conn);
        self.get_user_memory(id)
    }

    /// Delete a user memory note.
    pub fn delete_user_memory(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "DELETE FROM user_memories WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("UserMemory {id}")));
        }
        Ok(())
    }

    fn get_user_memory(&self, id: &str) -> Result<UserMemory, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, content, COALESCE(source, 'manual'), created_at, updated_at
             FROM user_memories
             WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                let source_str: String = row.get(2)?;
                Ok(UserMemory {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    source: parse_memory_source(&source_str),
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("UserMemory {id}")),
            other => CoreError::Database(other),
        })
    }
}

/// Extract the last directory component from a path for display.
fn extract_dir_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }

    let mut out = String::new();
    for ch in text.chars().take(max_chars.saturating_sub(3)) {
        out.push(ch);
    }
    out.push_str("...");
    out
}

fn compact_text(text: &str, max_chars: usize) -> String {
    truncate_chars(&normalize_whitespace(text), max_chars)
}

fn validate_user_memory_content(content: &str) -> Result<String, CoreError> {
    let normalized = normalize_whitespace(content.trim());
    if normalized.is_empty() {
        return Err(CoreError::InvalidInput(
            "Memory content cannot be empty".to_string(),
        ));
    }

    let char_count = normalized.chars().count();
    if char_count > USER_MEMORY_MAX_CHARS {
        return Err(CoreError::InvalidInput(format!(
            "Memory content is too long ({char_count} chars, max {USER_MEMORY_MAX_CHARS})"
        )));
    }

    Ok(normalized)
}

fn budgeted_bullets(items: Vec<String>, token_budget: u32) -> Vec<String> {
    let mut bullets = Vec::new();
    let mut used_tokens = 0u32;

    for item in items {
        let compact = normalize_whitespace(&item);
        if compact.is_empty() {
            continue;
        }

        let bullet = format!("  - {compact}");
        let token_cost = estimate_tokens(&bullet);
        if used_tokens + token_cost > token_budget {
            if bullets.is_empty() {
                let forced = format!("  - {}", truncate_chars(&compact, 72));
                bullets.push(forced);
            }
            break;
        }

        bullets.push(bullet);
        used_tokens += token_cost;
    }

    bullets
}

fn extract_query_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    let mut ascii_buf = String::new();
    let mut cjk_buf = String::new();

    let lowered = text.to_lowercase();
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            flush_cjk_terms(&mut cjk_buf, &mut terms, &mut seen);
            ascii_buf.push(ch);
            continue;
        }

        if is_cjk_char(ch) {
            flush_ascii_term(&mut ascii_buf, &mut terms, &mut seen);
            cjk_buf.push(ch);
            continue;
        }

        flush_ascii_term(&mut ascii_buf, &mut terms, &mut seen);
        flush_cjk_terms(&mut cjk_buf, &mut terms, &mut seen);
    }

    flush_ascii_term(&mut ascii_buf, &mut terms, &mut seen);
    flush_cjk_terms(&mut cjk_buf, &mut terms, &mut seen);
    terms
}

fn push_unique_term(terms: &mut Vec<String>, seen: &mut HashSet<String>, term: String) {
    if term.is_empty() || !seen.insert(term.clone()) {
        return;
    }
    terms.push(term);
}

fn flush_ascii_term(ascii_buf: &mut String, terms: &mut Vec<String>, seen: &mut HashSet<String>) {
    if ascii_buf.len() >= 2 {
        push_unique_term(terms, seen, std::mem::take(ascii_buf));
    } else {
        ascii_buf.clear();
    }
}

fn flush_cjk_terms(cjk_buf: &mut String, terms: &mut Vec<String>, seen: &mut HashSet<String>) {
    if cjk_buf.is_empty() {
        return;
    }

    let chars: Vec<char> = cjk_buf.chars().collect();
    if chars.len() >= 2 {
        for window in chars.windows(2) {
            let term: String = window.iter().collect();
            push_unique_term(terms, seen, term);
        }
    } else {
        push_unique_term(terms, seen, chars[0].to_string());
    }

    cjk_buf.clear();
}

fn text_relevance_score(text: &str, query_terms: &[String]) -> i32 {
    if query_terms.is_empty() {
        return 0;
    }

    let haystack = text.to_lowercase();
    let mut matches = 0i32;
    let mut longest = 0usize;

    for term in query_terms {
        if haystack.contains(term) {
            matches += 1;
            longest = longest.max(term.chars().count());
        }
    }

    if matches == 0 {
        return 0;
    }

    let long_match_bonus = if longest >= 4 { 2 } else { 0 };
    matches * 2 + long_match_bonus
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
            | '\u{3400}'..='\u{4DBF}' // CJK Extension A
            | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
            | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
            | '\u{3040}'..='\u{309F}' // Hiragana
            | '\u{30A0}'..='\u{30FF}' // Katakana
            | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
    )
}

// ---------------------------------------------------------------------------
// Auto memory extraction
// ---------------------------------------------------------------------------

/// Maximum tokens the memory extraction LLM call may produce.
const EXTRACT_MAX_TOKENS: u32 = 400;

/// Maximum input characters sent to the extraction prompt.
const EXTRACT_MAX_INPUT: usize = 6_000;

/// Maximum characters per individual message included in the extraction input.
const EXTRACT_MSG_CAP: usize = 500;

/// Minimum number of user+assistant messages before extraction is worthwhile.
const EXTRACT_MIN_MESSAGES: usize = 5;

/// Minimum turns since the last extraction before we extract again.
/// Stored as a conversation-level key in the DB app_config table.
const EXTRACT_MIN_TURN_INTERVAL: usize = 5;

const EXTRACT_SYSTEM_PROMPT: &str = r#"You are a memory extraction assistant. Analyze the conversation and extract key personal facts, preferences, or decisions the user has shared that would be useful to remember in future conversations.

Rules:
- Only extract information explicitly stated by the user, not inferred.
- Each memory should be a single concise sentence (max 200 chars).
- Do NOT duplicate any existing memories listed below.
- Be highly selective — only truly important, reusable facts.
- Return a JSON array of strings. Return [] if nothing worth remembering.
- Output ONLY the JSON array, no other text."#;

/// Extract potential memories from conversation messages using an LLM.
///
/// Returns a list of memory strings ready to be saved. An empty list means
/// nothing worth remembering was found (or the conversation is too short).
pub async fn extract_memories_from_conversation(
    messages: &[crate::conversation::ConversationMessage],
    existing_memories: &[UserMemory],
    llm: &dyn LlmProvider,
    model: &str,
) -> Result<Vec<String>, CoreError> {
    // Only consider user + assistant messages.
    let relevant: Vec<_> = messages
        .iter()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant))
        .collect();

    if relevant.len() < EXTRACT_MIN_MESSAGES {
        return Ok(vec![]);
    }

    // Check if user provided substantial content (not just "yes"/"ok").
    let user_content_len: usize = relevant
        .iter()
        .filter(|m| matches!(m.role, Role::User))
        .map(|m| m.content.trim().len())
        .sum();
    if user_content_len < 100 {
        return Ok(vec![]);
    }

    // Build existing-memories block.
    let existing_block = if existing_memories.is_empty() {
        "(none)".to_string()
    } else {
        existing_memories
            .iter()
            .map(|m| format!("- {}", &m.content))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // Build conversation transcript.
    let mut transcript_parts = Vec::new();
    for msg in &relevant {
        let role_label = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            _ => continue,
        };
        let text = if msg.content.len() > EXTRACT_MSG_CAP {
            &msg.content[..EXTRACT_MSG_CAP]
        } else {
            &msg.content
        };
        transcript_parts.push(format!("{role_label}: {text}"));
    }
    let mut transcript = transcript_parts.join("\n");
    if transcript.len() > EXTRACT_MAX_INPUT {
        transcript.truncate(EXTRACT_MAX_INPUT);
    }

    let user_prompt = format!(
        "Existing memories (do not duplicate):\n{existing_block}\n\nConversation:\n{transcript}"
    );

    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(Role::System, EXTRACT_SYSTEM_PROMPT),
            Message::text(Role::User, user_prompt),
        ],
        max_tokens: Some(EXTRACT_MAX_TOKENS),
        temperature: Some(0.2),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
    };

    let response = llm.complete(&request).await?;
    let text = response.content.trim();

    parse_memory_json(text)
}

/// Parse the LLM response as a JSON array of strings.
fn parse_memory_json(text: &str) -> Result<Vec<String>, CoreError> {
    // Try to find a JSON array in the response (the LLM may wrap it in markdown).
    let json_text = if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            &text[start..=end]
        } else {
            text
        }
    } else {
        text
    };

    let parsed: Vec<String> = serde_json::from_str(json_text).unwrap_or_default();

    // Filter out empty/too-long entries.
    Ok(parsed
        .into_iter()
        .filter(|s| {
            let trimmed = s.trim();
            !trimmed.is_empty() && trimmed.len() <= USER_MEMORY_MAX_CHARS
        })
        .collect())
}

/// Run auto memory extraction for a conversation, saving results to DB.
///
/// This is designed to be called from a background task after a successful
/// agent turn. It checks rate-limiting internally.
pub async fn auto_extract_and_save(
    db: &Database,
    conversation_id: &str,
    llm: &dyn LlmProvider,
    model: &str,
) -> Result<usize, CoreError> {
    // Load conversation messages.
    let messages = db.get_messages(conversation_id)?;

    // Rate-limit: count user messages since we don't track extraction per-conversation.
    let user_msg_count = messages
        .iter()
        .filter(|m| matches!(m.role, Role::User))
        .count();
    if user_msg_count < EXTRACT_MIN_TURN_INTERVAL {
        return Ok(0);
    }

    // Only extract every EXTRACT_MIN_TURN_INTERVAL user messages.
    // Use modular arithmetic: extract when user_msg_count is a multiple.
    if user_msg_count % EXTRACT_MIN_TURN_INTERVAL != 0 {
        return Ok(0);
    }

    let existing = db.list_user_memories()?;
    let extracted = extract_memories_from_conversation(&messages, &existing, llm, model).await?;

    let mut saved = 0usize;
    for memory_text in extracted {
        // Double-check deduplication: skip if any existing memory contains the same text.
        let dominated = existing.iter().any(|m| {
            m.content.to_lowercase().contains(&memory_text.to_lowercase())
                || memory_text.to_lowercase().contains(&m.content.to_lowercase())
        });
        if dominated {
            continue;
        }

        match db.create_user_memory_with_source(&memory_text, MemorySource::AutoExtracted) {
            Ok(_) => saved += 1,
            Err(e) => {
                warn!("Failed to save auto-extracted memory: {e}");
                break; // Likely hit the limit.
            }
        }
    }

    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::feedback::FeedbackAction;
    use rusqlite::params;

    fn setup_source_with_chunks(db: &Database, root_path: &str) -> Vec<String> {
        let source_id = uuid::Uuid::new_v4().to_string();
        let doc_id = uuid::Uuid::new_v4().to_string();

        let conn = db.conn();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path, include_globs, exclude_globs, watch_enabled)
             VALUES (?1, 'local_folder', ?2, '[]', '[]', 0)",
            params![&source_id, root_path],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO documents (id, source_id, path, mime_type, file_size, modified_at, content_hash)
             VALUES (?1, ?2, ?3, 'text/plain', 100, datetime('now'), 'hash')",
            params![&doc_id, &source_id, format!("{root_path}/doc.md")],
        )
        .unwrap();

        let mut chunk_ids = Vec::new();
        for i in 0..2 {
            let cid = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, ?3, 'text', 'content', 0, 7, 1, 1, ?4)",
                params![&cid, &doc_id, i, format!("hash{i}")],
            )
            .unwrap();
            chunk_ids.push(cid);
        }
        drop(conn);
        chunk_ids
    }

    #[test]
    fn test_get_preferred_source_paths() {
        let db = Database::open_memory().expect("open_memory");

        let chunks_a = setup_source_with_chunks(&db, "/home/user/notes");
        let chunks_b = setup_source_with_chunks(&db, "/home/user/docs");

        // 3 upvotes on source A, 1 on source B
        for cid in &chunks_a {
            db.add_feedback(cid, "query", FeedbackAction::Upvote)
                .unwrap();
        }
        db.add_feedback(&chunks_a[0], "query2", FeedbackAction::Pin)
            .unwrap();
        db.add_feedback(&chunks_b[0], "query", FeedbackAction::Upvote)
            .unwrap();

        let paths = get_preferred_source_paths(&db, 5).unwrap();
        assert_eq!(paths.len(), 2);
        assert_eq!(
            paths[0], "/home/user/notes",
            "source with most engagement should be first"
        );
        assert_eq!(paths[1], "/home/user/docs");
    }

    #[test]
    fn test_get_preferred_source_paths_empty() {
        let db = Database::open_memory().expect("open_memory");
        let paths = get_preferred_source_paths(&db, 5).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn test_get_preferred_source_paths_excludes_downvotes() {
        let db = Database::open_memory().expect("open_memory");
        let chunks = setup_source_with_chunks(&db, "/home/user/bad");

        // Only downvotes — should NOT appear
        db.add_feedback(&chunks[0], "query", FeedbackAction::Downvote)
            .unwrap();

        let paths = get_preferred_source_paths(&db, 5).unwrap();
        assert!(paths.is_empty());
    }

    #[test]
    fn test_user_memory_crud() {
        let db = Database::open_memory().expect("open_memory");

        let created = db.create_user_memory("I prefer concise answers.").unwrap();
        assert_eq!(created.content, "I prefer concise answers.");

        let listed = db.list_user_memories().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);

        let updated = db
            .update_user_memory(&created.id, "I prefer concise, bullet answers.")
            .unwrap();
        assert!(updated.content.contains("bullet"));

        db.delete_user_memory(&created.id).unwrap();
        assert!(db.list_user_memories().unwrap().is_empty());
    }

    #[test]
    fn test_build_memory_summary() {
        let db = Database::open_memory().expect("open_memory");
        assert!(build_memory_summary(&db).unwrap().is_empty());

        db.create_user_memory("My name is Alex.").unwrap();
        db.create_user_memory("Default language should be Chinese.")
            .unwrap();

        let summary = build_memory_summary(&db).unwrap();
        assert!(summary.contains("User Long-Term Memory"));
        assert!(summary.contains("My name is Alex."));
        assert!(summary.contains("Default language should be Chinese."));
    }

    #[test]
    fn test_build_memory_summary_for_query_is_progressive() {
        let db = Database::open_memory().expect("open_memory");

        db.create_user_memory("Old memory one.").unwrap();
        db.create_user_memory("Another unrelated memory.").unwrap();
        db.create_user_memory("Preferred language is Chinese.")
            .unwrap();

        let summary =
            build_memory_summary_for_query(&db, Some("language preference for this answer"))
                .unwrap();

        assert!(summary.contains("Preferred language is Chinese."));
        assert!(
            !summary.contains("Old memory one."),
            "irrelevant older memory should not always be injected"
        );
    }

    #[test]
    fn test_user_memory_rejects_too_long_content() {
        let db = Database::open_memory().expect("open_memory");
        let too_long = "x".repeat(USER_MEMORY_MAX_CHARS + 1);

        let err = db.create_user_memory(&too_long).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidInput(_)),
            "should reject overlong memory content"
        );
    }

    #[test]
    fn test_extract_query_terms_mixed_language() {
        let terms = extract_query_terms("请用中文回答 and keep concise");
        assert!(terms.contains(&"中文".to_string()));
        assert!(terms.contains(&"concise".to_string()));
    }
}
