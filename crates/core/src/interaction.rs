//! Durable user-interaction protocol and persistence boundary.
//!
//! Agent tools create stable requests through this module. Presentation code
//! may be replaced without changing idempotency, ordering, validation, or
//! response lifecycle semantics.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{OptionalExtension, Row, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::conversation::{AgentTurnLaunchRecord, ConversationMessage};
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::Role;

pub const INTERACTION_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    UserInput,
    Approval,
    HighRiskConfirmation,
    CredentialRequest,
    ConflictResolution,
}

impl InteractionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserInput => "user_input",
            Self::Approval => "approval",
            Self::HighRiskConfirmation => "high_risk_confirmation",
            Self::CredentialRequest => "credential_request",
            Self::ConflictResolution => "conflict_resolution",
        }
    }

    fn from_db(value: &str) -> Result<Self, CoreError> {
        match value {
            "user_input" => Ok(Self::UserInput),
            "approval" => Ok(Self::Approval),
            "high_risk_confirmation" => Ok(Self::HighRiskConfirmation),
            "credential_request" => Ok(Self::CredentialRequest),
            "conflict_resolution" => Ok(Self::ConflictResolution),
            other => Err(CoreError::Internal(format!(
                "Unknown persisted interaction kind: {other}"
            ))),
        }
    }

    pub fn risk_priority(self) -> i64 {
        match self {
            Self::HighRiskConfirmation => 400,
            Self::Approval => 350,
            Self::CredentialRequest => 300,
            Self::ConflictResolution => 200,
            Self::UserInput => 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    Pending,
    Presented,
    PartiallyAnswered,
    Submitted,
    Acknowledged,
    Cancelled,
    Expired,
    Superseded,
    Failed,
}

impl InteractionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Presented => "presented",
            Self::PartiallyAnswered => "partially_answered",
            Self::Submitted => "submitted",
            Self::Acknowledged => "acknowledged",
            Self::Cancelled => "cancelled",
            Self::Expired => "expired",
            Self::Superseded => "superseded",
            Self::Failed => "failed",
        }
    }

    fn from_db(value: &str) -> Result<Self, CoreError> {
        match value {
            "pending" => Ok(Self::Pending),
            "presented" => Ok(Self::Presented),
            "partially_answered" => Ok(Self::PartiallyAnswered),
            "submitted" => Ok(Self::Submitted),
            "acknowledged" => Ok(Self::Acknowledged),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            "superseded" => Ok(Self::Superseded),
            "failed" => Ok(Self::Failed),
            other => Err(CoreError::Internal(format!(
                "Unknown persisted interaction status: {other}"
            ))),
        }
    }

    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Presented | Self::PartiallyAnswered
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Acknowledged | Self::Cancelled | Self::Expired | Self::Superseded | Self::Failed
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionQuestionKind {
    Short,
    Long,
    SingleChoice,
    MultiChoice,
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionQuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(rename = "type")]
    pub kind: InteractionQuestionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<InteractionQuestionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

pub type InteractionAnswers = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionRequest {
    pub schema_version: u16,
    pub interaction_id: String,
    pub conversation_id: String,
    pub turn_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub kind: InteractionKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub questions: Vec<InteractionQuestion>,
    pub required: bool,
    pub status: InteractionStatus,
    pub risk_priority: i64,
    pub queue_sequence: i64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub resume_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionDraft {
    pub schema_version: u16,
    pub interaction_id: String,
    pub conversation_id: String,
    pub answers: InteractionAnswers,
    pub current_question_index: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InteractionResponse {
    pub schema_version: u16,
    pub interaction_id: String,
    pub answers: InteractionAnswers,
    pub submitted_at: String,
}

#[derive(Debug, Clone)]
pub struct CreateInteractionRequest {
    pub conversation_id: String,
    pub turn_id: String,
    pub tool_call_id: Option<String>,
    pub idempotency_key: String,
    pub kind: InteractionKind,
    pub title: String,
    pub description: Option<String>,
    pub questions: Vec<InteractionQuestion>,
    pub required: bool,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedInteractionRequest {
    pub request: InteractionRequest,
    pub reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitInteractionResponse {
    pub interaction_id: String,
    pub resume_token: String,
    pub answers: InteractionAnswers,
}

#[derive(Debug)]
pub(crate) struct ConsumedInteractionResponse {
    pub(crate) response: InteractionResponse,
    run_id: String,
    title: String,
}

const REQUEST_COLUMNS: &str =
    "id, conversation_id, turn_id, tool_call_id, kind, title, description, questions_json, required, status, risk_priority, queue_sequence, created_at, updated_at, expires_at, resume_token";

fn normalize_required_text(
    value: &str,
    field: &str,
    max_chars: usize,
) -> Result<String, CoreError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(CoreError::InvalidInput(format!(
            "Interaction {field} must not be empty"
        )));
    }
    if value.chars().count() > max_chars {
        return Err(CoreError::InvalidInput(format!(
            "Interaction {field} exceeds {max_chars} characters"
        )));
    }
    Ok(value.to_string())
}

fn normalize_optional_text(
    value: Option<&str>,
    field: &str,
    max_chars: usize,
) -> Result<Option<String>, CoreError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_required_text(value, field, max_chars))
        .transpose()
}

fn normalize_expiry(value: Option<&str>) -> Result<Option<String>, CoreError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| {
        CoreError::InvalidInput("Interaction expiry must be an RFC 3339 timestamp".to_string())
    })?;
    Ok(Some(
        parsed
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    ))
}

