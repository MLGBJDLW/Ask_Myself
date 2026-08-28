//! Canonical, invocation-level AI usage accounting and aggregate queries.

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{ProviderType, Usage};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UsageAnalyticsFilter {
    pub start_at: Option<String>,
    pub end_at: Option<String>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub operation_kind: Option<String>,
    pub time_bucket: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AiUsageRecordInput<'a> {
    pub invocation_id: &'a str,
    pub occurred_at: Option<&'a str>,
    pub provider_id: &'a str,
    pub provider_type: &'a str,
    pub model_id: &'a str,
    pub raw_model_id: Option<&'a str>,
    pub modality: &'a str,
    pub operation_kind: &'a str,
    pub conversation_id: Option<&'a str>,
    pub turn_id: Option<&'a str>,
    pub run_id: Option<&'a str>,
    pub subtask_run_id: Option<&'a str>,
    pub project_id: Option<&'a str>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub thinking_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_creation_tokens: u64,
    pub usage_source: &'a str,
    pub request_status: &'a str,
    pub latency_ms: Option<u64>,
    pub time_to_first_token_ms: Option<u64>,
    pub upstream_provider_id: Option<&'a str>,
    pub cache_outcome_reason: Option<&'a str>,
    pub estimated_cost_micros: Option<u64>,
    pub currency: Option<&'a str>,
    pub pricing_version: Option<&'a str>,
    pub provider_raw: &'a serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub request_count: u64,
    pub agent_run_count: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub thinking_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_hit_rate: Option<f64>,
    pub estimated_cost_micros: Option<u64>,
    pub currency: Option<String>,
    pub provider_reported_percent: f64,
    pub normalized_percent: f64,
    pub estimated_percent: f64,
    pub unknown_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageBreakdownRow {
    pub key: String,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub request_count: u64,
    pub agent_run_count: u64,
    pub turn_count: u64,
    pub success_count: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub thinking_tokens: u64,
    pub total_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_miss_tokens: u64,
    pub estimated_cost_micros: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageTimeSeriesPoint {
    pub date: String,
    pub request_count: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_miss_tokens: u64,
    pub cache_creation_tokens: u64,
    pub estimated_cost_micros: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageAnalytics {
    pub totals: UsageTotals,
    pub by_model: Vec<UsageBreakdownRow>,
    pub by_operation: Vec<UsageBreakdownRow>,
    pub time_series: Vec<UsageTimeSeriesPoint>,
}

pub(crate) fn record_ai_usage_on_connection(
    conn: &rusqlite::Connection,
    input: &AiUsageRecordInput<'_>,
) -> Result<bool, CoreError> {
    let provider_raw_json = serde_json::to_string(&crate::sensitive_data::sanitize_json_strings(
        input.provider_raw,
        None,
    ))?;
    let changed = conn.execute(
        "INSERT OR IGNORE INTO ai_usage_records (
                id, invocation_id, occurred_at, provider_id, provider_type,
                model_id, raw_model_id, modality, operation_kind,
                conversation_id, turn_id, run_id, subtask_run_id, project_id,
                prompt_tokens, completion_tokens, thinking_tokens, total_tokens,
                cache_read_tokens, cache_miss_tokens, cache_creation_tokens,
                usage_source, request_status, latency_ms, time_to_first_token_ms,
                upstream_provider_id, cache_outcome_reason,
                estimated_cost_micros, currency, pricing_version, provider_raw_json
             ) VALUES (
                ?1, ?2, COALESCE(?3, datetime('now')), ?4, ?5,
                ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                ?25, ?26, ?27, ?28, ?29, ?30, ?31
             )",
        params![
            Uuid::new_v4().to_string(),
            input.invocation_id,
            input.occurred_at,
            input.provider_id,
            input.provider_type,
            input.model_id,
            input.raw_model_id,
            input.modality,
            input.operation_kind,
            input.conversation_id,
            input.turn_id,
            input.run_id,
            input.subtask_run_id,
            input.project_id,
            to_i64(input.prompt_tokens),
            to_i64(input.completion_tokens),
            to_i64(input.thinking_tokens),
            to_i64(input.total_tokens),
            to_i64(input.cache_read_tokens),
            to_i64(input.cache_miss_tokens),
            to_i64(input.cache_creation_tokens),
            input.usage_source,
            input.request_status,
            input.latency_ms.map(to_i64),
            input.time_to_first_token_ms.map(to_i64),
            input.upstream_provider_id,
            input.cache_outcome_reason,
            input.estimated_cost_micros.map(to_i64),
            input.currency,
            input.pricing_version,
            provider_raw_json,
        ],
    )?;
    Ok(changed > 0)
}

impl Database {
    pub fn record_ai_usage(&self, input: &AiUsageRecordInput<'_>) -> Result<bool, CoreError> {
        let conn = self.conn();
        record_ai_usage_on_connection(&conn, input)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_model_step_usage(
        &self,
        conversation_id: Option<&str>,
        turn_id: Option<&str>,
        invocation_scope: Option<&str>,
        run_id_override: Option<&str>,
        subtask_run_id: Option<&str>,
        iteration: u32,
        provider_type: Option<ProviderType>,
        model: &str,
        operation_kind: &str,
        usage: Option<&Usage>,
        estimated_prompt_tokens: u32,
        normalized_cache_miss_tokens: Option<u32>,
        latency_ms: Option<u64>,
        time_to_first_token_ms: Option<u64>,
        cache_outcome_reason: Option<&str>,
    ) -> Result<bool, CoreError> {
        let run = turn_id
            .map(|turn_id| {
                let conn = self.conn();
                conn.query_row(
                    "SELECT id, provider, model FROM agent_task_runs
                     WHERE turn_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1",
                    [turn_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()
            })
            .transpose()?
            .flatten();
        let run_id = run_id_override.or_else(|| run.as_ref().map(|value| value.0.as_str()));
        let provider_id = run
            .as_ref()
            .and_then(|value| value.1.as_deref())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| provider_type_id(provider_type));
        let raw_model = run
            .as_ref()
            .and_then(|value| value.2.as_deref())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(model);
        let invocation_id = format!(
            "{}:{}:{}:{}",
            subtask_run_id
                .or(run_id)
                .or(invocation_scope)
                .or(turn_id)
                .unwrap_or("detached"),
            operation_kind,
            iteration,
            model
        );
        let (source, prompt, completion, thinking, total, cache_read, cache_creation, raw) =
            match usage {
                Some(usage) => (
                    "provider",
                    usage.prompt_tokens as u64,
                    usage.completion_tokens as u64,
                    usage.thinking_tokens.unwrap_or(0) as u64,
                    usage
                        .total_tokens
                        .max(usage.prompt_tokens.saturating_add(usage.completion_tokens))
                        as u64,
                    usage.cache_read_tokens.unwrap_or(0) as u64,
                    usage.cache_creation_tokens.unwrap_or(0) as u64,
                    usage
                        .provider_raw
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({ "usageCoverage": "notReported" })),
                ),
                None => (
                    "estimated",
                    estimated_prompt_tokens as u64,
                    0,
                    0,
                    estimated_prompt_tokens as u64,
                    0,
                    0,
                    serde_json::json!({
                        "usageCoverage": "notReported",
                        "estimatedPromptTokens": estimated_prompt_tokens
                    }),
                ),
            };
        let (estimated_cost_micros, currency, pricing_version) = usage_cost_metadata(provider_type);
        let upstream_provider_id = reported_upstream_provider(provider_type, usage);
        self.record_ai_usage(&AiUsageRecordInput {
            invocation_id: &invocation_id,
            occurred_at: None,
            provider_id,
            provider_type: provider_type_id(provider_type),
            model_id: raw_model,
            raw_model_id: Some(model),
            modality: "language_model",
            operation_kind,
            conversation_id,
            turn_id,
            run_id,
            subtask_run_id,
            project_id: None,
            prompt_tokens: prompt,
            completion_tokens: completion,
            thinking_tokens: thinking,
            total_tokens: total,
            cache_read_tokens: cache_read,
            cache_miss_tokens: normalized_cache_miss_tokens.unwrap_or(0) as u64,
            cache_creation_tokens: cache_creation,
            usage_source: source,
            request_status: "success",
            latency_ms,
            time_to_first_token_ms,
            upstream_provider_id,
            cache_outcome_reason,
            estimated_cost_micros,
            currency,
            pricing_version,
            provider_raw: &raw,
        })
    }

    pub fn get_usage_analytics(
        &self,
        filter: &UsageAnalyticsFilter,
    ) -> Result<UsageAnalytics, CoreError> {
        let conn = self.conn();
        let where_clause = usage_where_clause();
        let mut totals_stmt = conn.prepare(&format!(
            "SELECT COUNT(*), COUNT(DISTINCT run_id),
                    COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                    COALESCE(SUM(thinking_tokens), 0), COALESCE(SUM(total_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_miss_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    CASE WHEN COUNT(estimated_cost_micros) = 0 THEN NULL ELSE SUM(estimated_cost_micros) END,
                    MIN(currency),
                    COALESCE(SUM(CASE WHEN usage_source = 'provider' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN usage_source = 'normalized' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN usage_source = 'estimated' THEN 1 ELSE 0 END), 0)
             FROM ai_usage_records WHERE {where_clause}"
        ))?;
        let raw = totals_stmt.query_row(filter_params(filter), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
                row.get::<_, i64>(13)?,
            ))
        })?;
        let requests = nonnegative(raw.0);
        let percent = |count: i64| {
            if requests == 0 {
                0.0
            } else {
                nonnegative(count) as f64 * 100.0 / requests as f64
            }
        };
        let cache_denom = nonnegative(raw.6).saturating_add(nonnegative(raw.7));
        let totals = UsageTotals {
            request_count: requests,
            agent_run_count: nonnegative(raw.1),
            prompt_tokens: nonnegative(raw.2),
            completion_tokens: nonnegative(raw.3),
            thinking_tokens: nonnegative(raw.4),
            total_tokens: nonnegative(raw.5),
            cache_read_tokens: nonnegative(raw.6),
            cache_miss_tokens: nonnegative(raw.7),
            cache_creation_tokens: nonnegative(raw.8),
            cache_hit_rate: (cache_denom > 0)
                .then(|| nonnegative(raw.6) as f64 * 100.0 / cache_denom as f64),
            estimated_cost_micros: raw.9.map(nonnegative),
            currency: raw.10,
            provider_reported_percent: percent(raw.11),
            normalized_percent: percent(raw.12),
            estimated_percent: percent(raw.13),
            unknown_percent: percent(raw.0 - raw.11 - raw.12 - raw.13),
        };
        let by_model = query_breakdown(&conn, filter, "provider_id || ' / ' || model_id", true)?;
        let by_operation = query_breakdown(&conn, filter, "operation_kind", false)?;
        let time_series = query_time_series(&conn, filter)?;
        Ok(UsageAnalytics {
            totals,
            by_model,
            by_operation,
            time_series,
        })
    }

    pub fn delete_usage_records(&self, filter: &UsageAnalyticsFilter) -> Result<u64, CoreError> {
        let conn = self.conn();
        let changed = conn.execute(
            &format!(
                "DELETE FROM ai_usage_records WHERE {}",
                usage_where_clause()
            ),
            filter_params(filter),
        )?;
        Ok(changed as u64)
    }
}

fn reported_upstream_provider(
    provider_type: Option<ProviderType>,
    usage: Option<&Usage>,
) -> Option<&str> {
    usage
        .and_then(|usage| usage.provider_raw.as_ref())
        .and_then(|raw| {
            ["upstream_provider", "provider_name", "provider"]
                .into_iter()
                .find_map(|key| raw.get(key).and_then(serde_json::Value::as_str))
                .or_else(|| {
                    raw.get("endpoint")
                        .and_then(|endpoint| endpoint.get("provider_name"))
                        .and_then(serde_json::Value::as_str)
                })
                .or_else(|| {
                    raw.get("openrouterMetadata")
                        .or_else(|| raw.get("openrouter_metadata"))
                        .and_then(|metadata| metadata.get("endpoints"))
                        .and_then(|endpoints| endpoints.get("available"))
                        .and_then(serde_json::Value::as_array)
                        .and_then(|endpoints| {
                            endpoints.iter().find(|endpoint| {
                                endpoint
                                    .get("selected")
                                    .and_then(serde_json::Value::as_bool)
                                    == Some(true)
                            })
                        })
                        .and_then(|endpoint| endpoint.get("provider"))
                        .and_then(serde_json::Value::as_str)
                })
        })
        .or_else(|| match provider_type {
            Some(ProviderType::OpenRouter | ProviderType::Custom) | None => None,
            _ => Some(provider_type_id(provider_type)),
        })
}

fn query_breakdown(
    conn: &rusqlite::Connection,
    filter: &UsageAnalyticsFilter,
    key_expression: &str,
    include_model: bool,
) -> Result<Vec<UsageBreakdownRow>, CoreError> {
    let select_identity = if include_model {
        "provider_id, model_id"
    } else {
        "NULL, NULL"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {key_expression}, {select_identity}, COUNT(*), COUNT(DISTINCT run_id),
                COUNT(DISTINCT turn_id),
                SUM(CASE WHEN request_status = 'success' THEN 1 ELSE 0 END),
                COALESCE(SUM(prompt_tokens), 0), COALESCE(SUM(completion_tokens), 0),
                COALESCE(SUM(thinking_tokens), 0), COALESCE(SUM(total_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_miss_tokens), 0),
                CASE WHEN COUNT(estimated_cost_micros) = 0 THEN NULL ELSE SUM(estimated_cost_micros) END
         FROM ai_usage_records WHERE {} GROUP BY {key_expression}
         ORDER BY SUM(total_tokens) DESC, COUNT(*) DESC",
        usage_where_clause()
    ))?;
    let rows = stmt
        .query_map(filter_params(filter), |row| {
            Ok(UsageBreakdownRow {
                key: row.get(0)?,
                provider_id: row.get(1)?,
                model_id: row.get(2)?,
                request_count: nonnegative(row.get(3)?),
                agent_run_count: nonnegative(row.get(4)?),
                turn_count: nonnegative(row.get(5)?),
                success_count: nonnegative(row.get(6)?),
                prompt_tokens: nonnegative(row.get(7)?),
                completion_tokens: nonnegative(row.get(8)?),
                thinking_tokens: nonnegative(row.get(9)?),
                total_tokens: nonnegative(row.get(10)?),
                cache_read_tokens: nonnegative(row.get(11)?),
                cache_miss_tokens: nonnegative(row.get(12)?),
                estimated_cost_micros: row.get::<_, Option<i64>>(13)?.map(nonnegative),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn query_time_series(
    conn: &rusqlite::Connection,
    filter: &UsageAnalyticsFilter,
) -> Result<Vec<UsageTimeSeriesPoint>, CoreError> {
    let bucket_expression = match filter.time_bucket.as_deref() {
        Some("week") => "strftime('%Y-W%W', occurred_at)",
        Some("month") => "strftime('%Y-%m', occurred_at)",
        _ => "date(occurred_at)",
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {bucket_expression}, COUNT(*), COALESCE(SUM(prompt_tokens), 0),
                COALESCE(SUM(completion_tokens), 0), COALESCE(SUM(thinking_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_miss_tokens), 0),
                COALESCE(SUM(cache_creation_tokens), 0),
                CASE WHEN COUNT(estimated_cost_micros) = 0 THEN NULL ELSE SUM(estimated_cost_micros) END
         FROM ai_usage_records WHERE {} GROUP BY {bucket_expression} ORDER BY {bucket_expression}",
        usage_where_clause()
    ))?;
    let rows = stmt
        .query_map(filter_params(filter), |row| {
            Ok(UsageTimeSeriesPoint {
                date: row.get(0)?,
                request_count: nonnegative(row.get(1)?),
                prompt_tokens: nonnegative(row.get(2)?),
                completion_tokens: nonnegative(row.get(3)?),
                thinking_tokens: nonnegative(row.get(4)?),
                cache_read_tokens: nonnegative(row.get(5)?),
                cache_miss_tokens: nonnegative(row.get(6)?),
                cache_creation_tokens: nonnegative(row.get(7)?),
                estimated_cost_micros: row.get::<_, Option<i64>>(8)?.map(nonnegative),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn usage_where_clause() -> &'static str {
    "(?1 IS NULL OR datetime(occurred_at) >= datetime(?1))
     AND (?2 IS NULL OR datetime(occurred_at) < datetime(?2))
     AND (?3 IS NULL OR provider_id = ?3)
     AND (?4 IS NULL OR model_id = ?4)
     AND (?5 IS NULL OR operation_kind = ?5)"
}

fn filter_params(filter: &UsageAnalyticsFilter) -> [&dyn rusqlite::ToSql; 5] {
    [
        &filter.start_at,
        &filter.end_at,
        &filter.provider_id,
        &filter.model_id,
        &filter.operation_kind,
    ]
}

pub fn provider_type_id(provider_type: Option<ProviderType>) -> &'static str {
    match provider_type {
        Some(ProviderType::OpenAi) => "open_ai",
        Some(ProviderType::OpenRouter) => "open_router",
        Some(ProviderType::Anthropic) => "anthropic",
        Some(ProviderType::Google) => "google",
        Some(ProviderType::DeepSeek) => "deep_seek",
        Some(ProviderType::Ollama) => "ollama",
        Some(ProviderType::LmStudio) => "lm_studio",
        Some(ProviderType::AzureOpenAi) => "azure_open_ai",
        Some(ProviderType::Zhipu) => "zhipu",
        Some(ProviderType::Moonshot) => "moonshot",
        Some(ProviderType::Qwen) => "qwen",
        Some(ProviderType::AlibabaModelStudio) => "alibaba_model_studio",
        Some(ProviderType::SiliconFlow) => "silicon_flow",
        Some(ProviderType::Doubao) => "doubao",
        Some(ProviderType::Yi) => "yi",
        Some(ProviderType::Baichuan) => "baichuan",
        Some(ProviderType::Custom) => "custom",
        None => "unknown",
    }
}

/// Local providers do not charge an API fee. Remote models remain unknown
/// until a versioned price is configured; the ledger never guesses a price.
pub fn usage_cost_metadata(
    provider_type: Option<ProviderType>,
) -> (Option<u64>, Option<&'static str>, Option<&'static str>) {
    match provider_type {
        Some(ProviderType::Ollama | ProviderType::LmStudio) => {
            (Some(0), Some("USD"), Some("local-api-v1"))
        }
        _ => (None, None, None),
    }
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}
fn nonnegative(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(invocation_id: &'a str, raw: &'a serde_json::Value) -> AiUsageRecordInput<'a> {
        AiUsageRecordInput {
            invocation_id,
            occurred_at: Some("2026-07-29T10:00:00Z"),
            provider_id: "open_ai",
            provider_type: "open_ai",
            model_id: "gpt-test",
            raw_model_id: None,
            modality: "language_model",
            operation_kind: "agent_main",
            conversation_id: None,
            turn_id: None,
            run_id: None,
            subtask_run_id: None,
            project_id: None,
            prompt_tokens: 100,
            completion_tokens: 20,
            thinking_tokens: 5,
            total_tokens: 120,
            cache_read_tokens: 80,
            cache_miss_tokens: 20,
            cache_creation_tokens: 0,
            usage_source: "provider",
            request_status: "success",
            latency_ms: Some(250),
            time_to_first_token_ms: Some(80),
            upstream_provider_id: Some("openai"),
            cache_outcome_reason: Some("hit_reported"),
            estimated_cost_micros: None,
            currency: None,
            pricing_version: None,
            provider_raw: raw,
        }
    }

    #[test]
    fn invocation_id_makes_recording_idempotent() {
        let db = Database::open_memory().unwrap();
        let raw = serde_json::json!({"promptTokens": 100});
        assert!(db.record_ai_usage(&input("inv-1", &raw)).unwrap());
        assert!(!db.record_ai_usage(&input("inv-1", &raw)).unwrap());
        assert_eq!(
            db.get_usage_analytics(&UsageAnalyticsFilter::default())
                .unwrap()
                .totals
                .request_count,
            1
        );
    }

    #[test]
    fn provider_raw_diagnostics_are_redacted_before_insert() {
        let db = Database::open_memory().unwrap();
        let raw = serde_json::json!({
            "requestUrl": "https://example.test/generate?key=AIza0123456789abcdefghijklmnopqrst",
            "authorization": "Authorization: Bearer provider-secret",
            "promptTokens": 100
        });
        db.record_ai_usage(&input("inv-redacted", &raw)).unwrap();

        let stored: String = db
            .conn()
            .query_row(
                "SELECT provider_raw_json FROM ai_usage_records WHERE invocation_id = 'inv-redacted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.contains("AIza"));
        assert!(!stored.contains("provider-secret"));
        assert!(!stored.to_ascii_lowercase().contains("?key="));
        assert!(stored.contains("promptTokens"));
    }

    #[test]
    fn analytics_separates_token_and_request_dimensions() {
        let db = Database::open_memory().unwrap();
        let raw = serde_json::json!({});
        db.record_ai_usage(&input("inv-1", &raw)).unwrap();
        let mut second = input("inv-2", &raw);
        second.model_id = "gpt-small";
        second.prompt_tokens = 10;
        second.total_tokens = 30;
        db.record_ai_usage(&second).unwrap();
        let analytics = db
            .get_usage_analytics(&UsageAnalyticsFilter::default())
            .unwrap();
        assert_eq!(analytics.totals.request_count, 2);
        assert_eq!(analytics.totals.total_tokens, 150);
        assert_eq!(analytics.by_model.len(), 2);
        assert_eq!(analytics.totals.cache_hit_rate, Some(80.0));
    }

    #[test]
    fn iso_filters_include_sqlite_timestamp_records() {
        let db = Database::open_memory().unwrap();
        let raw = serde_json::json!({});
        let mut record = input("inv-sqlite-time", &raw);
        record.occurred_at = Some("2026-07-29 10:00:00");
        db.record_ai_usage(&record).unwrap();

        let analytics = db
            .get_usage_analytics(&UsageAnalyticsFilter {
                start_at: Some("2026-07-29T00:00:00.000Z".into()),
                end_at: Some("2026-07-30T00:00:00.000Z".into()),
                ..UsageAnalyticsFilter::default()
            })
            .unwrap();

        assert_eq!(analytics.totals.request_count, 1);
    }

    #[test]
    fn detached_executor_scopes_are_unique_and_local_cost_is_zero() {
        let db = Database::open_memory().unwrap();
        let record = |scope| {
            db.record_model_step_usage(
                None,
                None,
                Some(scope),
                None,
                None,
                0,
                Some(ProviderType::Ollama),
                "llama-test",
                "subagent",
                None,
                42,
                None,
                Some(250),
                Some(80),
                Some("usage_schema_unknown"),
            )
            .unwrap()
        };

        assert!(record("worker-a"));
        assert!(!record("worker-a"));
        assert!(record("worker-b"));

        let totals = db
            .get_usage_analytics(&UsageAnalyticsFilter::default())
            .unwrap()
            .totals;
        assert_eq!(totals.request_count, 2);
        assert_eq!(totals.estimated_cost_micros, Some(0));
        assert_eq!(totals.currency.as_deref(), Some("USD"));
    }

    #[test]
    fn openrouter_upstream_provider_uses_selected_router_metadata_endpoint() {
        let usage = Usage {
            provider_raw: Some(serde_json::json!({
                "usage": {"prompt_tokens": 1, "completion_tokens": 1},
                "openrouterMetadata": {
                    "endpoints": {
                        "available": [
                            {"provider": "Fallback", "selected": false},
                            {"provider": "DeepInfra", "selected": true}
                        ]
                    }
                }
            })),
            ..Usage::default()
        };

        assert_eq!(
            reported_upstream_provider(Some(ProviderType::OpenRouter), Some(&usage)),
            Some("DeepInfra")
        );
    }

    #[test]
    fn openrouter_cache_hits_tolerate_missing_router_metadata() {
        let usage = Usage {
            provider_raw: Some(serde_json::json!({
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            })),
            ..Usage::default()
        };

        assert_eq!(
            reported_upstream_provider(Some(ProviderType::OpenRouter), Some(&usage)),
            None
        );
    }
}
