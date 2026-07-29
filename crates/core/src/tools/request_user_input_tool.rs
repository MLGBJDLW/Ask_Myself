//! Structured user-input request rendered as interactive question cards.

#[cfg(test)]
use crate::db::Database;

use std::collections::HashSet;
use std::sync::OnceLock;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

use super::{Tool, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/request_user_input.json");
const VALID_TYPES: [&str; 5] = ["short", "long", "single_choice", "multi_choice", "confirm"];

pub struct RequestUserInputTool;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestArgs {
    questions: Vec<Question>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Question {
    id: String,
    header: String,
    question: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    options: Vec<QuestionOption>,
    #[serde(default)]
    placeholder: Option<String>,
    #[serde(default)]
    why: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct QuestionOption {
    label: String,
    description: String,
}

fn validate(args: &RequestArgs) -> Result<(), CoreError> {
    if !(1..=3).contains(&args.questions.len()) {
        return Err(CoreError::InvalidInput(
            "request_user_input requires one to three questions".into(),
        ));
    }
    let mut ids = HashSet::new();
    for question in &args.questions {
        if question.id.trim().is_empty()
            || question.header.trim().is_empty()
            || question.question.trim().is_empty()
        {
            return Err(CoreError::InvalidInput(
                "Question id, header, and question must be non-empty".into(),
            ));
        }
        if !ids.insert(question.id.trim().to_lowercase()) {
            return Err(CoreError::InvalidInput(format!(
                "Duplicate question id: {}",
                question.id
            )));
        }
        if !VALID_TYPES.contains(&question.kind.as_str()) {
            return Err(CoreError::InvalidInput(format!(
                "Unsupported question type: {}",
                question.kind
            )));
        }
        let choice = matches!(question.kind.as_str(), "single_choice" | "multi_choice");
        if choice && !(2..=4).contains(&question.options.len()) {
            return Err(CoreError::InvalidInput(format!(
                "Choice question `{}` requires two to four options",
                question.id
            )));
        }
        if question
            .options
            .iter()
            .any(|option| option.label.trim().is_empty())
        {
            return Err(CoreError::InvalidInput(format!(
                "Question `{}` contains an empty option label",
                question.id
            )));
        }
    }
    Ok(())
}

#[async_trait]
impl Tool for RequestUserInputTool {
    fn name(&self) -> &str {
        "request_user_input"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn is_concurrency_safe(&self, _args: &serde_json::Value) -> bool {
        false
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db: _db,
            source_scope: _source_scope,
            ..
        } = context;
        let args: RequestArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid request_user_input arguments: {error}"))
        })?;
        validate(&args)?;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content:
                "The questions are displayed to the user. Stop now and wait for their next message."
                    .into(),
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "questionRequest",
                "version": 1,
                "callId": call_id,
                "status": "pending",
                "questions": args.questions,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_a_typed_question_request_artifact() {
        let db = Database::open_memory().unwrap();
        let result = RequestUserInputTool
            .execute(crate::tools::ToolExecutionContext::new("call-1", r#"{"questions":[{"id":"scope","header":"Scope","question":"Which scope?","type":"single_choice","options":[{"label":"App (Recommended)","description":"This app only."},{"label":"Repo","description":"The full repository."}]}]}"#, &db, &[]))
            .await
            .unwrap();
        assert_eq!(result.artifacts.unwrap()["kind"], "questionRequest");
    }
}