pub fn normalize_questions(
    questions: &[InteractionQuestion],
) -> Result<Vec<InteractionQuestion>, CoreError> {
    if !(1..=6).contains(&questions.len()) {
        return Err(CoreError::InvalidInput(
            "An interaction requires one to six questions".to_string(),
        ));
    }
    let mut ids = HashSet::new();
    questions
        .iter()
        .map(|question| {
            let id = normalize_required_text(&question.id, "question id", 64)?;
            let normalized_id = id.to_lowercase();
            if !ids.insert(normalized_id) {
                return Err(CoreError::InvalidInput(format!(
                    "Duplicate interaction question id: {id}"
                )));
            }
            let header = normalize_required_text(&question.header, "question header", 64)?;
            let prompt = normalize_required_text(&question.question, "question text", 2_000)?;
            let options = question
                .options
                .iter()
                .map(|option| {
                    Ok(InteractionQuestionOption {
                        label: normalize_required_text(&option.label, "option label", 160)?,
                        description: normalize_optional_text(
                            option.description.as_deref(),
                            "option description",
                            1_000,
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            let is_choice = matches!(
                question.kind,
                InteractionQuestionKind::SingleChoice | InteractionQuestionKind::MultiChoice
            );
            let unique_options = options
                .iter()
                .map(|option| option.label.to_lowercase())
                .collect::<HashSet<_>>();
            if unique_options.len() != options.len() {
                return Err(CoreError::InvalidInput(format!(
                    "Question `{id}` contains duplicate option labels"
                )));
            }
            if is_choice && !(2..=4).contains(&options.len()) {
                return Err(CoreError::InvalidInput(format!(
                    "Choice question `{id}` requires two to four options"
                )));
            }
            if !is_choice && !options.is_empty() {
                return Err(CoreError::InvalidInput(format!(
                    "Non-choice question `{id}` cannot define options"
                )));
            }
            Ok(InteractionQuestion {
                id,
                header,
                question: prompt,
                kind: question.kind,
                options,
                placeholder: normalize_optional_text(
                    question.placeholder.as_deref(),
                    "question placeholder",
                    500,
                )?,
                why: normalize_optional_text(
                    question.why.as_deref(),
                    "question explanation",
                    2_000,
                )?,
            })
        })
        .collect()
}

fn validate_answers(
    request: &InteractionRequest,
    answers: &InteractionAnswers,
) -> Result<InteractionAnswers, CoreError> {
    let known_ids = request
        .questions
        .iter()
        .map(|question| question.id.as_str())
        .collect::<HashSet<_>>();
    if let Some(unknown) = answers.keys().find(|id| !known_ids.contains(id.as_str())) {
        return Err(CoreError::InvalidInput(format!(
            "Unknown interaction question id: {unknown}"
        )));
    }

    let mut normalized = InteractionAnswers::new();
    for question in &request.questions {
        let values = answers
            .get(&question.id)
            .into_iter()
            .flatten()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if values.len() > 32 || values.iter().any(|value| value.chars().count() > 20_000) {
            return Err(CoreError::InvalidInput(format!(
                "Question `{}` exceeds the answer size limit",
                question.id
            )));
        }
        if request.required && values.is_empty() {
            return Err(CoreError::InvalidInput(format!(
                "Question `{}` requires an answer",
                question.id
            )));
        }
        let allows_multiple = question.kind == InteractionQuestionKind::MultiChoice;
        if !allows_multiple && values.len() > 1 {
            return Err(CoreError::InvalidInput(format!(
                "Question `{}` accepts only one answer",
                question.id
            )));
        }
        let unique_count = values.iter().collect::<HashSet<_>>().len();
        if unique_count != values.len() {
            return Err(CoreError::InvalidInput(format!(
                "Question `{}` contains duplicate answers",
                question.id
            )));
        }
        if !values.is_empty() {
            normalized.insert(question.id.clone(), values);
        }
    }
    Ok(normalized)
}

fn decode_json<T: serde::de::DeserializeOwned>(raw: String, column: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn request_from_row(row: &Row<'_>) -> rusqlite::Result<InteractionRequest> {
    let kind: String = row.get(4)?;
    let status: String = row.get(9)?;
    Ok(InteractionRequest {
        schema_version: INTERACTION_PROTOCOL_VERSION,
        interaction_id: row.get(0)?,
        conversation_id: row.get(1)?,
        turn_id: row.get(2)?,
        tool_call_id: row.get(3)?,
        kind: InteractionKind::from_db(&kind).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        title: row.get(5)?,
        description: row.get(6)?,
        questions: decode_json(row.get(7)?, 7)?,
        required: row.get(8)?,
        status: InteractionStatus::from_db(&status).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        risk_priority: row.get(10)?,
        queue_sequence: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        expires_at: row.get(14)?,
        resume_token: row.get(15)?,
    })
}

pub(crate) fn expire_due_requests(conn: &mut rusqlite::Connection) -> Result<(), CoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO interaction_events (interaction_id, from_status, to_status, reason)
         SELECT id, status, 'expired', 'deadline_elapsed'
         FROM interaction_requests
         WHERE status IN ('pending', 'presented', 'partially_answered')
           AND expires_at IS NOT NULL
           AND datetime(expires_at) <= datetime('now')",
        [],
    )?;
    tx.execute(
        "UPDATE interaction_requests
         SET status = 'expired', updated_at = datetime('now')
         WHERE status IN ('pending', 'presented', 'partially_answered')
           AND expires_at IS NOT NULL
           AND datetime(expires_at) <= datetime('now')",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn consume_interaction_response_in_transaction(
    tx: &Transaction<'_>,
    input: &SubmitInteractionResponse,
    expected_conversation_id: Option<&str>,
) -> Result<ConsumedInteractionResponse, CoreError> {
    let request = tx
        .query_row(
            &format!("SELECT {REQUEST_COLUMNS} FROM interaction_requests WHERE id = ?1"),
            rusqlite::params![&input.interaction_id],
            request_from_row,
        )
        .optional()?
        .ok_or_else(|| {
            CoreError::NotFound(format!("Interaction request {}", input.interaction_id))
        })?;
    if expected_conversation_id.is_some_and(|id| request.conversation_id != id) {
        return Err(CoreError::InvalidInput(
            "Interaction response belongs to a different conversation".to_string(),
        ));
    }
    if !bool::from(
        request
            .resume_token
            .as_bytes()
            .ct_eq(input.resume_token.as_bytes()),
    ) {
        return Err(CoreError::InvalidInput(
            "Interaction resume token is invalid or stale".to_string(),
        ));
    }
    if !matches!(
        request.status,
        InteractionStatus::Presented | InteractionStatus::PartiallyAnswered
    ) {
        return Err(CoreError::InvalidInput(format!(
            "Interaction {} cannot be submitted from status {}",
            input.interaction_id,
            request.status.as_str()
        )));
    }
    let answers = validate_answers(&request, &input.answers)?;
    let answers_json = serde_json::to_string(&answers)?;
    let affected = tx.execute(
        "UPDATE interaction_requests
         SET status = 'submitted', updated_at = datetime('now')
         WHERE id = ?1
           AND status IN ('presented', 'partially_answered')
           AND (expires_at IS NULL OR datetime(expires_at) > datetime('now'))",
        rusqlite::params![&input.interaction_id],
    )?;
    if affected != 1 {
        return Err(CoreError::InvalidInput(format!(
            "Interaction {} changed or expired while it was being submitted",
            input.interaction_id
        )));
    }
    let response_id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO interaction_responses (id, interaction_id, answers_json)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![&response_id, &input.interaction_id, &answers_json],
    )?;
    tx.execute(
        "INSERT INTO interaction_events (interaction_id, from_status, to_status)
         VALUES (?1, ?2, 'submitted')",
        rusqlite::params![&input.interaction_id, request.status.as_str()],
    )?;
    let submitted_at: String = tx.query_row(
        "SELECT submitted_at FROM interaction_responses WHERE id = ?1",
        rusqlite::params![&response_id],
        |row| row.get(0),
    )?;
    Ok(ConsumedInteractionResponse {
        response: InteractionResponse {
            schema_version: INTERACTION_PROTOCOL_VERSION,
            interaction_id: input.interaction_id.clone(),
            answers,
            submitted_at,
        },
        run_id: tx.query_row(
            "SELECT run_id FROM interaction_requests WHERE id = ?1",
            rusqlite::params![&input.interaction_id],
            |row| row.get(0),
        )?,
        title: request.title,
    })
}

impl Database {
    /// Move the owning turn/run into a durable waiting state before the
    /// interaction artifact is exposed to the frontend. Doing this inside the
    /// tool execution closes the race where a fast answer could arrive while
    /// the executor still looked runnable.
    pub fn suspend_agent_turn_for_interaction(
        &self,
        interaction_id: &str,
    ) -> Result<bool, CoreError> {
        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (turn_id, run_id, interaction_status, turn_status, run_status) = tx
            .query_row(
                "SELECT i.turn_id, i.run_id, i.status, t.status, r.status
                 FROM interaction_requests i
                 JOIN conversation_turns t ON t.id = i.turn_id
                 JOIN agent_task_runs r ON r.id = i.run_id
                 WHERE i.id = ?1",
                rusqlite::params![interaction_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("Interaction request {interaction_id}")))?;
        let status = InteractionStatus::from_db(&interaction_status)?;
        if !status.is_active() {
            tx.commit()?;
            return Ok(false);
        }
        if turn_status == "awaiting_user_input" && run_status == "awaiting_user_input" {
            tx.commit()?;
            return Ok(true);
        }
        if !matches!(run_status.as_str(), "queued" | "running") {
            return Err(CoreError::InvalidInput(format!(
                "Interaction {interaction_id} cannot suspend task run {run_id} from status {run_status}"
            )));
        }
        tx.execute(
            "UPDATE conversation_turns
             SET status = 'awaiting_user_input', finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&turn_id],
        )?;
        tx.execute(
            "UPDATE agent_task_runs
             SET status = 'awaiting_user_input',
                 phase = 'awaiting_user_input',
                 summary = 'Waiting for user input',
                 error_message = NULL,
                 finished_at = NULL,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&run_id],
        )?;
        tx.commit()?;
        drop(conn);
        let _ = self.record_agent_task_run_event(
            &run_id,
            "interaction_waiting",
            "Waiting for user input",
            Some("awaiting_user_input"),
            Some(&serde_json::json!({ "interactionId": interaction_id })),
        );
        Ok(true)
    }

    /// Append ordinary user context to a suspended turn without consuming its
    /// one-shot response or allocating a second conversation turn.
    pub fn append_interaction_supplement(
        &self,
        interaction_id: &str,
        content: &str,
    ) -> Result<ConversationMessage, CoreError> {
        let content = normalize_required_text(content, "supplement", 40_000)?;
        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (conversation_id, run_id, request_status, run_status) = tx
            .query_row(
                "SELECT i.conversation_id, i.run_id, i.status, r.status
                 FROM interaction_requests i
                 JOIN agent_task_runs r ON r.id = i.run_id
                 WHERE i.id = ?1",
                rusqlite::params![interaction_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("Interaction request {interaction_id}")))?;
        if !InteractionStatus::from_db(&request_status)?.is_active()
            || run_status != "awaiting_user_input"
        {
            return Err(CoreError::InvalidInput(
                "Supplementary text can only be added while the task is waiting for this interaction"
                    .to_string(),
            ));
        }
        let message_id = Uuid::new_v4().to_string();
        let sort_order = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM messages WHERE conversation_id = ?1",
            rusqlite::params![&conversation_id],
            |row| row.get::<_, i64>(0),
        )?;
        let artifacts = serde_json::json!({
            "kind": "interactionSupplement",
            "version": 1,
            "interactionId": interaction_id,
        });
        let artifacts_json = serde_json::to_string(&artifacts)?;
        tx.execute(
            "INSERT INTO messages
             (id, conversation_id, role, content, tool_call_id, tool_calls_json,
              artifacts_json, token_count, sort_order, thinking, image_attachments_json)
             VALUES (?1, ?2, 'user', ?3, NULL, NULL, ?4, ?5, ?6, NULL, NULL)",
            rusqlite::params![
                &message_id,
                &conversation_id,
                &content,
                &artifacts_json,
                crate::conversation::memory::estimate_tokens(&content),
                sort_order,
            ],
        )?;
        tx.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&conversation_id],
        )?;
        let created_at: String = tx.query_row(
            "SELECT created_at FROM messages WHERE id = ?1",
            rusqlite::params![&message_id],
            |row| row.get(0),
        )?;
        tx.commit()?;
        drop(conn);
        let token_count = crate::conversation::memory::estimate_tokens(&content);
        let _ = self.record_agent_task_run_event(
            &run_id,
            "interaction_supplemented",
            "User added supplementary context",
            Some("awaiting_user_input"),
            Some(&serde_json::json!({ "interactionId": interaction_id })),
        );
        Ok(ConversationMessage {
            id: message_id,
            conversation_id,
            role: Role::User,
            content,
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: Some(artifacts),
            token_count,
            created_at,
            sort_order,
            thinking: None,
            image_attachments: None,
        })
    }

    /// Atomically consume an interaction answer, append its hidden transcript
    /// message, and re-queue the original turn/run. Replays with the same
    /// launch key return the same tuple and never append a second response.
    pub fn resume_agent_turn_with_interaction_response(
        &self,
        message: &ConversationMessage,
        provider: Option<&str>,
        model: Option<&str>,
        idempotency_key: &str,
        input: &SubmitInteractionResponse,
    ) -> Result<AgentTurnLaunchRecord, CoreError> {
        let idempotency_key =
            normalize_required_text(idempotency_key, "response launch idempotency key", 256)?;
        if message.role != Role::User {
            return Err(CoreError::InvalidInput(
                "Interaction response message must have the user role".to_string(),
            ));
        }
        let tool_calls_json = if message.tool_calls.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&message.tool_calls)?)
        };
        let artifacts_json = message
            .artifacts
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let image_attachments_json = message
            .image_attachments
            .as_ref()
            .filter(|attachments| !attachments.is_empty())
            .map(serde_json::to_string)
            .transpose()?;

        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let request = tx
            .query_row(
                &format!("SELECT {REQUEST_COLUMNS} FROM interaction_requests WHERE id = ?1"),
                rusqlite::params![&input.interaction_id],
                request_from_row,
            )
            .optional()?
            .ok_or_else(|| {
                CoreError::NotFound(format!("Interaction request {}", input.interaction_id))
            })?;
        if request.conversation_id != message.conversation_id {
            return Err(CoreError::InvalidInput(
                "Interaction response belongs to a different conversation".to_string(),
            ));
        }
        if !bool::from(
            request
                .resume_token
                .as_bytes()
                .ct_eq(input.resume_token.as_bytes()),
        ) {
            return Err(CoreError::InvalidInput(
                "Interaction resume token is invalid or stale".to_string(),
            ));
        }
        let answers = validate_answers(&request, &input.answers)?;
        let answers_json = serde_json::to_string(&answers)?;
        let existing_response = tx
            .query_row(
                "SELECT id, answers_json, launch_idempotency_key, response_message_id
                 FROM interaction_responses WHERE interaction_id = ?1",
                rusqlite::params![&input.interaction_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?;

        if let Some((_, persisted_answers, launch_key, Some(response_message_id))) =
            existing_response.as_ref()
        {
            if persisted_answers != &answers_json {
                return Err(CoreError::InvalidInput(
                    "Interaction response was already launched with different input".to_string(),
                ));
            }
            if launch_key.as_deref() != Some(idempotency_key.as_str()) {
                return Err(CoreError::InvalidInput(
                    "Interaction response was already launched with a different idempotency key"
                        .to_string(),
                ));
            }
            let (run_id, mut run_status) = tx.query_row(
                "SELECT i.run_id, r.status
                 FROM interaction_requests i
                 JOIN agent_task_runs r ON r.id = i.run_id
                 WHERE i.id = ?1",
                rusqlite::params![&input.interaction_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            let sort_order = tx.query_row(
                "SELECT sort_order FROM messages WHERE id = ?1",
                rusqlite::params![response_message_id],
                |row| row.get::<_, i64>(0),
            )?;
            let latest_response_message_id = tx
                .query_row(
                    "SELECT response.response_message_id
                     FROM interaction_responses response
                     JOIN interaction_requests sibling
                       ON sibling.id = response.interaction_id
                     JOIN messages message
                       ON message.id = response.response_message_id
                     WHERE sibling.run_id = ?1
                       AND response.response_message_id IS NOT NULL
                     ORDER BY message.sort_order DESC, response.rowid DESC
                     LIMIT 1",
                    rusqlite::params![&run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let active_siblings: i64 = tx.query_row(
                "SELECT COUNT(*) FROM interaction_requests
                 WHERE run_id = ?1 AND id != ?2
                   AND (
                     status IN ('pending', 'presented', 'partially_answered')
                     OR (
                       status = 'submitted'
                       AND EXISTS (
                         SELECT 1 FROM interaction_responses response
                         WHERE response.interaction_id = interaction_requests.id
                           AND response.response_message_id IS NULL
                       )
                     )
                   )",
                rusqlite::params![&run_id, &input.interaction_id],
                |row| row.get(0),
            )?;
            let recovering_interrupted_launch = matches!(
                request.status,
                InteractionStatus::Submitted | InteractionStatus::Acknowledged
            ) && run_status == "awaiting_user_input"
                && latest_response_message_id.as_deref() == Some(response_message_id.as_str())
                && active_siblings == 0;
            if recovering_interrupted_launch {
                let turn_requeued = tx.execute(
                    "UPDATE conversation_turns
                     SET status = 'running', finished_at = NULL, updated_at = datetime('now')
                     WHERE id = ?1 AND status = 'awaiting_user_input'",
                    rusqlite::params![&request.turn_id],
                )?;
                if turn_requeued != 1 {
                    return Err(CoreError::InvalidInput(
                        "Interrupted interaction turn could not be re-queued".to_string(),
                    ));
                }
                let requeued = tx.execute(
                    "UPDATE agent_task_runs
                     SET status = 'queued', phase = 'queued', summary = 'Recovering user input',
                         error_message = NULL, finished_at = NULL,
                         updated_at = datetime('now')
                     WHERE id = ?1 AND status = 'awaiting_user_input'",
                    rusqlite::params![&run_id],
                )?;
                if requeued != 1 {
                    return Err(CoreError::InvalidInput(
                        "Interrupted interaction continuation could not be re-queued".to_string(),
                    ));
                }
                tx.execute(
                    "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
                    rusqlite::params![&message.conversation_id],
                )?;
                run_status = "queued".to_string();
            }
            tx.commit()?;
            return Ok(AgentTurnLaunchRecord {
                conversation_id: message.conversation_id.clone(),
                user_message_id: response_message_id.clone(),
                user_message_sort_order: sort_order,
                turn_id: request.turn_id,
                run_id,
                status: run_status,
                reused: !recovering_interrupted_launch,
            });
        }

        let current_run_status: String = tx.query_row(
            "SELECT r.status
             FROM agent_task_runs r
             JOIN interaction_requests i ON i.run_id = r.id
             WHERE i.id = ?1",
            rusqlite::params![&input.interaction_id],
            |row| row.get(0),
        )?;
        if current_run_status != "awaiting_user_input" {
            return Err(CoreError::InvalidInput(format!(
                "Interaction {} cannot resume task from status {current_run_status}",
                input.interaction_id
            )));
        }

        let (response_id, newly_consumed) =
            if let Some((response_id, persisted_answers, launch_key, None)) = existing_response {
                if persisted_answers != answers_json
                    || launch_key
                        .as_deref()
                        .is_some_and(|key| key != idempotency_key.as_str())
                {
                    return Err(CoreError::InvalidInput(
                        "Interaction response was already submitted with different input"
                            .to_string(),
                    ));
                }
                (response_id, None)
            } else {
                let consumed = consume_interaction_response_in_transaction(
                    &tx,
                    input,
                    Some(&message.conversation_id),
                )?;
                let response_id = tx.query_row(
                    "SELECT id FROM interaction_responses WHERE interaction_id = ?1",
                    rusqlite::params![&input.interaction_id],
                    |row| row.get::<_, String>(0),
                )?;
                (response_id, Some(consumed))
            };

        let user_message_sort_order = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1
             FROM messages WHERE conversation_id = ?1",
            rusqlite::params![&message.conversation_id],
            |row| row.get::<_, i64>(0),
        )?;
        tx.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_call_id,
             tool_calls_json, artifacts_json, token_count, sort_order, thinking,
             image_attachments_json)
             VALUES (?1, ?2, 'user', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                &message.id,
                &message.conversation_id,
                &message.content,
                &message.tool_call_id,
                &tool_calls_json,
                &artifacts_json,
                message.token_count,
                user_message_sort_order,
                &message.thinking,
                &image_attachments_json,
            ],
        )?;
        tx.execute(
            "UPDATE interaction_responses
             SET launch_idempotency_key = ?2, response_message_id = ?3
             WHERE id = ?1",
            rusqlite::params![&response_id, &idempotency_key, &message.id],
        )?;
        let run_id: String = tx.query_row(
            "SELECT run_id FROM interaction_requests WHERE id = ?1",
            rusqlite::params![&input.interaction_id],
            |row| row.get(0),
        )?;
        let active_siblings: i64 = tx.query_row(
            "SELECT COUNT(*) FROM interaction_requests
             WHERE run_id = ?1 AND id != ?2
               AND (
                 status IN ('pending', 'presented', 'partially_answered')
                 OR (
                   status = 'submitted'
                   AND EXISTS (
                     SELECT 1 FROM interaction_responses response
                     WHERE response.interaction_id = interaction_requests.id
                       AND response.response_message_id IS NULL
                   )
                 )
               )",
            rusqlite::params![&run_id, &input.interaction_id],
            |row| row.get(0),
        )?;
        if active_siblings > 0 {
            tx.execute(
                "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![&message.conversation_id],
            )?;
            tx.commit()?;
            drop(conn);
            if let Some(consumed) = newly_consumed.as_ref() {
                self.record_interaction_submitted_event(consumed);
            }
            return Ok(AgentTurnLaunchRecord {
                conversation_id: message.conversation_id.clone(),
                user_message_id: message.id.clone(),
                user_message_sort_order,
                turn_id: request.turn_id,
                run_id,
                status: "awaiting_user_input".to_string(),
                reused: true,
            });
        }
        let turn_updated = tx.execute(
            "UPDATE conversation_turns
             SET status = 'running', finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND status = 'awaiting_user_input'",
            rusqlite::params![&request.turn_id],
        )?;
        if turn_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Interaction turn changed while its response was being resumed".to_string(),
            ));
        }
        let run_updated = tx.execute(
            "UPDATE agent_task_runs
             SET status = 'queued', phase = 'queued',
                 summary = 'User input received', error_message = NULL,
                 provider = COALESCE(?2, provider), model = COALESCE(?3, model),
                 finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND status = 'awaiting_user_input'",
            rusqlite::params![&run_id, provider, model],
        )?;
        if run_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Interaction task changed while its response was being resumed".to_string(),
            ));
        }
        tx.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&message.conversation_id],
        )?;
        tx.commit()?;
        drop(conn);
        if let Some(consumed) = newly_consumed.as_ref() {
            self.record_interaction_submitted_event(consumed);
        }
        Ok(AgentTurnLaunchRecord {
            conversation_id: message.conversation_id.clone(),
            user_message_id: message.id.clone(),
            user_message_sort_order,
            turn_id: request.turn_id,
            run_id,
            status: "queued".to_string(),
            reused: false,
        })
    }

    /// Locate the most recent run whose executor is suspended at the durable
    /// user-input barrier. Callers can persist a cancelling Run Event before
    /// mutating the interaction rows, so a crash cannot lose the stop intent.
    pub fn stoppable_interaction_run_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<String>, CoreError> {
        let mut connection = self.conn();
        expire_due_requests(&mut connection)?;
        connection
            .query_row(
                "SELECT id FROM agent_task_runs
                 WHERE conversation_id = ?1
                   AND status IN ('awaiting_user_input', 'cancelling')
                 ORDER BY updated_at DESC, created_at DESC, id DESC LIMIT 1",
                rusqlite::params![conversation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(CoreError::Database)
    }

    /// Mark every resumable interaction owned by an intentionally stopped run
    /// as cancelled. A cancelled task run otherwise looks identical to a run
    /// interrupted between response acknowledgement and continuation startup,
    /// which would incorrectly offer the saved response as crash recovery.
    pub fn cancel_interactions_for_stopped_run(&self, run_id: &str) -> Result<usize, CoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO interaction_events (interaction_id, from_status, to_status, reason)
             SELECT id, status, 'cancelled', 'task_cancelled'
             FROM interaction_requests
             WHERE run_id = ?1
               AND status IN (
                 'pending', 'presented', 'partially_answered', 'submitted', 'acknowledged'
               )",
            rusqlite::params![run_id],
        )?;
        let updated = tx.execute(
            "UPDATE interaction_requests
             SET status = 'cancelled', updated_at = datetime('now')
             WHERE run_id = ?1
               AND status IN (
                 'pending', 'presented', 'partially_answered', 'submitted', 'acknowledged'
               )",
            rusqlite::params![run_id],
        )?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn create_interaction_request(
        &self,
        input: &CreateInteractionRequest,
    ) -> Result<CreatedInteractionRequest, CoreError> {
        let idempotency_key =
            normalize_required_text(&input.idempotency_key, "idempotency key", 256)?;
        let title = normalize_required_text(&input.title, "title", 160)?;
        let questions = normalize_questions(&input.questions)?;
        let questions_json = serde_json::to_string(&questions)?;
        let description =
            normalize_optional_text(input.description.as_deref(), "description", 4_000)?;
        let expires_at = normalize_expiry(input.expires_at.as_deref())?;
        let tool_call_id =
            normalize_optional_text(input.tool_call_id.as_deref(), "tool call id", 256)?;

        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run_id = tx
            .query_row(
                "SELECT id FROM agent_task_runs WHERE conversation_id = ?1 AND turn_id = ?2",
                rusqlite::params![&input.conversation_id, &input.turn_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "Interaction turn is not attached to the requested conversation".to_string(),
                )
            })?;

        let existing = tx
            .query_row(
                &format!(
                    "SELECT {REQUEST_COLUMNS} FROM interaction_requests
                     WHERE conversation_id = ?1 AND turn_id = ?2 AND idempotency_key = ?3"
                ),
                rusqlite::params![&input.conversation_id, &input.turn_id, &idempotency_key],
                request_from_row,
            )
            .optional()?;
        if let Some(existing) = existing {
            if existing.kind != input.kind
                || existing.title != title
                || existing.description != description
                || existing.questions != questions
                || existing.required != input.required
                || existing.expires_at != expires_at
                || existing.tool_call_id != tool_call_id
            {
                return Err(CoreError::InvalidInput(
                    "Interaction idempotency key was reused with different content".to_string(),
                ));
            }
            tx.commit()?;
            return Ok(CreatedInteractionRequest {
                request: existing,
                reused: true,
            });
        }

        let interaction_id = Uuid::new_v4().to_string();
        let resume_token = Uuid::new_v4().to_string();
        tx.execute("INSERT INTO interaction_queue_clock DEFAULT VALUES", [])?;
        let queue_sequence = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO interaction_requests
             (id, conversation_id, turn_id, run_id, tool_call_id, idempotency_key,
              kind, title, description, questions_json, required, status,
              risk_priority, queue_sequence, resume_token, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     'pending', ?12, ?13, ?14, ?15)",
            rusqlite::params![
                &interaction_id,
                &input.conversation_id,
                &input.turn_id,
                &run_id,
                &tool_call_id,
                &idempotency_key,
                input.kind.as_str(),
                &title,
                &description,
                &questions_json,
                input.required,
                input.kind.risk_priority(),
                queue_sequence,
                &resume_token,
                &expires_at,
            ],
        )?;
        tx.execute(
            "INSERT INTO interaction_events (interaction_id, from_status, to_status)
             VALUES (?1, NULL, 'pending')",
            rusqlite::params![&interaction_id],
        )?;
        tx.commit()?;
        drop(conn);
        let request = self.get_interaction_request(&interaction_id)?;
        let _ = self.record_agent_task_run_event(
            &run_id,
            "interaction_requested",
            &title,
            Some(request.status.as_str()),
            Some(&serde_json::json!({
                "version": INTERACTION_PROTOCOL_VERSION,
                "interactionId": interaction_id,
                "kind": input.kind,
            })),
        );
        Ok(CreatedInteractionRequest {
            request,
            reused: false,
        })
    }

    pub fn get_interaction_request(
        &self,
        interaction_id: &str,
    ) -> Result<InteractionRequest, CoreError> {
        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        conn.query_row(
            &format!("SELECT {REQUEST_COLUMNS} FROM interaction_requests WHERE id = ?1"),
            rusqlite::params![interaction_id],
            request_from_row,
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Interaction request {interaction_id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn get_interaction_request_run_id(
        &self,
        interaction_id: &str,
    ) -> Result<String, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT run_id FROM interaction_requests WHERE id = ?1",
            rusqlite::params![interaction_id],
            |row| row.get(0),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Interaction request {interaction_id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_interaction_requests(
        &self,
        conversation_id: Option<&str>,
        include_terminal: bool,
    ) -> Result<Vec<InteractionRequest>, CoreError> {
        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        let status_filter = if include_terminal {
            String::new()
        } else {
            " AND (
                status IN ('pending', 'presented', 'partially_answered')
                OR (
                  status = 'submitted'
                  AND (
                    EXISTS (
                      SELECT 1 FROM interaction_responses response
                      WHERE response.interaction_id = interaction_requests.id
                        AND response.response_message_id IS NULL
                    )
                    OR EXISTS (
                      SELECT 1 FROM agent_task_runs run
                      WHERE run.id = interaction_requests.run_id
                        AND run.status IN ('cancelled', 'failed')
                    )
                  )
                )
                OR (
                  status = 'acknowledged'
                  AND EXISTS (
                    SELECT 1 FROM agent_task_runs run
                    WHERE run.id = interaction_requests.run_id
                      AND run.status IN ('cancelled', 'failed')
                  )
                )
              )"
            .to_string()
        };
        let (sql, params): (String, Vec<String>) = match conversation_id {
            Some(conversation_id) => (
                format!(
                    "SELECT {REQUEST_COLUMNS} FROM interaction_requests
                     WHERE conversation_id = ?1{status_filter}
                     ORDER BY risk_priority DESC, queue_sequence"
                ),
                vec![conversation_id.to_string()],
            ),
            None => (
                format!(
                    "SELECT {REQUEST_COLUMNS} FROM interaction_requests
                     WHERE 1 = 1{status_filter}
                     ORDER BY risk_priority DESC, queue_sequence"
                ),
                Vec::new(),
            ),
        };
        let mut statement = conn.prepare(&sql)?;
        let rows =
            statement.query_map(rusqlite::params_from_iter(params.iter()), request_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(CoreError::Database)
    }

    /// Whether a run still owns an interaction that legitimately keeps it at
    /// the user-input barrier. Submitted responses without a transcript
    /// message are unresolved outbox entries; launched responses are not.
    pub fn agent_run_has_unresolved_interactions(&self, run_id: &str) -> Result<bool, CoreError> {
        let connection = self.conn();
        Self::agent_run_has_unresolved_interactions_on_connection(&connection, run_id)
    }

    pub(crate) fn agent_run_has_unresolved_interactions_on_connection(
        connection: &rusqlite::Connection,
        run_id: &str,
    ) -> Result<bool, CoreError> {
        connection
            .query_row(
                "SELECT EXISTS(
               SELECT 1 FROM interaction_requests request
               LEFT JOIN interaction_responses response
                 ON response.interaction_id = request.id
               WHERE request.run_id = ?1
                 AND (
                   request.status IN ('pending', 'presented', 'partially_answered')
                   OR (
                     request.status = 'submitted'
                     AND response.response_message_id IS NULL
                   )
                 )
             )",
                rusqlite::params![run_id],
                |row| row.get(0),
            )
            .map_err(CoreError::Database)
    }

    pub fn mark_interaction_presented(
        &self,
        interaction_id: &str,
    ) -> Result<InteractionRequest, CoreError> {
        self.transition_interaction_status(
            interaction_id,
            &[InteractionStatus::Pending],
            InteractionStatus::Presented,
            &[
                InteractionStatus::PartiallyAnswered,
                InteractionStatus::Submitted,
                InteractionStatus::Acknowledged,
            ],
        )
    }

    pub fn mark_interaction_partially_answered(
        &self,
        interaction_id: &str,
    ) -> Result<InteractionRequest, CoreError> {
        self.transition_interaction_status(
            interaction_id,
            &[InteractionStatus::Presented],
            InteractionStatus::PartiallyAnswered,
            &[
                InteractionStatus::Submitted,
                InteractionStatus::Acknowledged,
            ],
        )
    }

    pub fn acknowledge_interaction(
        &self,
        interaction_id: &str,
    ) -> Result<InteractionRequest, CoreError> {
        self.transition_interaction_status(
            interaction_id,
            &[InteractionStatus::Submitted],
            InteractionStatus::Acknowledged,
            &[],
        )
    }

    /// A continuation may collect more than one queued interaction before the
    /// shared run is eligible to restart. Once it does restart, acknowledge
    /// every submitted response attached to that run in one transaction.
    pub fn acknowledge_submitted_interactions_for_run(
        &self,
        run_id: &str,
    ) -> Result<usize, CoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO interaction_events (interaction_id, from_status, to_status, reason)
             SELECT id, status, 'acknowledged', 'turn_resumed'
             FROM interaction_requests
             WHERE run_id = ?1 AND status = 'submitted'",
            rusqlite::params![run_id],
        )?;
        let updated = tx.execute(
            "UPDATE interaction_requests
             SET status = 'acknowledged', updated_at = datetime('now')
             WHERE run_id = ?1 AND status = 'submitted'",
            rusqlite::params![run_id],
        )?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn cancel_interaction(
        &self,
        interaction_id: &str,
    ) -> Result<InteractionRequest, CoreError> {
        self.transition_interaction_status(
            interaction_id,
            &[
                InteractionStatus::Pending,
                InteractionStatus::Presented,
                InteractionStatus::PartiallyAnswered,
            ],
            InteractionStatus::Cancelled,
            &[],
        )
    }

    pub fn supersede_interaction(
        &self,
        interaction_id: &str,
    ) -> Result<InteractionRequest, CoreError> {
        self.transition_interaction_status(
            interaction_id,
            &[
                InteractionStatus::Pending,
                InteractionStatus::Presented,
                InteractionStatus::PartiallyAnswered,
            ],
            InteractionStatus::Superseded,
            &[],
        )
    }

    pub fn fail_interaction(&self, interaction_id: &str) -> Result<InteractionRequest, CoreError> {
        self.transition_interaction_status(
            interaction_id,
            &[
                InteractionStatus::Pending,
                InteractionStatus::Presented,
                InteractionStatus::PartiallyAnswered,
                InteractionStatus::Submitted,
            ],
            InteractionStatus::Failed,
            &[],
        )
    }

    fn transition_interaction_status(
        &self,
        interaction_id: &str,
        from: &[InteractionStatus],
        to: InteractionStatus,
        accept_if_advanced_to: &[InteractionStatus],
    ) -> Result<InteractionRequest, CoreError> {
        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = tx
            .query_row(
                &format!("SELECT {REQUEST_COLUMNS} FROM interaction_requests WHERE id = ?1"),
                rusqlite::params![interaction_id],
                request_from_row,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("Interaction request {interaction_id}")))?;
        if current.status == to || accept_if_advanced_to.contains(&current.status) {
            return Ok(current);
        }
        if !from.contains(&current.status) {
            return Err(CoreError::InvalidInput(format!(
                "Interaction {interaction_id} cannot transition from {} to {}",
                current.status.as_str(),
                to.as_str()
            )));
        }
        let affected = tx.execute(
            "UPDATE interaction_requests
             SET status = ?2, updated_at = datetime('now')
             WHERE id = ?1 AND status = ?3",
            rusqlite::params![interaction_id, to.as_str(), current.status.as_str()],
        )?;
        if affected != 1 {
            return Err(CoreError::InvalidInput(format!(
                "Interaction {interaction_id} changed while it was being updated"
            )));
        }
        tx.execute(
            "INSERT INTO interaction_events (interaction_id, from_status, to_status)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![interaction_id, current.status.as_str(), to.as_str()],
        )?;
        tx.commit()?;
        drop(conn);
        self.get_interaction_request(interaction_id)
    }

    pub fn submit_interaction_response(
        &self,
        input: &SubmitInteractionResponse,
    ) -> Result<InteractionResponse, CoreError> {
        let mut conn = self.conn();
        expire_due_requests(&mut conn)?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let consumed = consume_interaction_response_in_transaction(&tx, input, None)?;
        tx.commit()?;
        drop(conn);
        self.record_interaction_submitted_event(&consumed);
        Ok(consumed.response)
    }

    pub(crate) fn record_interaction_submitted_event(
        &self,
        consumed: &ConsumedInteractionResponse,
    ) {
        let _ = self.record_agent_task_run_event(
            &consumed.run_id,
            "interaction_submitted",
            &consumed.title,
            Some(InteractionStatus::Submitted.as_str()),
            Some(&serde_json::json!({
                "version": INTERACTION_PROTOCOL_VERSION,
                "interactionId": consumed.response.interaction_id,
            })),
        );
    }

    pub fn get_interaction_response(
        &self,
        interaction_id: &str,
    ) -> Result<InteractionResponse, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT interaction_id, answers_json, submitted_at
             FROM interaction_responses WHERE interaction_id = ?1",
            rusqlite::params![interaction_id],
            |row| {
                Ok(InteractionResponse {
                    schema_version: INTERACTION_PROTOCOL_VERSION,
                    interaction_id: row.get(0)?,
                    answers: decode_json(row.get(1)?, 1)?,
                    submitted_at: row.get(2)?,
                })
            },
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Interaction response {interaction_id}"))
            }
            other => CoreError::Database(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::llm::Role;

    struct Fixture {
        db: Database,
        conversation_id: String,
        turn_id: String,
        user_message_id: String,
    }

    fn fixture() -> Fixture {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "openai".into(),
                model: "gpt-5".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "Choose a scope".into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 3,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .unwrap();
        db.create_agent_task_run(
            &conversation.id,
            &turn.id,
            &message.id,
            "Choose a scope",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
        Fixture {
            db,
            conversation_id: conversation.id,
            turn_id: turn.id,
            user_message_id: message.id,
        }
    }

    fn request_input(fixture: &Fixture, key: &str) -> CreateInteractionRequest {
        CreateInteractionRequest {
            conversation_id: fixture.conversation_id.clone(),
            turn_id: fixture.turn_id.clone(),
            tool_call_id: Some(key.to_string()),
            idempotency_key: key.to_string(),
            kind: InteractionKind::UserInput,
            title: "Scope".into(),
            description: Some("Select the scope to continue.".into()),
            questions: vec![InteractionQuestion {
                id: "scope".into(),
                header: "Scope".into(),
                question: "Which scope?".into(),
                kind: InteractionQuestionKind::SingleChoice,
                options: vec![
                    InteractionQuestionOption {
                        label: "App".into(),
                        description: None,
                    },
                    InteractionQuestionOption {
                        label: "Repo".into(),
                        description: None,
                    },
                ],
                placeholder: None,
                why: None,
            }],
            required: true,
            expires_at: None,
        }
    }

    #[test]
    fn creation_is_idempotent_but_rejects_payload_drift() {
        let fixture = fixture();
        let input = request_input(&fixture, "call-1");
        let first = fixture.db.create_interaction_request(&input).unwrap();
        let second = fixture.db.create_interaction_request(&input).unwrap();
        assert!(!first.reused);
        assert!(second.reused);
        assert_eq!(first.request.interaction_id, second.request.interaction_id);
        assert_eq!(first.request.resume_token, second.request.resume_token);

        let mut changed = input;
        changed.title = "Different".into();
        assert!(fixture.db.create_interaction_request(&changed).is_err());
    }

    #[test]
    fn queue_orders_risk_before_fifo() {
        let fixture = fixture();
        let low = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-low"))
            .unwrap();
        let mut high_input = request_input(&fixture, "call-high");
        high_input.kind = InteractionKind::HighRiskConfirmation;
        high_input.title = "Confirm write".into();
        let high = fixture.db.create_interaction_request(&high_input).unwrap();
        let mut approval_input = request_input(&fixture, "call-approval");
        approval_input.kind = InteractionKind::Approval;
        approval_input.title = "Approve change".into();
        let approval = fixture
            .db
            .create_interaction_request(&approval_input)
            .unwrap();
        let queue = fixture
            .db
            .list_interaction_requests(Some(&fixture.conversation_id), false)
            .unwrap();
        assert_eq!(queue[0].interaction_id, high.request.interaction_id);
        assert_eq!(queue[1].interaction_id, approval.request.interaction_id);
        assert_eq!(queue[2].interaction_id, low.request.interaction_id);
    }

    #[test]
    fn queue_sequence_remains_monotonic_after_deletion() {
        let fixture = fixture();
        let first = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-first"))
            .unwrap();
        fixture
            .db
            .conn()
            .execute(
                "DELETE FROM interaction_requests WHERE id = ?1",
                rusqlite::params![&first.request.interaction_id],
            )
            .unwrap();
        let second = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-second"))
            .unwrap();
        assert!(second.request.queue_sequence > first.request.queue_sequence);
    }

    #[test]
    fn response_requires_token_and_can_only_be_submitted_once() {
        let fixture = fixture();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-1"))
            .unwrap();
        fixture
            .db
            .mark_interaction_presented(&created.request.interaction_id)
            .unwrap();
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["Repo".into()]);
        let invalid = SubmitInteractionResponse {
            interaction_id: created.request.interaction_id.clone(),
            resume_token: "stale".into(),
            answers: answers.clone(),
        };
        assert!(fixture.db.submit_interaction_response(&invalid).is_err());

        let valid = SubmitInteractionResponse {
            interaction_id: created.request.interaction_id.clone(),
            resume_token: created.request.resume_token.clone(),
            answers,
        };
        let response = fixture.db.submit_interaction_response(&valid).unwrap();
        assert_eq!(response.answers["scope"], vec!["Repo"]);
        assert!(fixture.db.submit_interaction_response(&valid).is_err());
        assert_eq!(
            fixture
                .db
                .get_interaction_request(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Submitted
        );
    }

    #[test]
    fn failed_turn_launch_rolls_back_interaction_submission() {
        let fixture = fixture();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-atomic"))
            .unwrap();
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["Repo".into()]);
        let submission = SubmitInteractionResponse {
            interaction_id: created.request.interaction_id.clone(),
            resume_token: created.request.resume_token,
            answers,
        };
        fixture
            .db
            .mark_interaction_presented(&created.request.interaction_id)
            .unwrap();
        fixture
            .db
            .suspend_agent_turn_for_interaction(&created.request.interaction_id)
            .unwrap();
        let mut follow_up = ConversationMessage {
            id: fixture.user_message_id.clone(),
            conversation_id: fixture.conversation_id.clone(),
            role: Role::User,
            content: "Repo".into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 1,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        assert!(fixture
            .db
            .create_agent_turn_and_run_with_interaction_response(
                &follow_up,
                "Repo",
                Some("openai"),
                Some("gpt-5"),
                "launch-atomic",
                Some(&submission),
            )
            .is_err());
        assert_eq!(
            fixture
                .db
                .get_interaction_request(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Presented
        );
        assert!(fixture
            .db
            .get_interaction_response(&created.request.interaction_id)
            .is_err());

        follow_up.id = Uuid::new_v4().to_string();
        fixture
            .db
            .create_agent_turn_and_run_with_interaction_response(
                &follow_up,
                "Repo",
                Some("openai"),
                Some("gpt-5"),
                "launch-atomic",
                Some(&submission),
            )
            .unwrap();
        assert_eq!(
            fixture
                .db
                .get_interaction_request(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Submitted
        );
    }

    #[test]
    fn lifecycle_moves_forward_and_rejects_terminal_regression() {
        let fixture = fixture();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-1"))
            .unwrap();
        assert!(fixture
            .db
            .mark_interaction_partially_answered(&created.request.interaction_id)
            .is_err());
        assert_eq!(
            fixture
                .db
                .mark_interaction_presented(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Presented
        );
        assert_eq!(
            fixture
                .db
                .mark_interaction_partially_answered(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::PartiallyAnswered
        );
        assert_eq!(
            fixture
                .db
                .mark_interaction_presented(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::PartiallyAnswered
        );
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["App".into()]);
        fixture
            .db
            .submit_interaction_response(&SubmitInteractionResponse {
                interaction_id: created.request.interaction_id.clone(),
                resume_token: created.request.resume_token,
                answers,
            })
            .unwrap();
        assert_eq!(
            fixture
                .db
                .mark_interaction_partially_answered(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Submitted
        );
        assert_eq!(
            fixture
                .db
                .acknowledge_interaction(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Acknowledged
        );
        assert!(fixture
            .db
            .cancel_interaction(&created.request.interaction_id)
            .is_err());
        let transitions = {
            let conn = fixture.db.conn();
            let mut statement = conn
                .prepare(
                    "SELECT to_status FROM interaction_events
                     WHERE interaction_id = ?1 ORDER BY id",
                )
                .unwrap();
            statement
                .query_map(rusqlite::params![&created.request.interaction_id], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            transitions,
            vec![
                "pending",
                "presented",
                "partially_answered",
                "submitted",
                "acknowledged",
            ]
        );
    }

    #[test]
    fn superseded_and_failed_are_reachable_terminal_states() {
        let fixture = fixture();
        let superseded = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-superseded"))
            .unwrap();
        assert_eq!(
            fixture
                .db
                .supersede_interaction(&superseded.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Superseded
        );

        let failed = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-failed"))
            .unwrap();
        assert_eq!(
            fixture
                .db
                .fail_interaction(&failed.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Failed
        );
    }

    #[test]
    fn expired_request_cannot_be_submitted() {
        let fixture = fixture();
        let mut input = request_input(&fixture, "call-expired");
        input.expires_at = Some("2000-01-01T00:00:00Z".into());
        let created = fixture.db.create_interaction_request(&input).unwrap();
        let expired = fixture
            .db
            .get_interaction_request(&created.request.interaction_id)
            .unwrap();
        assert_eq!(expired.status, InteractionStatus::Expired);
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["Repo".into()]);
        assert!(fixture
            .db
            .submit_interaction_response(&SubmitInteractionResponse {
                interaction_id: expired.interaction_id,
                resume_token: expired.resume_token,
                answers,
            })
            .is_err());
    }

    #[test]
    fn transactional_submission_rechecks_the_deadline() {
        let fixture = fixture();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-deadline"))
            .unwrap();
        fixture
            .db
            .mark_interaction_presented(&created.request.interaction_id)
            .unwrap();
        fixture
            .db
            .conn()
            .execute(
                "UPDATE interaction_requests SET expires_at = '2000-01-01T00:00:00Z'
                 WHERE id = ?1",
                rusqlite::params![&created.request.interaction_id],
            )
            .unwrap();
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["Repo".into()]);
        let submission = SubmitInteractionResponse {
            interaction_id: created.request.interaction_id.clone(),
            resume_token: created.request.resume_token,
            answers,
        };
        let mut conn = fixture.db.conn();
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        assert!(consume_interaction_response_in_transaction(&tx, &submission, None).is_err());
        tx.rollback().unwrap();
        drop(conn);
        assert!(fixture
            .db
            .get_interaction_response(&created.request.interaction_id)
            .is_err());
    }

    #[test]
    fn pending_request_remains_durable_until_an_explicit_transition() {
        let fixture = fixture();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-1"))
            .unwrap();
        let request = fixture
            .db
            .get_interaction_request(&created.request.interaction_id)
            .unwrap();
        assert_eq!(request.status, InteractionStatus::Pending);
    }

    #[test]
    fn suspended_interaction_resumes_the_original_turn_exactly_once() {
        let fixture = fixture();
        let original_run = fixture
            .db
            .get_agent_task_run_by_turn(&fixture.turn_id)
            .unwrap()
            .unwrap();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-resume"))
            .unwrap();
        assert!(fixture
            .db
            .suspend_agent_turn_for_interaction(&created.request.interaction_id)
            .unwrap());
        assert_eq!(
            fixture
                .db
                .get_agent_task_run(&original_run.id)
                .unwrap()
                .status,
            "awaiting_user_input"
        );
        assert_eq!(
            fixture
                .db
                .get_conversation_turn(&fixture.turn_id)
                .unwrap()
                .status,
            "awaiting_user_input"
        );

        let supplement = fixture
            .db
            .append_interaction_supplement(
                &created.request.interaction_id,
                "Keep legacy configuration compatibility.",
            )
            .unwrap();
        assert_eq!(
            supplement.artifacts.as_ref().unwrap()["kind"],
            "interactionSupplement"
        );
        fixture
            .db
            .mark_interaction_presented(&created.request.interaction_id)
            .unwrap();
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["Repo".into()]);
        let input = SubmitInteractionResponse {
            interaction_id: created.request.interaction_id.clone(),
            resume_token: created.request.resume_token,
            answers,
        };
        let response_message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: fixture.conversation_id.clone(),
            role: Role::User,
            content: "Which scope?\nRepo".into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: Some(serde_json::json!({
                "kind": "questionResponse",
                "version": 2,
                "interactionId": created.request.interaction_id,
                "requestCallId": "call-resume",
                "answers": [{ "id": "scope", "question": "Which scope?", "answers": ["Repo"] }],
            })),
            token_count: 4,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        let launch = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &response_message,
                Some("openai"),
                Some("gpt-5"),
                "resume-launch-1",
                &input,
            )
            .unwrap();
        assert_eq!(launch.turn_id, fixture.turn_id);
        assert_eq!(launch.run_id, original_run.id);
        assert_eq!(launch.user_message_id, response_message.id);
        assert_eq!(launch.status, "queued");
        assert!(!launch.reused);
        assert_eq!(
            fixture
                .db
                .get_conversation_turn(&fixture.turn_id)
                .unwrap()
                .status,
            "running"
        );

        let replay = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &response_message,
                Some("openai"),
                Some("gpt-5"),
                "resume-launch-1",
                &input,
            )
            .unwrap();
        assert!(replay.reused);
        assert_eq!(replay.turn_id, fixture.turn_id);
        assert_eq!(replay.run_id, original_run.id);
        let response_messages = fixture
            .db
            .get_messages(&fixture.conversation_id)
            .unwrap()
            .into_iter()
            .filter(|message| {
                message.artifacts.as_ref().is_some_and(|artifact| {
                    artifact.get("kind").and_then(serde_json::Value::as_str)
                        == Some("questionResponse")
                })
            })
            .count();
        assert_eq!(response_messages, 1);
    }

    #[test]
    fn suspended_interaction_cancellation_leaves_run_terminalization_to_outbox() {
        let fixture = fixture();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-cancel"))
            .unwrap();
        fixture
            .db
            .suspend_agent_turn_for_interaction(&created.request.interaction_id)
            .unwrap();
        let run_id = fixture
            .db
            .get_interaction_request_run_id(&created.request.interaction_id)
            .unwrap();
        assert_eq!(
            fixture
                .db
                .cancel_interactions_for_stopped_run(&run_id)
                .unwrap(),
            1
        );
        assert_eq!(
            fixture.db.get_agent_task_run(&run_id).unwrap().status,
            "awaiting_user_input"
        );
        assert_eq!(
            fixture
                .db
                .get_interaction_request(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Cancelled
        );
    }

    #[test]
    fn intentionally_stopped_continuation_does_not_reenter_recovery_queue() {
        let fixture = fixture();
        let created = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-stop-after-answer"))
            .unwrap();
        let run_id = {
            let conn = fixture.db.conn();
            conn.query_row(
                "SELECT run_id FROM interaction_requests WHERE id = ?1",
                rusqlite::params![&created.request.interaction_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
        };
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["Repo".into()]);
        fixture
            .db
            .mark_interaction_presented(&created.request.interaction_id)
            .unwrap();
        fixture
            .db
            .submit_interaction_response(&SubmitInteractionResponse {
                interaction_id: created.request.interaction_id.clone(),
                resume_token: created.request.resume_token,
                answers,
            })
            .unwrap();
        fixture
            .db
            .acknowledge_interaction(&created.request.interaction_id)
            .unwrap();

        assert_eq!(
            fixture
                .db
                .cancel_interactions_for_stopped_run(&run_id)
                .unwrap(),
            1
        );
        assert_eq!(
            fixture
                .db
                .get_interaction_request(&created.request.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Cancelled
        );
        assert!(fixture
            .db
            .list_interaction_requests(Some(&fixture.conversation_id), false)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn multiple_interactions_resume_only_after_the_last_response() {
        let fixture = fixture();
        let first = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-first-gate"))
            .unwrap()
            .request;
        let second = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-second-gate"))
            .unwrap()
            .request;
        fixture
            .db
            .suspend_agent_turn_for_interaction(&first.interaction_id)
            .unwrap();
        fixture
            .db
            .mark_interaction_presented(&first.interaction_id)
            .unwrap();
        fixture
            .db
            .mark_interaction_presented(&second.interaction_id)
            .unwrap();

        let response_for = |request: &InteractionRequest, call_id: &str| {
            let mut answers = InteractionAnswers::new();
            answers.insert("scope".into(), vec!["Repo".into()]);
            let input = SubmitInteractionResponse {
                interaction_id: request.interaction_id.clone(),
                resume_token: request.resume_token.clone(),
                answers,
            };
            let message = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: fixture.conversation_id.clone(),
                role: Role::User,
                content: "Which scope?\nRepo".into(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                artifacts: Some(serde_json::json!({
                    "kind": "questionResponse",
                    "version": 2,
                    "interactionId": request.interaction_id.clone(),
                    "requestCallId": call_id,
                    "answers": [{ "id": "scope", "question": "Which scope?", "answers": ["Repo"] }],
                })),
                token_count: 4,
                created_at: String::new(),
                sort_order: 0,
                thinking: None,
                image_attachments: None,
            };
            (message, input)
        };

        let (first_message, first_input) = response_for(&first, "call-first-gate");
        let first_launch = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &first_message,
                None,
                None,
                "first-gate-launch",
                &first_input,
            )
            .unwrap();
        assert_eq!(first_launch.status, "awaiting_user_input");
        assert!(first_launch.reused);
        assert_eq!(
            fixture
                .db
                .get_agent_task_run(&first_launch.run_id)
                .unwrap()
                .status,
            "awaiting_user_input"
        );

        let (second_message, second_input) = response_for(&second, "call-second-gate");
        let final_launch = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &second_message,
                None,
                None,
                "second-gate-launch",
                &second_input,
            )
            .unwrap();
        assert_eq!(final_launch.status, "queued");
        assert!(!final_launch.reused);
        assert_eq!(
            fixture
                .db
                .acknowledge_submitted_interactions_for_run(&final_launch.run_id)
                .unwrap(),
            2
        );
        assert_eq!(
            fixture
                .db
                .get_interaction_request(&first.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Acknowledged
        );
        assert_eq!(
            fixture
                .db
                .get_interaction_request(&second.interaction_id)
                .unwrap()
                .status,
            InteractionStatus::Acknowledged
        );
    }

    #[test]
    fn separately_submitted_sibling_remains_a_barrier_until_its_message_is_launched() {
        let fixture = fixture();
        let first = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-outbox-first"))
            .unwrap()
            .request;
        let second = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-outbox-second"))
            .unwrap()
            .request;
        fixture
            .db
            .suspend_agent_turn_for_interaction(&first.interaction_id)
            .unwrap();
        fixture
            .db
            .mark_interaction_presented(&first.interaction_id)
            .unwrap();
        fixture
            .db
            .mark_interaction_presented(&second.interaction_id)
            .unwrap();

        let response_for = |request: &InteractionRequest, call_id: &str| {
            let mut answers = InteractionAnswers::new();
            answers.insert("scope".into(), vec!["Repo".into()]);
            let input = SubmitInteractionResponse {
                interaction_id: request.interaction_id.clone(),
                resume_token: request.resume_token.clone(),
                answers,
            };
            let message = ConversationMessage {
                id: Uuid::new_v4().to_string(),
                conversation_id: fixture.conversation_id.clone(),
                role: Role::User,
                content: "Which scope?\nRepo".into(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                artifacts: Some(serde_json::json!({
                    "kind": "questionResponse",
                    "version": 2,
                    "interactionId": request.interaction_id.clone(),
                    "requestCallId": call_id,
                    "answers": [{ "id": "scope", "question": "Which scope?", "answers": ["Repo"] }],
                })),
                token_count: 4,
                created_at: String::new(),
                sort_order: 0,
                thinking: None,
                image_attachments: None,
            };
            (message, input)
        };

        let (_, first_input) = response_for(&first, "call-outbox-first");
        fixture
            .db
            .submit_interaction_response(&first_input)
            .unwrap();
        let visible = fixture
            .db
            .list_interaction_requests(Some(&fixture.conversation_id), false)
            .unwrap();
        assert!(visible.iter().any(|request| {
            request.interaction_id == first.interaction_id
                && request.status == InteractionStatus::Submitted
        }));

        let (second_message, second_input) = response_for(&second, "call-outbox-second");
        let second_launch = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &second_message,
                None,
                None,
                "outbox-second-launch",
                &second_input,
            )
            .unwrap();
        assert_eq!(second_launch.status, "awaiting_user_input");
        assert!(second_launch.reused);

        let (first_message, _) = response_for(&first, "call-outbox-first");
        let final_launch = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &first_message,
                None,
                None,
                "outbox-first-launch",
                &first_input,
            )
            .unwrap();
        assert_eq!(final_launch.status, "queued");
        assert!(!final_launch.reused);
        assert_eq!(
            fixture
                .db
                .acknowledge_submitted_interactions_for_run(&final_launch.run_id)
                .unwrap(),
            2
        );
    }

    #[test]
    fn restored_interaction_response_requeues_once_per_restart_without_duplicate_message() {
        let fixture = fixture();
        let request = fixture
            .db
            .create_interaction_request(&request_input(&fixture, "call-recover-launch"))
            .unwrap()
            .request;
        fixture
            .db
            .suspend_agent_turn_for_interaction(&request.interaction_id)
            .unwrap();
        fixture
            .db
            .mark_interaction_presented(&request.interaction_id)
            .unwrap();
        let mut answers = InteractionAnswers::new();
        answers.insert("scope".into(), vec!["Repo".into()]);
        let input = SubmitInteractionResponse {
            interaction_id: request.interaction_id.clone(),
            resume_token: request.resume_token,
            answers,
        };
        let message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: fixture.conversation_id.clone(),
            role: Role::User,
            content: "Which scope?\nRepo".into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: Some(serde_json::json!({
                "kind": "questionResponse",
                "version": 2,
                "interactionId": request.interaction_id,
                "requestCallId": "call-recover-launch",
                "answers": [{ "id": "scope", "question": "Which scope?", "answers": ["Repo"] }],
            })),
            token_count: 4,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        let first_launch = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &message,
                None,
                None,
                "recover-launch",
                &input,
            )
            .unwrap();
        assert_eq!(first_launch.status, "queued");
        assert!(!first_launch.reused);
        let message_count = fixture
            .db
            .get_messages(&fixture.conversation_id)
            .unwrap()
            .len();
        {
            let conn = fixture.db.conn();
            conn.execute(
                "UPDATE conversation_turns
                 SET status = 'awaiting_user_input', finished_at = NULL
                 WHERE id = ?1",
                [&first_launch.turn_id],
            )
            .unwrap();
            conn.execute(
                "UPDATE agent_task_runs
                 SET status = 'awaiting_user_input', phase = 'awaiting_user_input',
                     finished_at = NULL
                 WHERE id = ?1",
                [&first_launch.run_id],
            )
            .unwrap();
        }

        let recovered = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &message,
                None,
                None,
                "recover-launch",
                &input,
            )
            .unwrap();
        assert_eq!(recovered.run_id, first_launch.run_id);
        assert_eq!(recovered.turn_id, first_launch.turn_id);
        assert_eq!(recovered.status, "queued");
        assert!(!recovered.reused);
        assert_eq!(recovered.user_message_id, first_launch.user_message_id);
        assert_eq!(
            fixture
                .db
                .get_messages(&fixture.conversation_id)
                .unwrap()
                .len(),
            message_count
        );

        let replayed = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &message,
                None,
                None,
                "recover-launch",
                &input,
            )
            .unwrap();
        assert_eq!(replayed.status, "queued");
        assert!(replayed.reused);
        assert_eq!(replayed.user_message_id, first_launch.user_message_id);

        let different_key = fixture.db.resume_agent_turn_with_interaction_response(
            &message,
            None,
            None,
            "different-recover-launch",
            &input,
        );
        assert!(matches!(different_key, Err(CoreError::InvalidInput(_))));

        assert_eq!(
            fixture
                .db
                .acknowledge_submitted_interactions_for_run(&recovered.run_id)
                .unwrap(),
            1
        );
        {
            let conn = fixture.db.conn();
            conn.execute(
                "UPDATE conversation_turns
                 SET status = 'awaiting_user_input', finished_at = NULL
                 WHERE id = ?1",
                [&recovered.turn_id],
            )
            .unwrap();
            conn.execute(
                "UPDATE agent_task_runs
                 SET status = 'awaiting_user_input', phase = 'awaiting_user_input',
                     finished_at = NULL
                 WHERE id = ?1",
                [&recovered.run_id],
            )
            .unwrap();
        }
        let recovered_after_acknowledgement = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &message,
                None,
                None,
                "recover-launch",
                &input,
            )
            .unwrap();
        assert_eq!(recovered_after_acknowledgement.status, "queued");
        assert!(!recovered_after_acknowledgement.reused);

        fixture
            .db
            .finalize_conversation_turn(
                &recovered_after_acknowledgement.turn_id,
                "error",
                None,
                None,
            )
            .unwrap();
        fixture
            .db
            .finish_agent_task_run(
                &recovered_after_acknowledgement.run_id,
                "failed",
                Some("Initialization failed"),
                Some("provider unavailable"),
                None,
            )
            .unwrap();
        let terminal_replay = fixture
            .db
            .resume_agent_turn_with_interaction_response(
                &message,
                None,
                None,
                "recover-launch",
                &input,
            )
            .unwrap();
        assert_eq!(terminal_replay.run_id, first_launch.run_id);
        assert_eq!(terminal_replay.status, "failed");
        assert!(terminal_replay.reused);
        assert_eq!(
            fixture
                .db
                .get_agent_task_run(&first_launch.run_id)
                .unwrap()
                .status,
            "failed"
        );

        let stale_waiting = crate::agent_run::AgentRunEvent::status_update(
            &terminal_replay.run_id,
            Some(&terminal_replay.turn_id),
            999,
            crate::agent_run::AgentRunPhase::AwaitingUserInput,
            "Waiting for your response",
            Some("awaiting_user_input"),
            None,
        );
        let run = crate::task_run::AgentTaskRuntime::new(&fixture.db)
            .apply_run_event(&terminal_replay.run_id, &stale_waiting)
            .unwrap();
        assert_eq!(run.status, "failed");
    }
}
