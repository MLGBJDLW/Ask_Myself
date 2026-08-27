//! Timezone-aware Cron5 recurrence for durable workflow automations.
//!
//! Cron syntax, timezone validation, next-occurrence calculation, and UI
//! preview deliberately cross one interface. Callers never append timezones or
//! reinterpret day-of-month/day-of-week rules themselves.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use cronexpr::{Crontab, MakeTimestamp};
use serde::{Deserialize, Serialize};

use crate::agent::power_mode::AgentPowerMode;
use crate::agent::AgentExecutionMode;
use crate::error::CoreError;
use crate::mixture_of_agents::AgentCollaborationMode;
use crate::quality_profile::OrchestrationProfile;

pub const WORKFLOW_CRON_SCHEDULE_VERSION: u16 = 2;
pub const DEFAULT_WORKFLOW_TIMEZONE: &str = "UTC";
pub const DEFAULT_MISFIRE_GRACE_SECONDS: u32 = 300;
const MAX_PREVIEW_OCCURRENCES: usize = 20;
const MAX_SPARSE_SEARCH_WINDOWS: usize = 25;
const SPARSE_SEARCH_ADVANCE_DAYS: i64 = 1_400;

fn default_schedule_version() -> u16 {
    WORKFLOW_CRON_SCHEDULE_VERSION
}

fn default_timezone() -> String {
    DEFAULT_WORKFLOW_TIMEZONE.to_string()
}

fn default_misfire_grace_seconds() -> u32 {
    DEFAULT_MISFIRE_GRACE_SECONDS
}

fn default_power_mode() -> String {
    "standard".to_string()
}

fn default_orchestration_profile() -> String {
    "balanced".to_string()
}

