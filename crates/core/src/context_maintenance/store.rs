use std::collections::HashSet;

use rusqlite::OptionalExtension;
use tokio_util::sync::CancellationToken;

use crate::conversation::memory::estimate_tokens_for_model;
use crate::conversation::{ConversationMessage, ImageAttachment};
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{Role, ToolCallRequest};
use crate::usage_analytics::{
    provider_type_id, record_ai_usage_on_connection, usage_cost_metadata, AiUsageRecordInput,
};

use super::model::{ContextCheckpointInput, ContextProjection};
use super::planner::hash_field;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    Committed,
    Superseded,
}

pub(crate) fn commit_context_checkpoint(
    database: &Database,
    input: &ContextCheckpointInput,
    expected_message_ids: &[String],
    cancellation: &CancellationToken,
) -> Result<CommitOutcome, CoreError> {
    let mut conn = database.conn();
    let tx = conn.transaction()?;
    let active_run = tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM agent_task_runs
             WHERE conversation_id = ?1
               AND status IN ('queued', 'running', 'waiting_approval', 'cancelling')
         )",
        rusqlite::params![input.conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    let (current_message_ids, current_snapshot_hash) =
        current_snapshot(&tx, &input.conversation_id)?;
    if active_run
        || current_message_ids != expected_message_ids
        || current_snapshot_hash != input.snapshot_hash
    {
        return Ok(CommitOutcome::Superseded);
    }
    if cancellation.is_cancelled() {
        return Err(CoreError::Cancelled(
            "Context compaction was cancelled before commit".to_string(),
        ));
    }

    let retained_tail_json = serde_json::to_string(&input.retained_tail_message_ids)?;
    let usage_json = input
        .usage
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    tx.execute(
        "INSERT INTO context_compactions (
             id, operation_id, conversation_id, idempotency_key,
             snapshot_high_watermark, snapshot_hash, summary,
             retained_tail_json, retained_start_sort_order,
             tokens_before, tokens_after, provider, model, usage_json, status
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'completed')",
        rusqlite::params![
            input.operation_id,
            input.operation_id,
            input.conversation_id,
            input.idempotency_key,
            input.snapshot_high_watermark,
            input.snapshot_hash,
            input.summary,
            retained_tail_json,
            input.retained_start_sort_order,
            input.tokens_before,
            input.tokens_after,
            input.provider,
            input.model,
            usage_json,
        ],
    )?;
    if let Some(usage) = input.usage.as_ref() {
        let invocation_id = format!("{}:summarization:{}", input.operation_id, input.model);
        let provider_id = provider_type_id(input.provider_type);
        let provider_raw = serde_json::to_value(usage)?;
        let (estimated_cost_micros, currency, pricing_version) =
            usage_cost_metadata(input.provider_type);
        record_ai_usage_on_connection(
            &tx,
            &AiUsageRecordInput {
                invocation_id: &invocation_id,
                occurred_at: None,
                provider_id,
                provider_type: provider_id,
                model_id: &input.model,
                raw_model_id: Some(&input.model),
                modality: "language_model",
                operation_kind: "compaction",
                conversation_id: Some(&input.conversation_id),
                turn_id: None,
                run_id: None,
                subtask_run_id: None,
                project_id: None,
                prompt_tokens: u64::from(usage.prompt_tokens),
                completion_tokens: u64::from(usage.completion_tokens),
                thinking_tokens: u64::from(usage.thinking_tokens.unwrap_or(0)),
                total_tokens: u64::from(
                    usage
                        .total_tokens
                        .max(usage.prompt_tokens.saturating_add(usage.completion_tokens)),
                ),
                cache_read_tokens: u64::from(usage.cache_read_tokens.unwrap_or(0)),
                cache_miss_tokens: u64::from(usage.cache_miss_tokens.unwrap_or(0)),
                cache_creation_tokens: u64::from(usage.cache_creation_tokens.unwrap_or(0)),
                usage_source: "provider",
                request_status: "success",
                latency_ms: None,
                time_to_first_token_ms: None,
                upstream_provider_id: None,
                cache_outcome_reason: None,
                estimated_cost_micros,
                currency,
                pricing_version,
                provider_raw: &provider_raw,
            },
        )?;
    }
    tx.execute(
        "UPDATE conversations
         SET active_context_compaction_id = ?2, updated_at = datetime('now')
         WHERE id = ?1",
        rusqlite::params![input.conversation_id, input.operation_id],
    )?;
    tx.commit()?;
    Ok(CommitOutcome::Committed)
}

