//! Message-level feedback + learned-success retrieval loop.
//!
//! This module complements the chunk-level `feedback` module by capturing
//! signals at the **message** level:
//!
//! 1. `message_feedback` — per-message thumbs up/down/cleared, persisted so
//!    it survives across sessions and can power analytics.
//! 2. `learned_successes` — distilled (user_query, response_summary) pairs
//!    derived from upvoted turns. They hold an embedding of the user query
//!    and are retrieved by cosine similarity when a new user message comes
//!    in, to inject a few-shot "here's something you've handled well
//!    before" section into the system prompt.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::db::Database;
use crate::embed::{blob_to_vector, cosine_similarity, vector_to_blob};
use crate::error::CoreError;
use crate::llm::{CompletionRequest, LlmProvider, Message, Role};

// ── Types ────────────────────────────────────────────────────────────

/// Persisted message-level feedback row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedback {
    pub id: String,
    pub message_id: String,
    pub conversation_id: String,
    /// `+1` = upvote, `-1` = downvote, `0` = cleared/unset (row kept for history).
    pub rating: i32,
    pub note: Option<String>,
    pub created_at: String,
}

/// Distilled (user_query → response_summary) example derived from a
/// positively-rated assistant turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearnedSuccess {
    pub id: String,
    pub user_query: String,
    pub response_summary: String,
    pub source_message_id: String,
    pub created_at: String,
}

/// Hard cap on distilled text so the injected few-shot stays compact.
pub const LEARNED_TEXT_MAX_CHARS: usize = 500;

/// Truncate a free-form string to a safe, char-boundary-respecting length.
pub fn distill_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = String::with_capacity(max_chars * 4);
    for (i, ch) in trimmed.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// Maximum retries for transient / rate-limited errors during LLM distillation.
const DISTILL_LLM_MAX_RETRIES: u32 = 1;

/// Char-boundary-safe truncation that preserves the `distill_text` contract
/// on possibly-overlong LLM output.
fn cap_to_max_chars(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = String::with_capacity(max_chars * 4);
    for (i, ch) in trimmed.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

/// LLM-powered distillation for a learned-success response summary.
///
/// Uses a short, targeted system prompt to compress `text` into ≤`max_chars`
/// characters while preserving retrieval-relevant keywords. On any error,
/// empty response, rate-limit exhaustion, or transient failure exceeding the
/// retry budget, falls back to the cheap character-truncation path
/// ([`distill_text`]) so the caller always gets a usable summary.
///
/// The returned string is defensively char-truncated to `max_chars` so the
/// length contract holds even if the model ignores the instruction.
pub async fn distill_text_llm(
    provider: &dyn LlmProvider,
    model: &str,
    text: &str,
    max_chars: usize,
) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Skip the LLM round-trip when already short enough.
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let system_prompt = format!(
        "You are a retrieval-oriented summarizer. Summarize the user's intent and the \
         assistant's solution in at most {max_chars} characters. Keep keywords that aid \
         retrieval. No prose, no pleasantries, no preamble — output the summary only."
    );

    let request = CompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message::text(Role::System, system_prompt),
            Message::text(Role::User, format!("Summarize this:\n\n{}", trimmed)),
        ],
        max_tokens: Some(256),
        temperature: Some(0.2),
        tools: None,
        stop: None,
        thinking_budget: None,
        reasoning_effort: None,
        provider_type: None,
        parallel_tool_calls: true,
    };

    let mut retry_count = 0u32;
    loop {
        match provider.complete(&request).await {
            Ok(response) => {
                let body = response.content.trim();
                if body.is_empty() {
                    return distill_text(text, max_chars);
                }
                return cap_to_max_chars(body, max_chars);
            }
            Err(CoreError::RateLimited { retry_after_secs }) => {
                retry_count += 1;
                if retry_count > DISTILL_LLM_MAX_RETRIES {
                    warn!(
                        "distill_text_llm: rate limited after {} retries, falling back to char truncation",
                        DISTILL_LLM_MAX_RETRIES
                    );
                    return distill_text(text, max_chars);
                }
                let wait = if retry_after_secs > 0 {
                    retry_after_secs
                } else {
                    2u64.pow(retry_count)
                };
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
            Err(CoreError::TransientLlm(msg)) => {
                retry_count += 1;
                if retry_count > DISTILL_LLM_MAX_RETRIES {
                    warn!(
                        "distill_text_llm: transient error after {} retries ({msg}), falling back",
                        DISTILL_LLM_MAX_RETRIES
                    );
                    return distill_text(text, max_chars);
                }
                let wait = 2u64.pow(retry_count - 1);
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            }
            Err(e) => {
                warn!("distill_text_llm: non-retryable error: {e}, falling back");
                return distill_text(text, max_chars);
            }
        }
    }
}