fn default_collaboration_mode() -> String {
    "direct".to_string()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowScheduleMisfirePolicy {
    #[default]
    RunLatest,
    Skip,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowScheduleOverlapPolicy {
    #[default]
    Skip,
    Allow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationExecutionPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_config_id: Option<String>,
    /// Provider snapshot used to fail closed if a saved agent config changes route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_endpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// `None` means use the selected provider/model route default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default = "default_power_mode")]
    pub power_mode: String,
    #[serde(default = "default_orchestration_profile")]
    pub orchestration_profile: String,
    #[serde(default = "default_collaboration_mode")]
    pub collaboration_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<String>,
}

impl Default for WorkflowAutomationExecutionPolicy {
    fn default() -> Self {
        Self {
            agent_config_id: None,
            provider: None,
            provider_endpoint_id: None,
            model: None,
            context_window: None,
            power_mode: default_power_mode(),
            orchestration_profile: default_orchestration_profile(),
            collaboration_mode: default_collaboration_mode(),
            execution_mode: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAutomationScheduleConfig {
    #[serde(default = "default_schedule_version")]
    pub version: u16,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default)]
    pub misfire_policy: WorkflowScheduleMisfirePolicy,
    #[serde(default = "default_misfire_grace_seconds")]
    pub misfire_grace_seconds: u32,
    #[serde(default)]
    pub overlap_policy: WorkflowScheduleOverlapPolicy,
    #[serde(default)]
    pub execution_policy: WorkflowAutomationExecutionPolicy,
    /// Migration-only flag. Legacy jobs are disabled until saved as v2.
    #[serde(default)]
    pub legacy_needs_review: bool,
}

impl Default for WorkflowAutomationScheduleConfig {
    fn default() -> Self {
        Self {
            version: WORKFLOW_CRON_SCHEDULE_VERSION,
            timezone: default_timezone(),
            misfire_policy: WorkflowScheduleMisfirePolicy::RunLatest,
            misfire_grace_seconds: default_misfire_grace_seconds(),
            overlap_policy: WorkflowScheduleOverlapPolicy::Skip,
            execution_policy: WorkflowAutomationExecutionPolicy::default(),
            legacy_needs_review: false,
        }
    }
}

impl WorkflowAutomationScheduleConfig {
    pub fn legacy_utc_needs_review() -> Self {
        Self {
            version: 1,
            timezone: default_timezone(),
            legacy_needs_review: true,
            ..Self::default()
        }
    }

    pub fn validate_for_save(&self, cron: &str) -> Result<(), CoreError> {
        if self.version != WORKFLOW_CRON_SCHEDULE_VERSION {
            return Err(CoreError::InvalidInput(format!(
                "Workflow schedule version {} must be reviewed and saved as version {WORKFLOW_CRON_SCHEDULE_VERSION}",
                self.version
            )));
        }
        if self.legacy_needs_review {
            return Err(CoreError::InvalidInput(
                "Legacy workflow schedule must be reviewed before it can be enabled".into(),
            ));
        }
        if self.misfire_grace_seconds > 604_800 {
            return Err(CoreError::InvalidInput(
                "Workflow schedule misfire grace cannot exceed 7 days".into(),
            ));
        }
        AgentPowerMode::from_wire(Some(&self.execution_policy.power_mode))
            .map_err(CoreError::InvalidInput)?;
        AgentCollaborationMode::from_wire(Some(&self.execution_policy.collaboration_mode))
            .map_err(CoreError::InvalidInput)?;
        OrchestrationProfile::from_wire(Some(&self.execution_policy.orchestration_profile))
            .map_err(CoreError::InvalidInput)?;
        if let Some(execution_mode) = self.execution_policy.execution_mode.as_deref() {
            AgentExecutionMode::from_wire(Some(execution_mode)).map_err(CoreError::InvalidInput)?;
        }
        if self.execution_policy.context_window == Some(0) {
            return Err(CoreError::InvalidInput(
                "Scheduled workflow context window must be greater than zero or Auto".into(),
            ));
        }
        validate_workflow_cron_schedule(cron, &self.timezone)
    }
}

/// Classifies the legacy `{ kind: "schedule", cron }` contract without
/// silently granting semantics that the former daily-UTC scheduler ignored.
/// Callers may persist the returned v2 config only when
/// `legacy_needs_review == false`; every other expression must remain paused or
/// be rejected until a user reviews it in the advanced scheduler UI.
pub fn legacy_workflow_schedule_config(cron: &str) -> WorkflowAutomationScheduleConfig {
    let fields = cron.split_whitespace().collect::<Vec<_>>();
    let is_daily_utc = fields.len() == 5
        && fields[2..] == ["*", "*", "*"]
        && fields[0]
            .parse::<u32>()
            .ok()
            .is_some_and(|minute| minute <= 59)
        && fields[1].parse::<u32>().ok().is_some_and(|hour| hour <= 23)
        && validate_workflow_cron_schedule(cron, DEFAULT_WORKFLOW_TIMEZONE).is_ok();
    if is_daily_utc {
        WorkflowAutomationScheduleConfig::default()
    } else {
        WorkflowAutomationScheduleConfig::legacy_utc_needs_review()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowScheduleOccurrencePreview {
    /// Authoritative occurrence identity in UTC.
    pub scheduled_for: String,
    /// Human-readable zoned timestamp including the IANA timezone.
    pub local_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowSchedulePreview {
    pub cron: String,
    pub timezone: String,
    pub occurrences: Vec<WorkflowScheduleOccurrencePreview>,
}

fn validate_five_fields(cron: &str) -> Result<String, CoreError> {
    let normalized = cron.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = normalized.split_whitespace().count();
    if count != 5 {
        return Err(CoreError::InvalidInput(format!(
            "Workflow schedules require exactly five cron fields (minute hour day-of-month month day-of-week); got {count}"
        )));
    }
    Ok(normalized)
}

fn validate_timezone_token(timezone: &str) -> Result<String, CoreError> {
    let timezone = timezone.trim();
    if timezone.is_empty() {
        return Err(CoreError::InvalidInput(
            "Workflow schedule timezone cannot be empty".into(),
        ));
    }
    if timezone.split_whitespace().count() != 1 {
        return Err(CoreError::InvalidInput(
            "Workflow schedule timezone must be one IANA timezone identifier".into(),
        ));
    }
    Ok(timezone.to_string())
}

fn parse_cron5(cron: &str, timezone: &str) -> Result<(String, String, Crontab), CoreError> {
    let cron = validate_five_fields(cron)?;
    let timezone = validate_timezone_token(timezone)?;
    let expression = format!("{cron} {timezone}");
    let parsed = cronexpr::parse_crontab(&expression).map_err(|error| {
        CoreError::InvalidInput(format!(
            "Invalid workflow Cron5 schedule '{cron}' in timezone '{timezone}': {error}"
        ))
    })?;
    Ok((cron, timezone, parsed))
}

pub fn validate_workflow_cron_schedule(cron: &str, timezone: &str) -> Result<(), CoreError> {
    parse_cron5(cron, timezone).map(|_| ())
}

fn zoned_to_utc(value: &jiff::Zoned) -> Result<DateTime<Utc>, CoreError> {
    DateTime::parse_from_rfc3339(&value.timestamp().to_string())
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            CoreError::Internal(format!(
                "cronexpr returned an invalid occurrence timestamp '{}': {error}",
                value.timestamp()
            ))
        })
}

fn find_next_with_sparse_fallback(
    schedule: &Crontab,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<jiff::Zoned, CoreError> {
    let mut cursor = after;
    for _ in 0..MAX_SPARSE_SEARCH_WINDOWS {
        let input = MakeTimestamp::from_second(cursor.timestamp()).map_err(|error| {
            CoreError::InvalidInput(format!("Invalid workflow schedule cursor: {error}"))
        })?;
        match schedule.find_next(input) {
            Ok(next) => {
                let zone = jiff::tz::TimeZone::get(timezone).map_err(|error| {
                    CoreError::InvalidInput(format!(
                        "Invalid workflow schedule timezone '{timezone}': {error}"
                    ))
                })?;
                let earlier =
                    zone.to_ambiguous_zoned(next.datetime())
                        .earlier()
                        .map_err(|error| {
                            CoreError::Internal(format!(
                                "Unable to resolve workflow schedule timezone fold: {error}"
                            ))
                        })?;
                if earlier.timestamp() != next.timestamp() {
                    // A fall-back transition can expose every matching wall
                    // clock in the repeated interval twice. Durable local jobs
                    // consistently keep the earlier offset candidate only.
                    cursor = zoned_to_utc(&next)?;
                    continue;
                }
                return Ok(next);
            }
            Err(error) if error.to_string().contains("four years") => {
                // cronexpr intentionally bounds one search to four calendar
                // years. Advancing by less than that proven-empty window keeps
                // sparse valid schedules (for example 29 February across 2100)
                // discoverable without skipping an occurrence.
                cursor += Duration::days(SPARSE_SEARCH_ADVANCE_DAYS);
            }
            Err(error) => {
                return Err(CoreError::InvalidInput(format!(
                    "Unable to calculate the next workflow schedule occurrence: {error}"
                )))
            }
        }
    }
    Err(CoreError::InvalidInput(
        "Workflow schedule has no occurrence within the supported 95-year planning horizon".into(),
    ))
}

pub fn next_workflow_cron_occurrence(
    cron: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, CoreError> {
    let (_, _, schedule) = parse_cron5(cron, timezone)?;
    let next = find_next_with_sparse_fallback(&schedule, timezone, after)?;
    zoned_to_utc(&next)
}

/// Returns the last occurrence in the inclusive `[first_scheduled, at]` window.
///
/// `cronexpr` intentionally exposes a forward-only search. A monotonic binary
/// search over that public seam avoids replaying every missed minute when a
/// desktop runtime has been offline for a long time.
pub fn latest_workflow_cron_occurrence_at_or_before(
    cron: &str,
    timezone: &str,
    first_scheduled: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<DateTime<Utc>, CoreError> {
    if first_scheduled > at {
        return Err(CoreError::InvalidInput(
            "The first missed workflow occurrence must not be after the claim time".into(),
        ));
    }
    let (_, timezone, schedule) = parse_cron5(cron, timezone)?;
    let mut low = first_scheduled.timestamp().checked_sub(1).ok_or_else(|| {
        CoreError::InvalidInput("Workflow occurrence is outside the supported time range".into())
    })?;
    let mut high = at.timestamp();
    while high - low > 1 {
        let mid = low + (high - low) / 2;
        let cursor = DateTime::from_timestamp(mid, 0).ok_or_else(|| {
            CoreError::InvalidInput(
                "Workflow schedule cursor is outside the supported range".into(),
            )
        })?;
        let candidate = zoned_to_utc(&find_next_with_sparse_fallback(
            &schedule, &timezone, cursor,
        )?)?;
        if candidate <= at {
            low = mid;
        } else {
            high = mid;
        }
    }
    let cursor = DateTime::from_timestamp(low, 0).ok_or_else(|| {
        CoreError::InvalidInput("Workflow schedule cursor is outside the supported range".into())
    })?;
    let latest = zoned_to_utc(&find_next_with_sparse_fallback(
        &schedule, &timezone, cursor,
    )?)?;
    if latest < first_scheduled || latest > at {
        return Err(CoreError::Internal(
            "Workflow schedule could not reconcile the latest missed occurrence".into(),
        ));
    }
    Ok(latest)
}

pub fn preview_workflow_cron_schedule(
    cron: &str,
    timezone: &str,
    after: DateTime<Utc>,
    limit: usize,
) -> Result<WorkflowSchedulePreview, CoreError> {
    if limit == 0 || limit > MAX_PREVIEW_OCCURRENCES {
        return Err(CoreError::InvalidInput(format!(
            "Workflow schedule preview limit must be between 1 and {MAX_PREVIEW_OCCURRENCES}"
        )));
    }
    let (cron, timezone, schedule) = parse_cron5(cron, timezone)?;
    let mut cursor = after;
    let mut occurrences = Vec::with_capacity(limit);
    for _ in 0..limit {
        let next = find_next_with_sparse_fallback(&schedule, &timezone, cursor)?;
        let scheduled_for = zoned_to_utc(&next)?;
        occurrences.push(WorkflowScheduleOccurrencePreview {
            scheduled_for: scheduled_for.to_rfc3339_opts(SecondsFormat::Secs, true),
            local_time: next.to_string(),
        });
        cursor = scheduled_for;
    }
    Ok(WorkflowSchedulePreview {
        cron,
        timezone,
        occurrences,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn validates_full_cron5_lists_ranges_steps_months_and_weekdays() {
        let preview = preview_workflow_cron_schedule(
            "*/15 9-10 * JAN,MAR MON-FRI",
            "Asia/Shanghai",
            utc("2026-01-02T01:07:00Z"),
            4,
        )
        .unwrap();
        assert_eq!(
            preview
                .occurrences
                .iter()
                .map(|item| item.scheduled_for.as_str())
                .collect::<Vec<_>>(),
            vec![
                "2026-01-02T01:15:00Z",
                "2026-01-02T01:30:00Z",
                "2026-01-02T01:45:00Z",
                "2026-01-02T02:00:00Z",
            ]
        );
        assert!(preview.occurrences[0]
            .local_time
            .contains("[Asia/Shanghai]"));
    }

    #[test]
    fn uses_vixie_union_when_dom_and_dow_are_both_restricted() {
        let preview =
            preview_workflow_cron_schedule("0 0 1 * MON", "UTC", utc("2026-08-02T00:01:00Z"), 2)
                .unwrap();
        assert_eq!(preview.occurrences[0].scheduled_for, "2026-08-03T00:00:00Z");
        assert_eq!(preview.occurrences[1].scheduled_for, "2026-08-10T00:00:00Z");
    }

    #[test]
    fn wildcard_day_field_uses_vixie_intersection() {
        let next =
            next_workflow_cron_occurrence("0 12 * * TUE", "UTC", utc("2026-08-24T13:00:00Z"))
                .unwrap();
        assert_eq!(next, utc("2026-08-25T12:00:00Z"));
    }

    #[test]
    fn spring_forward_skips_nonexistent_local_time() {
        let next = next_workflow_cron_occurrence(
            "30 2 * * *",
            "America/New_York",
            utc("2026-03-07T08:00:00Z"),
        )
        .unwrap();
        assert_eq!(next, utc("2026-03-09T06:30:00Z"));
    }

    #[test]
    fn fall_back_does_not_duplicate_the_same_wall_clock_occurrence() {
        let first = next_workflow_cron_occurrence(
            "30 1 * * *",
            "America/New_York",
            utc("2026-11-01T04:00:00Z"),
        )
        .unwrap();
        assert_eq!(first, utc("2026-11-01T05:30:00Z"));

        let after_first = next_workflow_cron_occurrence(
            "30 1 * * *",
            "America/New_York",
            utc("2026-11-01T05:31:00Z"),
        )
        .unwrap();
        assert_eq!(after_first, utc("2026-11-02T06:30:00Z"));
    }

    #[test]
    fn fall_back_drops_every_later_fold_candidate_in_the_repeated_hour() {
        let preview = preview_workflow_cron_schedule(
            "0,30 1 * * *",
            "America/New_York",
            utc("2026-11-01T04:59:00Z"),
            4,
        )
        .unwrap();
        assert_eq!(
            preview
                .occurrences
                .iter()
                .map(|item| item.scheduled_for.as_str())
                .collect::<Vec<_>>(),
            vec![
                "2026-11-01T05:00:00Z",
                "2026-11-01T05:30:00Z",
                "2026-11-02T06:00:00Z",
                "2026-11-02T06:30:00Z",
            ]
        );
    }

    #[test]
    fn rejects_invalid_cron_and_timezone_before_persistence() {
        for cron in ["0 9 * *", "61 9 * * *", "*/0 9 * * *", "0 9 * FOO *"] {
            assert!(
                validate_workflow_cron_schedule(cron, "UTC").is_err(),
                "{cron}"
            );
        }
        assert!(validate_workflow_cron_schedule("0 9 * * *", "Mars/Olympus").is_err());
        assert!(validate_workflow_cron_schedule("0 9 * * * UTC", "UTC").is_err());
    }

    #[test]
    fn preview_is_bounded() {
        assert!(
            preview_workflow_cron_schedule("* * * * *", "UTC", utc("2026-08-27T00:00:00Z"), 0,)
                .is_err()
        );
        assert!(preview_workflow_cron_schedule(
            "* * * * *",
            "UTC",
            utc("2026-08-27T00:00:00Z"),
            21,
        )
        .is_err());
    }

    #[test]
    fn schedule_execution_policy_rejects_invalid_unattended_values() {
        let mut config = WorkflowAutomationScheduleConfig::default();
        config.execution_policy.context_window = Some(0);
        assert!(config.validate_for_save("0 9 * * *").is_err());

        let mut config = WorkflowAutomationScheduleConfig::default();
        config.execution_policy.power_mode = "turbo".into();
        assert!(config.validate_for_save("0 9 * * *").is_err());

        let mut config = WorkflowAutomationScheduleConfig::default();
        config.execution_policy.collaboration_mode = "committee".into();
        assert!(config.validate_for_save("0 9 * * *").is_err());

        let mut config = WorkflowAutomationScheduleConfig::default();
        config.execution_policy.orchestration_profile = "maximum".into();
        assert!(config.validate_for_save("0 9 * * *").is_err());

        let mut config = WorkflowAutomationScheduleConfig::default();
        config.execution_policy.execution_mode = Some("unsafe".into());
        assert!(config.validate_for_save("0 9 * * *").is_err());
    }

    #[test]
    fn legacy_cron_classification_only_promotes_provable_daily_utc_schedules() {
        for cron in ["0 9 * * *", "0   9\t* * *"] {
            let config = legacy_workflow_schedule_config(cron);
            assert_eq!(config.version, WORKFLOW_CRON_SCHEDULE_VERSION, "{cron}");
            assert_eq!(config.timezone, "UTC", "{cron}");
            assert!(!config.legacy_needs_review, "{cron}");
        }
        for cron in [
            "* * * * *",
            "0 9 * * 1-5",
            "60 9 * * *",
            "0 24 * * *",
            "00 09 * * *",
        ] {
            let config = legacy_workflow_schedule_config(cron);
            assert_eq!(config.version, 1, "{cron}");
            assert!(config.legacy_needs_review, "{cron}");
        }
    }
}
