use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::error::CoreError;

use super::{Tool, ToolCategory, ToolExecutionContext, ToolResult};

const DEFAULT_WAIT_UP_TO_MS: u64 = 2_500;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObserveArgs {
    activity_id: String,
    #[serde(default)]
    after_seq: u64,
    #[serde(default = "default_wait_up_to_ms")]
    wait_up_to_ms: u64,
}

fn default_wait_up_to_ms() -> u64 {
    DEFAULT_WAIT_UP_TO_MS
}

pub struct ActivityObserveTool;

#[async_trait]
impl Tool for ActivityObserveTool {
    fn name(&self) -> &str {
        "activity_observe"
    }

    fn description(&self) -> &str {
        "Incrementally observe a running process, terminal command, browser wait, or desktop activity. Pass the last cursor as afterSeq to receive only newer events. The call returns immediately on new output or a state change and never waits longer than 2.5 seconds."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "activityId": {
                    "type": "string",
                    "description": "Activity identifier returned by a runtime-backed tool."
                },
                "afterSeq": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Return only events whose sequence is greater than this cursor."
                },
                "waitUpToMs": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 2500,
                    "default": DEFAULT_WAIT_UP_TO_MS,
                    "description": "Bounded long-poll budget. Runtime clamps larger values to 2500ms."
                }
            },
            "required": ["activityId"],
            "additionalProperties": false
        })
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[
            ToolCategory::Core,
            ToolCategory::Process,
            ToolCategory::Terminal,
            ToolCategory::BrowserInteract,
            ToolCategory::DesktopInteract,
        ]
    }

    async fn execute(&self, context: ToolExecutionContext<'_>) -> Result<ToolResult, CoreError> {
        let ToolExecutionContext {
            call_id,
            arguments,
            activity_runtime,
            ..
        } = context;
        let args: ObserveArgs = serde_json::from_str(arguments).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid activity_observe arguments: {error}"))
        })?;
        let activity_id = args.activity_id.trim();
        if activity_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "activityId cannot be empty".to_string(),
            ));
        }
        let runtime = activity_runtime.ok_or_else(|| {
            CoreError::Internal("Activity Runtime is unavailable for this tool call".to_string())
        })?;
        let observation = runtime
            .observe(
                activity_id,
                args.after_seq,
                Duration::from_millis(args.wait_up_to_ms),
            )
            .await?;
        let content = serde_json::to_string_pretty(&observation)?;
        Ok(ToolResult {
            call_id: call_id.to_string(),
            content,
            is_error: false,
            artifacts: Some(serde_json::json!({
                "kind": "activityObservation",
                "activity": observation,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{ActivityEventKind, ActivityRuntime, ActivitySpec, ActivitySurface};
    use crate::db::Database;

    #[tokio::test]
    async fn observe_tool_returns_only_events_after_cursor() {
        let db = Database::open_memory().unwrap();
        let runtime = ActivityRuntime::new();
        let activity = runtime
            .start(ActivitySpec::new(ActivitySurface::Process, "run_shell"))
            .unwrap();
        runtime
            .append(
                &activity.activity_id,
                ActivityEventKind::StdoutChunk,
                serde_json::json!({ "data": "hello" }),
            )
            .unwrap();
        let arguments = serde_json::json!({
            "activityId": activity.activity_id,
            "afterSeq": 1,
            "waitUpToMs": 0,
        })
        .to_string();

        let result = ActivityObserveTool
            .execute(
                ToolExecutionContext::new("observe-1", &arguments, &db, &[])
                    .with_activity_runtime(&runtime),
            )
            .await
            .unwrap();
        let observation = &result.artifacts.unwrap()["activity"];
        assert_eq!(observation["events"].as_array().unwrap().len(), 1);
        assert_eq!(observation["events"][0]["seq"], 2);
    }
}