/// Async wrapper around [`Database::insert_learned_success`] that first
/// distills the response summary via the LLM when a `provider`/`model` pair
/// is supplied, otherwise uses the cheap [`distill_text`] path. The
/// `user_query` side is always distilled with the cheap path because
/// keyword-preserving truncation is better for embedding-based retrieval.
///
/// LLM distillation is best-effort: if the call fails, the fallback path in
/// [`distill_text_llm`] guarantees a non-empty summary, so the insert never
/// fails due to LLM issues.
pub async fn insert_learned_success_with_llm(
    db: &Database,
    user_query: &str,
    response_content: &str,
    source_message_id: &str,
    provider: Option<&dyn LlmProvider>,
    model: Option<&str>,
) -> Result<String, CoreError> {
    let user_query_distilled = distill_text(user_query, LEARNED_TEXT_MAX_CHARS);
    let response_summary = match (provider, model) {
        (Some(p), Some(m)) if !m.is_empty() => {
            distill_text_llm(p, m, response_content, LEARNED_TEXT_MAX_CHARS).await
        }
        _ => distill_text(response_content, LEARNED_TEXT_MAX_CHARS),
    };
    db.insert_learned_success(&user_query_distilled, &response_summary, source_message_id)
}

// ── Database methods ─────────────────────────────────────────────────

