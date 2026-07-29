//! Durable usage projections derived from canonical Run Events.

use crate::agent::context::ContextUsageBreakdown;
use crate::agent_run::{AgentRunEvent, AgentRunEventKind};
use crate::db::Database;
use crate::error::CoreError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageSnapshotSource {
    Provider,
    Normalized,
    Estimated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub source: UsageSnapshotSource,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_creation_tokens: u64,
    pub last_prompt_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_breakdown: Option<ContextUsageBreakdown>,
    pub provider_raw: serde_json::Value,
}

impl Database {
    pub fn get_run_usage_snapshot(&self, run_id: &str) -> Result<Option<UsageSnapshot>, CoreError> {
        let events = self.list_agent_run_events(run_id)?;
        Ok(run_usage_snapshot(&events))
    }

    pub fn get_conversation_usage_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<Option<UsageSnapshot>, CoreError> {
        let run_ids = {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT id FROM agent_task_runs
                 WHERE conversation_id = ?1
                 ORDER BY created_at ASC, id ASC",
            )?;
            let rows = stmt
                .query_map([conversation_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };

        let mut snapshots = Vec::new();
        for run_id in run_ids {
            if let Some(snapshot) = self.get_run_usage_snapshot(&run_id)? {
                snapshots.push((run_id, snapshot));
            }
        }
        Ok(conversation_usage_snapshot(&snapshots))
    }
}

fn run_usage_snapshot(events: &[AgentRunEvent]) -> Option<UsageSnapshot> {
    events.iter().rev().find_map(|event| {
        if !matches!(
            event.kind,
            AgentRunEventKind::UsageUpdated | AgentRunEventKind::Done
        ) {
            return None;
        }

        let raw = event.payload.get("usageTotal")?.clone();
        let prompt_tokens = json_u64(&raw, "promptTokens");
        let completion_tokens = json_u64(&raw, "completionTokens");
        let total_tokens = json_u64(&raw, "totalTokens").max(prompt_tokens + completion_tokens);
        let last_prompt_tokens = event
            .payload
            .get("lastPromptTokens")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(prompt_tokens);
        let context_breakdown = event
            .payload
            .get("contextBreakdown")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok());
        let source = if prompt_tokens + completion_tokens > 0 {
            UsageSnapshotSource::Provider
        } else if last_prompt_tokens > 0 {
            UsageSnapshotSource::Estimated
        } else {
            UsageSnapshotSource::Normalized
        };

        Some(UsageSnapshot {
            source,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            thinking_tokens: json_u64(&raw, "thinkingTokens"),
            cache_read_tokens: json_u64(&raw, "cacheReadTokens"),
            cache_miss_tokens: json_u64(&raw, "cacheMissTokens"),
            cache_creation_tokens: json_u64(&raw, "cacheCreationTokens"),
            last_prompt_tokens,
            context_breakdown,
            provider_raw: raw,
        })
    })
}

fn conversation_usage_snapshot(snapshots: &[(String, UsageSnapshot)]) -> Option<UsageSnapshot> {
    let (_, latest) = snapshots.last()?;
    let mut aggregate = UsageSnapshot {
        source: UsageSnapshotSource::Provider,
        prompt_tokens: 0,
        completion_tokens: 0,
        total_tokens: 0,
        thinking_tokens: 0,
        cache_read_tokens: 0,
        cache_miss_tokens: 0,
        cache_creation_tokens: 0,
        last_prompt_tokens: latest.last_prompt_tokens,
        context_breakdown: latest.context_breakdown.clone(),
        provider_raw: serde_json::json!({
            "runs": snapshots
                .iter()
                .map(|(run_id, snapshot)| serde_json::json!({
                    "runId": run_id,
                    "usage": snapshot.provider_raw,
                }))
                .collect::<Vec<_>>(),
        }),
    };

    for (_, snapshot) in snapshots {
        aggregate.prompt_tokens += snapshot.prompt_tokens;
        aggregate.completion_tokens += snapshot.completion_tokens;
        aggregate.total_tokens += snapshot.total_tokens;
        aggregate.thinking_tokens += snapshot.thinking_tokens;
        aggregate.cache_read_tokens += snapshot.cache_read_tokens;
        aggregate.cache_miss_tokens += snapshot.cache_miss_tokens;
        aggregate.cache_creation_tokens += snapshot.cache_creation_tokens;
        aggregate.source = merge_source(aggregate.source, snapshot.source);
    }
    Some(aggregate)
}