fn current_snapshot(
    tx: &rusqlite::Transaction<'_>,
    conversation_id: &str,
) -> Result<(Vec<String>, String), CoreError> {
    let mut statement = tx.prepare(
        "SELECT id, sort_order, role, content, tool_call_id, tool_calls_json,
                artifacts_json, thinking, image_attachments_json
         FROM messages
         WHERE conversation_id = ?1
         ORDER BY sort_order ASC",
    )?;
    let rows = statement.query_map(rusqlite::params![conversation_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut message_ids = Vec::new();
    let mut hash = blake3::Hasher::new();
    for row in rows {
        let (
            id,
            sort_order,
            role,
            content,
            tool_call_id,
            tool_calls_json,
            artifacts_json,
            thinking,
            image_attachments_json,
        ) = row?;
        hash_field(&mut hash, id.as_bytes());
        hash_field(&mut hash, &sort_order.to_le_bytes());
        hash_field(&mut hash, role.as_bytes());
        hash_field(&mut hash, content.as_bytes());
        for optional in [
            tool_call_id,
            tool_calls_json,
            thinking,
            artifacts_json,
            image_attachments_json,
        ] {
            hash_field(
                &mut hash,
                optional.as_deref().unwrap_or_default().as_bytes(),
            );
        }
        message_ids.push(id);
    }
    Ok((message_ids, hash.finalize().to_hex().to_string()))
}

#[derive(Debug)]
struct ActiveCheckpoint {
    id: String,
    summary: String,
    snapshot_high_watermark: i64,
    retained_start_sort_order: i64,
    retained_tail_message_ids: Vec<String>,
}

pub fn load_context_projection(
    database: &Database,
    conversation_id: &str,
) -> Result<ContextProjection, CoreError> {
    let checkpoint = active_checkpoint(database, conversation_id)?;
    let Some(checkpoint) = checkpoint else {
        return Ok(ContextProjection {
            messages: database.get_messages(conversation_id)?,
            checkpoint_id: None,
            projected: false,
        });
    };

    let tail = get_messages_from_sort_order(
        database,
        conversation_id,
        checkpoint.retained_start_sort_order,
    )?;
    let expected_tail = checkpoint
        .retained_tail_message_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let present_tail = tail
        .iter()
        .filter(|message| message.sort_order <= checkpoint.snapshot_high_watermark)
        .map(|message| message.id.as_str())
        .collect::<HashSet<_>>();
    if !expected_tail.is_subset(&present_tail) {
        tracing::warn!(
            conversation_id,
            checkpoint_id = %checkpoint.id,
            "Active context checkpoint tail changed; falling back to canonical transcript"
        );
        return Ok(ContextProjection {
            messages: database.get_messages(conversation_id)?,
            checkpoint_id: None,
            projected: false,
        });
    }

    let summary = ConversationMessage {
        id: format!("context-compaction:{}", checkpoint.id),
        conversation_id: conversation_id.to_string(),
        role: Role::System,
        token_count: estimate_tokens_for_model("gpt-4o", &checkpoint.summary),
        content: checkpoint.summary,
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: Some(serde_json::json!({
            "kind": "contextCompaction",
            "checkpointId": checkpoint.id.clone(),
        })),
        created_at: String::new(),
        sort_order: checkpoint.retained_start_sort_order.saturating_sub(1),
        thinking: None,
        image_attachments: None,
    };
    let mut messages = Vec::with_capacity(tail.len() + 1);
    messages.push(summary);
    messages.extend(tail);
    Ok(ContextProjection {
        messages,
        checkpoint_id: Some(checkpoint.id),
        projected: true,
    })
}

fn active_checkpoint(
    database: &Database,
    conversation_id: &str,
) -> Result<Option<ActiveCheckpoint>, CoreError> {
    let conn = database.conn();
    conn.query_row(
        "SELECT cc.id, cc.summary, cc.snapshot_high_watermark,
                cc.retained_start_sort_order, cc.retained_tail_json
         FROM conversations c
         JOIN context_compactions cc ON cc.id = c.active_context_compaction_id
         WHERE c.id = ?1 AND cc.status = 'completed'",
        rusqlite::params![conversation_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    )
    .optional()?
    .map(
        |(id, summary, snapshot_high_watermark, retained_start_sort_order, retained_json)| {
            Ok(ActiveCheckpoint {
                id,
                summary,
                snapshot_high_watermark,
                retained_start_sort_order,
                retained_tail_message_ids: serde_json::from_str(&retained_json)?,
            })
        },
    )
    .transpose()
}

fn get_messages_from_sort_order(
    database: &Database,
    conversation_id: &str,
    start_sort_order: i64,
) -> Result<Vec<ConversationMessage>, CoreError> {
    let conn = database.conn();
    let mut statement = conn.prepare(
        "SELECT id, conversation_id, role, content, tool_call_id, tool_calls_json,
                artifacts_json, token_count, created_at, sort_order, thinking,
                image_attachments_json
         FROM messages
         WHERE conversation_id = ?1 AND sort_order >= ?2
         ORDER BY sort_order ASC",
    )?;
    let rows = statement.query_map(
        rusqlite::params![conversation_id, start_sort_order],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        },
    )?;
    let mut messages = Vec::new();
    for row in rows {
        let (
            id,
            conversation_id,
            role,
            content,
            tool_call_id,
            tool_calls_json,
            artifacts_json,
            token_count,
            created_at,
            sort_order,
            thinking,
            attachments_json,
        ) = row?;
        messages.push(ConversationMessage {
            id,
            conversation_id,
            role: role_from_storage(&role),
            content,
            tool_call_id,
            tool_calls: tool_calls_json
                .map(|json| serde_json::from_str::<Vec<ToolCallRequest>>(&json))
                .transpose()?
                .unwrap_or_default(),
            artifacts: artifacts_json
                .map(|json| serde_json::from_str(&json))
                .transpose()?,
            token_count,
            created_at,
            sort_order,
            thinking,
            image_attachments: attachments_json
                .and_then(|json| serde_json::from_str::<Vec<ImageAttachment>>(&json).ok()),
        });
    }
    Ok(messages)
}

fn role_from_storage(role: &str) -> Role {
    match role {
        "system" => Role::System,
        "assistant" => Role::Assistant,
        "tool" => Role::Tool,
        _ => Role::User,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::CreateConversationInput;

    fn add_message(database: &Database, conversation_id: &str, sort_order: i64, role: Role) {
        database
            .add_message(&ConversationMessage {
                id: format!("message-{sort_order}"),
                conversation_id: conversation_id.to_string(),
                role,
                content: format!("content {sort_order}"),
                tool_call_id: None,
                tool_calls: Vec::new(),
                artifacts: None,
                token_count: 10,
                created_at: String::new(),
                sort_order,
                thinking: None,
                image_attachments: None,
            })
            .expect("add message");
    }

    #[test]
    fn checkpoint_commit_preserves_canonical_transcript_and_projects_tail() {
        let database = Database::open_memory().expect("open database");
        let conversation = database
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        for (sort_order, role) in [
            (0, Role::User),
            (1, Role::Assistant),
            (2, Role::User),
            (3, Role::Assistant),
        ] {
            add_message(&database, &conversation.id, sort_order, role);
        }
        let canonical_before = database
            .get_messages(&conversation.id)
            .expect("load canonical transcript");
        let expected_ids = canonical_before
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        let outcome = commit_context_checkpoint(
            &database,
            &ContextCheckpointInput {
                operation_id: "ctx-test".to_string(),
                conversation_id: conversation.id.clone(),
                idempotency_key: "request-test".to_string(),
                snapshot_high_watermark: 3,
                snapshot_hash: super::super::planner::snapshot_hash(&canonical_before),
                summary: "older context summary".to_string(),
                retained_tail_message_ids: vec!["message-2".to_string(), "message-3".to_string()],
                retained_start_sort_order: 2,
                tokens_before: 40,
                tokens_after: 24,
                provider: "test".to_string(),
                provider_type: Some(crate::llm::ProviderType::OpenAi),
                model: "test".to_string(),
                usage: Some(crate::llm::Usage {
                    prompt_tokens: 120,
                    completion_tokens: 30,
                    total_tokens: 150,
                    ..Default::default()
                }),
            },
            &expected_ids,
            &CancellationToken::new(),
        )
        .expect("commit checkpoint");
        assert_eq!(outcome, CommitOutcome::Committed);
        let recorded_usage = database
            .conn()
            .query_row(
                "SELECT operation_kind, prompt_tokens, completion_tokens
                 FROM ai_usage_records WHERE conversation_id = ?1",
                [&conversation.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .expect("load compaction usage");
        assert_eq!(recorded_usage, ("compaction".to_string(), 120, 30));

        let canonical_after = database
            .get_messages(&conversation.id)
            .expect("reload canonical transcript");
        assert_eq!(
            canonical_after
                .iter()
                .map(|message| (&message.id, &message.content, message.sort_order))
                .collect::<Vec<_>>(),
            canonical_before
                .iter()
                .map(|message| (&message.id, &message.content, message.sort_order))
                .collect::<Vec<_>>()
        );

        add_message(&database, &conversation.id, 4, Role::User);
        let projection =
            load_context_projection(&database, &conversation.id).expect("load context projection");
        assert!(projection.projected);
        assert_eq!(projection.checkpoint_id.as_deref(), Some("ctx-test"));
        assert_eq!(projection.messages.len(), 4);
        assert_eq!(projection.messages[0].role, Role::System);
        assert_eq!(projection.messages[1].id, "message-2");
        assert_eq!(projection.messages[3].id, "message-4");

        database
            .update_message_llm_context_content("message-0", "edited replay context")
            .expect("edit older canonical message");
        let invalidated = load_context_projection(&database, &conversation.id)
            .expect("load invalidated projection");
        assert!(!invalidated.projected);
        assert_eq!(invalidated.messages.len(), 5);
    }

    #[test]
    fn cancellation_fence_prevents_checkpoint_commit() {
        let database = Database::open_memory().expect("open database");
        let conversation = database
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        add_message(&database, &conversation.id, 0, Role::User);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let canonical = database
            .get_messages(&conversation.id)
            .expect("load canonical transcript");
        let error = commit_context_checkpoint(
            &database,
            &ContextCheckpointInput {
                operation_id: "ctx-cancelled".to_string(),
                conversation_id: conversation.id.clone(),
                idempotency_key: "request-cancelled".to_string(),
                snapshot_high_watermark: 0,
                snapshot_hash: super::super::planner::snapshot_hash(&canonical),
                summary: "summary".to_string(),
                retained_tail_message_ids: vec!["message-0".to_string()],
                retained_start_sort_order: 0,
                tokens_before: 10,
                tokens_after: 10,
                provider: "test".to_string(),
                provider_type: None,
                model: "test".to_string(),
                usage: None,
            },
            &["message-0".to_string()],
            &cancellation,
        )
        .expect_err("cancelled commit must fail");
        assert!(matches!(error, CoreError::Cancelled(_)));
        let checkpoint_count = database
            .conn()
            .query_row("SELECT COUNT(*) FROM context_compactions", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("count checkpoints");
        assert_eq!(checkpoint_count, 0);
    }

    #[test]
    fn snapshot_hash_supersedes_same_id_content_edits() {
        let database = Database::open_memory().expect("open database");
        let conversation = database
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-4o".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .expect("create conversation");
        add_message(&database, &conversation.id, 0, Role::User);
        let snapshot = database
            .get_messages(&conversation.id)
            .expect("load snapshot");
        database
            .conn()
            .execute(
                "UPDATE messages SET content = 'changed' WHERE id = 'message-0'",
                [],
            )
            .expect("edit message in place");

        let outcome = commit_context_checkpoint(
            &database,
            &ContextCheckpointInput {
                operation_id: "ctx-superseded".to_string(),
                conversation_id: conversation.id,
                idempotency_key: "request-superseded".to_string(),
                snapshot_high_watermark: 0,
                snapshot_hash: super::super::planner::snapshot_hash(&snapshot),
                summary: "summary".to_string(),
                retained_tail_message_ids: vec!["message-0".to_string()],
                retained_start_sort_order: 0,
                tokens_before: 10,
                tokens_after: 10,
                provider: "test".to_string(),
                provider_type: None,
                model: "test".to_string(),
                usage: None,
            },
            &["message-0".to_string()],
            &CancellationToken::new(),
        )
        .expect("compare snapshot");
        assert_eq!(outcome, CommitOutcome::Superseded);
    }
}