impl Database {
    /// Upsert message-level feedback for `message_id`.
    ///
    /// `rating = 0` clears the signal but keeps the row (so we preserve a
    /// history of "user was unsure"). Re-clicking the same button with a
    /// different rating overwrites the previous value.
    pub fn set_message_feedback(
        &self,
        message_id: &str,
        conversation_id: &str,
        rating: i32,
        note: Option<&str>,
    ) -> Result<MessageFeedback, CoreError> {
        let conn = self.conn();
        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT id, created_at FROM message_feedback WHERE message_id = ?1",
                params![message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        match existing {
            Some((id, created_at)) => {
                conn.execute(
                    "UPDATE message_feedback SET rating = ?2, note = ?3 WHERE id = ?1",
                    params![&id, rating, note],
                )?;
                Ok(MessageFeedback {
                    id,
                    message_id: message_id.to_string(),
                    conversation_id: conversation_id.to_string(),
                    rating,
                    note: note.map(str::to_string),
                    created_at,
                })
            }
            None => {
                let id = Uuid::new_v4().to_string();
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO message_feedback (id, message_id, conversation_id, rating, note, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![&id, message_id, conversation_id, rating, note, &now],
                )?;
                Ok(MessageFeedback {
                    id,
                    message_id: message_id.to_string(),
                    conversation_id: conversation_id.to_string(),
                    rating,
                    note: note.map(str::to_string),
                    created_at: now,
                })
            }
        }
    }

    /// Fetch the current feedback row for a message, if any.
    pub fn get_message_feedback(
        &self,
        message_id: &str,
    ) -> Result<Option<MessageFeedback>, CoreError> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT id, message_id, conversation_id, rating, note, created_at
                 FROM message_feedback WHERE message_id = ?1",
                params![message_id],
                |row| {
                    Ok(MessageFeedback {
                        id: row.get(0)?,
                        message_id: row.get(1)?,
                        conversation_id: row.get(2)?,
                        rating: row.get(3)?,
                        note: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// List recent upvotes, newest first.
    pub fn list_positive_recent(&self, limit: usize) -> Result<Vec<MessageFeedback>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, message_id, conversation_id, rating, note, created_at
             FROM message_feedback
             WHERE rating > 0
             ORDER BY created_at DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(MessageFeedback {
                id: row.get(0)?,
                message_id: row.get(1)?,
                conversation_id: row.get(2)?,
                rating: row.get(3)?,
                note: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Count message-level feedback entries over the last `days` days,
    /// returning `(positive, total_non_zero)`.
    pub fn count_message_feedback_recent(&self, days: u32) -> Result<(i64, i64), CoreError> {
        let conn = self.conn();
        let cutoff = format!("-{days} days");
        let positive: i64 = conn.query_row(
            "SELECT COUNT(*) FROM message_feedback
             WHERE rating > 0 AND created_at >= datetime('now', ?1)",
            params![cutoff],
            |row| row.get(0),
        )?;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM message_feedback
             WHERE rating != 0 AND created_at >= datetime('now', ?1)",
            params![cutoff],
            |row| row.get(0),
        )?;
        Ok((positive, total))
    }

    /// Insert a new learned-success row (embedding is populated later).
    ///
    /// Returns the new row id. Idempotent per `source_message_id`: if a row
    /// already exists for this message, returns its id without inserting.
    pub fn insert_learned_success(
        &self,
        user_query: &str,
        response_summary: &str,
        source_message_id: &str,
    ) -> Result<String, CoreError> {
        let conn = self.conn();
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM learned_successes WHERE source_message_id = ?1",
                params![source_message_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }

        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO learned_successes (id, user_query, response_summary, source_message_id)
             VALUES (?1, ?2, ?3, ?4)",
            params![&id, user_query, response_summary, source_message_id],
        )?;
        Ok(id)
    }

    /// Attach an embedding to a learned-success row.
    pub fn update_learned_success_embedding(
        &self,
        id: &str,
        embedding: &[f32],
    ) -> Result<(), CoreError> {
        let blob = vector_to_blob(embedding);
        let conn = self.conn();
        conn.execute(
            "UPDATE learned_successes SET query_embedding = ?2 WHERE id = ?1",
            params![id, blob],
        )?;
        Ok(())
    }

    /// Load all learned successes that already have an embedding attached.
    pub fn list_learned_successes_with_embedding(
        &self,
    ) -> Result<Vec<(LearnedSuccess, Vec<f32>)>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, user_query, response_summary, source_message_id, created_at, query_embedding
             FROM learned_successes
             WHERE query_embedding IS NOT NULL
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(5)?;
            Ok((
                LearnedSuccess {
                    id: row.get(0)?,
                    user_query: row.get(1)?,
                    response_summary: row.get(2)?,
                    source_message_id: row.get(3)?,
                    created_at: row.get(4)?,
                },
                blob_to_vector(&blob),
            ))
        })?;
        Ok(rows.filter_map(Result::ok).collect())
    }

    /// Find the most recent user-role message that was inserted *before*
    /// the given assistant message in the same conversation.
    pub fn find_preceding_user_message(
        &self,
        assistant_message_id: &str,
    ) -> Result<Option<(String, String)>, CoreError> {
        let conn = self.conn();
        let target: Option<(String, i64)> = conn
            .query_row(
                "SELECT conversation_id, sort_order FROM messages WHERE id = ?1",
                params![assistant_message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        let Some((conv_id, sort_order)) = target else {
            return Ok(None);
        };

        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT id, content FROM messages
                 WHERE conversation_id = ?1 AND role = 'user' AND sort_order < ?2
                 ORDER BY sort_order DESC
                 LIMIT 1",
                params![conv_id, sort_order],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }

    /// Fetch `(role, content)` for a single message.
    pub fn get_message_role_and_content(
        &self,
        message_id: &str,
    ) -> Result<Option<(String, String)>, CoreError> {
        let conn = self.conn();
        let row = conn
            .query_row(
                "SELECT role, content FROM messages WHERE id = ?1",
                params![message_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row)
    }
}

// ── Retrieval ────────────────────────────────────────────────────────

/// Minimum cosine similarity below which a candidate is discarded.
pub const LEARNED_SIMILARITY_THRESHOLD: f32 = 0.7;

/// Retrieve the top-`top_k` learned successes most similar to
/// `query_embedding`, filtering out anything below
/// [`LEARNED_SIMILARITY_THRESHOLD`].
pub fn retrieve_similar_successes(
    db: &Database,
    query_embedding: &[f32],
    top_k: usize,
) -> Result<Vec<(LearnedSuccess, f32)>, CoreError> {
    if top_k == 0 || query_embedding.is_empty() {
        return Ok(Vec::new());
    }
    let candidates = db.list_learned_successes_with_embedding()?;
    let mut scored: Vec<(LearnedSuccess, f32)> = candidates
        .into_iter()
        .filter_map(|(row, emb)| {
            if emb.len() != query_embedding.len() {
                return None;
            }
            let sim = cosine_similarity(query_embedding, &emb);
            if sim >= LEARNED_SIMILARITY_THRESHOLD {
                Some((row, sim))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    Ok(scored)
}

/// Build the "Learned Successes" prompt section from retrieved examples.
///
/// Returns an empty string when `successes` is empty so the caller can just
/// feed it into the existing system-prompt builder (which skips empty
/// sections).
pub fn build_learned_successes_section(successes: &[(LearnedSuccess, f32)]) -> String {
    if successes.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Learned Successes (from past positive feedback)\n\
         Here are similar queries you've handled well before:\n",
    );
    for (s, _score) in successes {
        out.push_str("- Q: ");
        out.push_str(&s.user_query);
        out.push_str(" → A: ");
        out.push_str(&s.response_summary);
        out.push('\n');
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{CompletionResponse, FinishReason, StreamChunk, Usage};
    use async_trait::async_trait;
    use futures::stream::{self, BoxStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn open_db_with_conv_and_msgs() -> (Database, String, String, String) {
        let db = Database::open_memory().expect("open_memory");
        let conv_id = Uuid::new_v4().to_string();
        let user_id = Uuid::new_v4().to_string();
        let asst_id = Uuid::new_v4().to_string();
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO conversations (id, title, provider, model, system_prompt)
                 VALUES (?1, 'T', 'openai', 'gpt-4o-mini', '')",
                params![&conv_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, sort_order)
                 VALUES (?1, ?2, 'user', 'How do I sort a Rust vector?', 1)",
                params![&user_id, &conv_id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, sort_order)
                 VALUES (?1, ?2, 'assistant', 'Use Vec::sort or sort_by.', 2)",
                params![&asst_id, &conv_id],
            )
            .unwrap();
        }
        (db, conv_id, user_id, asst_id)
    }

    #[test]
    fn set_feedback_upsert_and_get() {
        let (db, conv_id, _u, asst_id) = open_db_with_conv_and_msgs();

        let fb1 = db
            .set_message_feedback(&asst_id, &conv_id, 1, Some("great"))
            .unwrap();
        assert_eq!(fb1.rating, 1);

        let fb2 = db
            .set_message_feedback(&asst_id, &conv_id, -1, None)
            .unwrap();
        assert_eq!(fb2.id, fb1.id, "re-click should overwrite, not insert");
        assert_eq!(fb2.rating, -1);

        let got = db.get_message_feedback(&asst_id).unwrap().unwrap();
        assert_eq!(got.rating, -1);
    }

    #[test]
    fn find_preceding_user_message_works() {
        let (db, _c, user_id, asst_id) = open_db_with_conv_and_msgs();
        let (found_id, found_content) = db.find_preceding_user_message(&asst_id).unwrap().unwrap();
        assert_eq!(found_id, user_id);
        assert!(found_content.contains("sort"));
    }

    #[test]
    fn insert_learned_success_is_idempotent_per_message() {
        let (db, _c, _u, asst_id) = open_db_with_conv_and_msgs();
        let id1 = db.insert_learned_success("q", "a", &asst_id).unwrap();
        let id2 = db.insert_learned_success("q", "a", &asst_id).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn retrieve_similar_successes_ranks_and_filters() {
        let (db, _c, _u, asst_id) = open_db_with_conv_and_msgs();
        let id = db
            .insert_learned_success("sort rust vec", "use .sort()", &asst_id)
            .unwrap();
        db.update_learned_success_embedding(&id, &[1.0, 0.0, 0.0])
            .unwrap();

        // Very similar query.
        let hits = retrieve_similar_successes(&db, &[0.99, 0.01, 0.0], 3).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1 >= LEARNED_SIMILARITY_THRESHOLD);

        // Orthogonal query — below threshold, filtered out.
        let miss = retrieve_similar_successes(&db, &[0.0, 1.0, 0.0], 3).unwrap();
        assert!(miss.is_empty());
    }

    #[test]
    fn distill_text_caps_length() {
        let long = "x".repeat(1000);
        let out = distill_text(&long, 50);
        assert!(out.chars().count() <= 51); // 50 chars + ellipsis
    }

    #[test]
    fn build_section_empty_when_no_hits() {
        assert_eq!(build_learned_successes_section(&[]), "");
    }

    // ── LLM distillation mocks & tests ─────────────────────────────

    /// Mock provider that either returns a fixed response string or an error
    /// for every `complete()` call. `stream()` is unused by the distiller.
    struct MockDistillProvider {
        response: Option<String>,
        error_factory: Option<Arc<dyn Fn() -> CoreError + Send + Sync>>,
        call_count: Arc<AtomicUsize>,
    }

    impl MockDistillProvider {
        fn ok(text: impl Into<String>) -> Self {
            Self {
                response: Some(text.into()),
                error_factory: None,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn err<F>(factory: F) -> Self
        where
            F: Fn() -> CoreError + Send + Sync + 'static,
        {
            Self {
                response: None,
                error_factory: Some(Arc::new(factory)),
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockDistillProvider {
        fn name(&self) -> &str {
            "mock-distill"
        }

        async fn list_models(&self) -> Result<Vec<String>, CoreError> {
            Ok(vec!["mock-model".to_string()])
        }

        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> Result<CompletionResponse, CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            if let Some(f) = &self.error_factory {
                return Err(f());
            }
            Ok(CompletionResponse {
                content: self.response.clone().unwrap_or_default(),
                tool_calls: None,
                finish_reason: FinishReason::Stop,
                usage: Usage::default(),
                thinking: None,
            })
        }

        async fn stream(
            &self,
            _request: &CompletionRequest,
        ) -> Result<BoxStream<'_, Result<StreamChunk, CoreError>>, CoreError> {
            Ok(Box::pin(stream::iter(Vec::new())))
        }

        async fn health_check(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn distill_text_llm_uses_model_output_when_successful() {
        let long_input = "x".repeat(1000);
        let provider = MockDistillProvider::ok("  concise summary with keywords  ");
        let out = distill_text_llm(&provider, "mock-model", &long_input, 500).await;
        assert_eq!(out, "concise summary with keywords");
    }

    #[tokio::test]
    async fn distill_text_llm_caps_overlong_model_output() {
        let long_input = "x".repeat(1000);
        let overlong = "y".repeat(800);
        let provider = MockDistillProvider::ok(overlong);
        let out = distill_text_llm(&provider, "mock-model", &long_input, 50).await;
        // 50 chars + single ellipsis char.
        assert!(out.chars().count() <= 51);
        assert!(out.ends_with('…'));
    }

    #[tokio::test]
    async fn distill_text_llm_falls_back_on_error() {
        let long_input = "a".repeat(1000);
        let provider = MockDistillProvider::err(|| CoreError::Llm("boom".to_string()));
        let out = distill_text_llm(&provider, "mock-model", &long_input, 50).await;
        // Fallback path: distill_text(a*1000, 50) == 50 'a's + '…'
        let expected = distill_text(&long_input, 50);
        assert_eq!(out, expected);
    }

    #[tokio::test]
    async fn distill_text_llm_falls_back_on_empty_response() {
        let long_input = "b".repeat(1000);
        let provider = MockDistillProvider::ok("   ");
        let out = distill_text_llm(&provider, "mock-model", &long_input, 50).await;
        assert_eq!(out, distill_text(&long_input, 50));
    }

    #[tokio::test]
    async fn distill_text_llm_skips_call_for_short_input() {
        let provider = MockDistillProvider::ok("SHOULD NOT BE USED");
        let calls = provider.call_count.clone();
        let out = distill_text_llm(&provider, "mock-model", "already short", 500).await;
        assert_eq!(out, "already short");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn insert_learned_success_with_llm_stores_llm_summary() {
        let (db, _c, _u, asst_id) = open_db_with_conv_and_msgs();
        let long_response = "z".repeat(1000);
        let provider = MockDistillProvider::ok("LLM summary of assistant answer");
        let id = insert_learned_success_with_llm(
            &db,
            "how do I sort a Rust vector?",
            &long_response,
            &asst_id,
            Some(&provider),
            Some("mock-model"),
        )
        .await
        .unwrap();

        let rows = db.list_learned_successes_with_embedding().unwrap();
        // No embedding attached yet — check via raw query instead.
        assert!(rows.is_empty());

        let conn = db.conn();
        let (q, s): (String, String) = conn
            .query_row(
                "SELECT user_query, response_summary FROM learned_successes WHERE id = ?1",
                params![&id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(q, "how do I sort a Rust vector?");
        assert_eq!(s, "LLM summary of assistant answer");
    }

    #[tokio::test]
    async fn insert_learned_success_with_llm_falls_back_on_provider_error() {
        let (db, _c, _u, asst_id) = open_db_with_conv_and_msgs();
        let long_response = "q".repeat(1000);
        let provider = MockDistillProvider::err(|| CoreError::Llm("provider unavailable".into()));
        let id = insert_learned_success_with_llm(
            &db,
            "user query text",
            &long_response,
            &asst_id,
            Some(&provider),
            Some("mock-model"),
        )
        .await
        .unwrap();

        let conn = db.conn();
        let stored: String = conn
            .query_row(
                "SELECT response_summary FROM learned_successes WHERE id = ?1",
                params![&id],
                |row| row.get(0),
            )
            .unwrap();
        // Falls back to distill_text on the raw response.
        assert_eq!(stored, distill_text(&long_response, LEARNED_TEXT_MAX_CHARS));
    }

    #[tokio::test]
    async fn insert_learned_success_with_llm_uses_cheap_path_without_provider() {
        let (db, _c, _u, asst_id) = open_db_with_conv_and_msgs();
        let long_response = "m".repeat(1000);
        let id = insert_learned_success_with_llm(&db, "q", &long_response, &asst_id, None, None)
            .await
            .unwrap();

        let conn = db.conn();
        let stored: String = conn
            .query_row(
                "SELECT response_summary FROM learned_successes WHERE id = ?1",
                params![&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, distill_text(&long_response, LEARNED_TEXT_MAX_CHARS));
    }
}