fn merge_source(left: UsageSnapshotSource, right: UsageSnapshotSource) -> UsageSnapshotSource {
    match (left, right) {
        (UsageSnapshotSource::Estimated, _) | (_, UsageSnapshotSource::Estimated) => {
            UsageSnapshotSource::Estimated
        }
        (UsageSnapshotSource::Normalized, _) | (_, UsageSnapshotSource::Normalized) => {
            UsageSnapshotSource::Normalized
        }
        _ => UsageSnapshotSource::Provider,
    }
}

fn json_u64(value: &serde_json::Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_run::{
        AgentRunDisplayKind, AgentRunEventImportance, AgentRunEventPersistence,
    };

    fn usage_event(seq: u64, prompt_tokens: u64, completion_tokens: u64) -> AgentRunEvent {
        AgentRunEvent {
            version: crate::agent_run::AGENT_RUN_EVENT_VERSION,
            run_id: "run-usage".to_string(),
            turn_id: "turn-usage".to_string(),
            event_seq: seq,
            kind: AgentRunEventKind::UsageUpdated,
            phase: crate::agent_run::AgentRunPhase::Accounting,
            visibility: crate::agent_run::AgentRunEventVisibility::Developer,
            persistence: AgentRunEventPersistence::Durable,
            display_kind: AgentRunDisplayKind::Usage,
            importance: AgentRunEventImportance::Low,
            label: "Token usage updated".to_string(),
            status: Some("running".to_string()),
            payload: serde_json::json!({
                "type": "usageUpdate",
                "usageTotal": {
                    "promptTokens": prompt_tokens,
                    "completionTokens": completion_tokens,
                    "totalTokens": prompt_tokens + completion_tokens,
                    "cacheReadTokens": 10,
                },
                "lastPromptTokens": prompt_tokens,
                "contextBreakdown": {
                    "totalTokens": prompt_tokens,
                    "segments": [{ "kind": "conversation", "tokens": prompt_tokens }],
                },
            }),
            created_at: None,
        }
    }

    #[test]
    fn run_snapshot_uses_the_latest_cumulative_usage_event() {
        let snapshot = run_usage_snapshot(&[usage_event(1, 100, 20), usage_event(2, 180, 30)])
            .expect("usage snapshot");
        assert_eq!(snapshot.source, UsageSnapshotSource::Provider);
        assert_eq!(snapshot.prompt_tokens, 180);
        assert_eq!(snapshot.completion_tokens, 30);
        assert_eq!(snapshot.last_prompt_tokens, 180);
        assert_eq!(snapshot.cache_read_tokens, 10);
        assert_eq!(snapshot.context_breakdown.unwrap().total_tokens, 180);
    }

    #[test]
    fn conversation_snapshot_sums_each_run_once_and_keeps_latest_context() {
        let first = run_usage_snapshot(&[usage_event(1, 100, 20)]).unwrap();
        let second = run_usage_snapshot(&[usage_event(2, 50, 10)]).unwrap();
        let snapshot = conversation_usage_snapshot(&[
            ("run-1".to_string(), first),
            ("run-2".to_string(), second),
        ])
        .unwrap();
        assert_eq!(snapshot.prompt_tokens, 150);
        assert_eq!(snapshot.completion_tokens, 30);
        assert_eq!(snapshot.last_prompt_tokens, 50);
        assert_eq!(snapshot.context_breakdown.unwrap().total_tokens, 50);
        assert_eq!(snapshot.provider_raw["runs"].as_array().unwrap().len(), 2);
    }
}
