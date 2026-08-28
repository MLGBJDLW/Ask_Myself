use super::WorkflowSchedulerEventType;
use chrono::Utc;
use uuid::Uuid;

use crate::agent::StreamBlockChannel;
use crate::agent_run::AgentRunEvent;
use crate::conversation::{AgentTaskRun, ConversationMessage, CreateConversationInput};
use crate::db::Database;
use crate::error::CoreError;
use crate::llm::Role;
use crate::workflow_automation::{
    workflow_automation_scheduler_retry_decision_from_events, SaveWorkflowAutomationInput,
    TaskResumeCheckpoint, WorkflowAutomation, WorkflowAutomationApprovalPolicy,
    WorkflowAutomationOccurrenceOrigin, WorkflowAutomationOccurrenceStatus,
    WorkflowAutomationRunStatus, WorkflowAutomationSchedulerEvent, WorkflowAutomationTrigger,
};
use crate::workflow_scheduler::{
    WorkflowAutomationScheduleConfig, WorkflowScheduleMisfirePolicy, WorkflowScheduleOverlapPolicy,
    WorkflowScheduleWorkspacePolicy,
};

fn add_user_message(db: &Database, conversation_id: &str, content: &str) -> ConversationMessage {
    let message = unpersisted_user_message(conversation_id, content);
    db.add_message(&message).unwrap();
    message
}

fn scheduled_automation(
    db: &Database,
    name: &str,
    cron: &str,
    schedule_config: WorkflowAutomationScheduleConfig,
) -> WorkflowAutomation {
    db.save_workflow_automation_with_schedule_config(
        &SaveWorkflowAutomationInput {
            id: None,
            name: name.into(),
            description: "scheduler contract test".into(),
            workflow_template_id: "report_brief".into(),
            prompt: "Run the scheduled contract test.".into(),
            trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
            source_scope: Vec::new(),
            approval_policy: WorkflowAutomationApprovalPolicy {
                require_before_run: false,
                allowed_tools: Vec::new(),
                risk_level: "low".into(),
            },
            enabled: true,
        },
        &schedule_config,
    )
    .unwrap()
}

fn scheduled_automation_requiring_approval(
    db: &Database,
    name: &str,
    cron: &str,
) -> WorkflowAutomation {
    db.save_workflow_automation_with_schedule_config(
        &SaveWorkflowAutomationInput {
            id: None,
            name: name.into(),
            description: "scheduler approval contract test".into(),
            workflow_template_id: "report_brief".into(),
            prompt: "Run only after approval.".into(),
            trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
            source_scope: Vec::new(),
            approval_policy: WorkflowAutomationApprovalPolicy {
                require_before_run: true,
                allowed_tools: Vec::new(),
                risk_level: "medium".into(),
            },
            enabled: true,
        },
        &WorkflowAutomationScheduleConfig::default(),
    )
    .unwrap()
}

fn create_test_agent_run(db: &Database, label: &str) -> AgentTaskRun {
    let conversation = db
        .create_conversation(&CreateConversationInput {
            provider: "openai".into(),
            model: "gpt-test".into(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap();
    let user = add_user_message(db, &conversation.id, label);
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, Some("workflow"))
        .unwrap();
    db.create_agent_task_run(
        &conversation.id,
        &turn.id,
        &user.id,
        label,
        Some("openai"),
        Some("gpt-test"),
    )
    .unwrap()
}

fn unpersisted_user_message(conversation_id: &str, content: &str) -> ConversationMessage {
    ConversationMessage {
        id: Uuid::new_v4().to_string(),
        conversation_id: conversation_id.to_string(),
        role: Role::User,
        content: content.to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 8,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    }
}

fn pause_task_run_with_checkpoint(
    db: &Database,
    run_id: &str,
    reason: &str,
) -> TaskResumeCheckpoint {
    let checkpoint = db
        .create_task_resume_checkpoint(run_id, reason)
        .expect("create test resume checkpoint");
    db.update_agent_task_run_progress(
        run_id,
        Some("paused"),
        Some("paused"),
        None,
        Some("Paused with a resumable checkpoint"),
        None,
        Some(&serde_json::json!({
            "kind": "resumeCheckpoint",
            "checkpointId": checkpoint.id,
            "resumePrompt": checkpoint.resume_prompt,
        })),
    )
    .expect("pause test task run");
    checkpoint
}

fn scheduler_event(
    id: &str,
    event_type: &str,
    created_at: &str,
) -> WorkflowAutomationSchedulerEvent {
    WorkflowAutomationSchedulerEvent {
        id: id.to_string(),
        automation_id: Some("automation-1".to_string()),
        run_id: None,
        event_type: event_type.to_string(),
        status: None,
        summary: event_type.to_string(),
        payload: serde_json::json!({}),
        created_at: created_at.to_string(),
    }
}

#[test]
fn workflow_scheduler_retry_decision_backs_off_consecutive_retryable_failures() {
    let events = vec![
        scheduler_event("event-3", "skipped_backoff", "2026-06-04T09:03:00Z"),
        scheduler_event("event-2", "launch_failed", "2026-06-04T09:00:00Z"),
        scheduler_event("event-1", "claim_failed", "2026-06-04T08:40:00Z"),
    ];

    let decision =
        workflow_automation_scheduler_retry_decision_from_events(&events, "2026-06-04T09:10:00Z")
            .unwrap();

    assert!(!decision.allowed);
    assert_eq!(decision.max_attempts, 4);
    assert!(!decision.attempts_exhausted);
    assert_eq!(decision.retryable_failure_count, 2);
    assert_eq!(
        decision.last_retryable_event_type.as_deref(),
        Some("launch_failed")
    );
    assert_eq!(decision.backoff_seconds, Some(900));
    assert_eq!(
        decision.backoff_until.as_deref(),
        Some("2026-06-04T09:15:00+00:00")
    );
    assert_eq!(decision.retry_after_seconds, Some(300));

    let elapsed =
        workflow_automation_scheduler_retry_decision_from_events(&events, "2026-06-04T09:15:00Z")
            .unwrap();
    assert!(elapsed.allowed);
    assert!(!elapsed.attempts_exhausted);
    assert_eq!(elapsed.retryable_failure_count, 2);
    assert_eq!(elapsed.retry_after_seconds, None);
}

#[test]
fn workflow_scheduler_retry_decision_blocks_after_max_attempts() {
    let events = vec![
        scheduler_event("event-5", "skipped_retry_limit", "2026-06-04T10:00:00Z"),
        scheduler_event("event-4", "launch_failed", "2026-06-04T09:00:00Z"),
        scheduler_event("event-3", "claim_failed", "2026-06-04T08:00:00Z"),
        scheduler_event("event-2", "launch_failed", "2026-06-04T07:00:00Z"),
        scheduler_event("event-1", "claim_failed", "2026-06-04T06:00:00Z"),
    ];

    let decision =
        workflow_automation_scheduler_retry_decision_from_events(&events, "2026-06-04T20:00:00Z")
            .unwrap();

    assert!(!decision.allowed);
    assert_eq!(decision.max_attempts, 4);
    assert!(decision.attempts_exhausted);
    assert_eq!(decision.retryable_failure_count, 4);
    assert_eq!(
        decision.last_retryable_event_type.as_deref(),
        Some("launch_failed")
    );
    assert_eq!(decision.backoff_seconds, Some(14_400));
    assert_eq!(decision.backoff_until, None);
    assert_eq!(decision.retry_after_seconds, None);
}

#[test]
fn workflow_scheduler_retry_decision_resets_after_progress_or_non_retry_gate() {
    let after_progress = workflow_automation_scheduler_retry_decision_from_events(
        &[
            scheduler_event("event-3", "launch_succeeded", "2026-06-04T09:05:00Z"),
            scheduler_event("event-2", "launch_failed", "2026-06-04T09:00:00Z"),
            scheduler_event("event-1", "claim_failed", "2026-06-04T08:55:00Z"),
        ],
        "2026-06-04T09:06:00Z",
    )
    .unwrap();
    assert!(after_progress.allowed);
    assert!(!after_progress.attempts_exhausted);
    assert_eq!(after_progress.retryable_failure_count, 0);
    assert_eq!(after_progress.backoff_until, None);

    let after_approval_gate = workflow_automation_scheduler_retry_decision_from_events(
        &[
            scheduler_event(
                "event-3",
                "skipped_pre_run_approval",
                "2026-06-04T09:05:00Z",
            ),
            scheduler_event("event-2", "launch_failed", "2026-06-04T09:00:00Z"),
        ],
        "2026-06-04T09:06:00Z",
    )
    .unwrap();
    assert!(after_approval_gate.allowed);
    assert!(!after_approval_gate.attempts_exhausted);
    assert_eq!(after_approval_gate.retryable_failure_count, 0);
}

#[test]
fn automation_lifecycle_computes_due_runs_and_audits_policy() {
    let db = Database::open_memory().unwrap();
    let saved = db
        .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
            id: None,
            name: "Morning inbox brief".into(),
            description: "Summarize new source material every morning.".into(),
            workflow_template_id: "report_brief".into(),
            prompt: "Summarize new documents in this source scope.".into(),
            trigger: crate::workflow_automation::WorkflowAutomationTrigger::Schedule {
                cron: "0 9 * * *".into(),
            },
            source_scope: vec!["source-a".into()],
            approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                require_before_run: true,
                allowed_tools: vec!["search_knowledge_base".into()],
                risk_level: "medium".into(),
            },
            enabled: true,
        })
        .unwrap();

    assert_eq!(saved.trigger_kind, "schedule");
    assert!(saved.next_run_at.is_some());
    assert!(saved.approval_policy.require_before_run);

    let due = db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].automation.id, saved.id);
    assert!(due[0].prompt.contains("Morning inbox brief"));
}

#[test]
fn invalid_cron_or_timezone_is_rejected_before_automation_is_saved() {
    let db = Database::open_memory().unwrap();
    for enabled in [true, false] {
        for (cron, timezone) in [
            ("61 9 * * *", "UTC"),
            ("0 9 * *", "UTC"),
            ("0 9 * * *", "Mars/Olympus"),
        ] {
            let mut config = WorkflowAutomationScheduleConfig::default();
            config.timezone = timezone.into();
            let result = db.save_workflow_automation_with_schedule_config(
                &SaveWorkflowAutomationInput {
                    id: None,
                    name: format!("invalid-{enabled}-{cron}-{timezone}"),
                    description: String::new(),
                    workflow_template_id: "report_brief".into(),
                    prompt: "must not persist".into(),
                    trigger: WorkflowAutomationTrigger::Schedule { cron: cron.into() },
                    source_scope: Vec::new(),
                    approval_policy: WorkflowAutomationApprovalPolicy::default(),
                    enabled,
                },
                &config,
            );
            assert!(result.is_err(), "enabled={enabled} {cron} {timezone}");
        }
    }
    assert!(db.list_workflow_automations().unwrap().is_empty());
}

#[test]
fn approval_occurrence_is_durable_actionable_and_single_winner() {
    let db = Database::open_memory().unwrap();
    let automation = db
        .save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: None,
                name: "approval-cas".into(),
                description: String::new(),
                workflow_template_id: "report_brief".into(),
                prompt: "Wait for approval.".into(),
                trigger: WorkflowAutomationTrigger::Schedule {
                    cron: "0 9 * * *".into(),
                },
                source_scope: Vec::new(),
                approval_policy: WorkflowAutomationApprovalPolicy {
                    require_before_run: true,
                    allowed_tools: Vec::new(),
                    risk_level: "medium".into(),
                },
                enabled: true,
            },
            &WorkflowAutomationScheduleConfig::default(),
        )
        .unwrap();
    let expected_next = automation.next_run_at.clone().unwrap();
    let due = db
        .list_due_workflow_automations(&expected_next)
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let claim = db
        .claim_workflow_automation_due_run_at(due, &expected_next, None)
        .unwrap();
    let run = claim.run.as_ref().unwrap();

    assert!(db
        .mark_workflow_automation_run_waiting_approval(&run.id)
        .unwrap());
    assert!(!db
        .mark_workflow_automation_run_waiting_approval(&run.id)
        .unwrap());

    let paused = db.get_workflow_automation(&automation.id).unwrap();
    assert!(paused.enabled);
    assert_eq!(paused.status, "waiting_approval");
    assert_eq!(paused.next_run_at.as_deref(), Some(expected_next.as_str()));
    let waiting = db.list_workflow_automation_runs_waiting_approval().unwrap();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].id, run.id);
    assert!(db
        .list_due_workflow_automations(&expected_next)
        .unwrap()
        .is_empty());
    let events = db
        .list_workflow_automation_scheduler_events(Some(&automation.id), 10)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "approval_requested");
    assert_eq!(events[0].status.as_deref(), Some("waiting_approval"));
    let approved = db
        .approve_workflow_automation_run_at(&run.id, &expected_next)
        .unwrap();
    assert_eq!(
        approved.run.as_ref().unwrap().status,
        WorkflowAutomationRunStatus::Queued
    );
    assert_eq!(
        approved.occurrence.as_ref().unwrap().status,
        WorkflowAutomationOccurrenceStatus::Claimed
    );
    assert_eq!(
        db.workflow_automation_occurrence_approval_state(
            approved.occurrence.as_ref().unwrap().id.as_str()
        )
        .unwrap(),
        super::WorkflowAutomationApprovalState::Approved
    );
    assert!(db
        .list_workflow_automation_runs_waiting_approval()
        .unwrap()
        .is_empty());
}

#[test]
fn denied_approval_consumes_occurrence_and_advances_schedule() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation_requiring_approval(&db, "approval-denied", "0 9 * * *");
    let scheduled_for = automation.next_run_at.clone().unwrap();
    let due = db
        .list_due_workflow_automations(&scheduled_for)
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let claim = db
        .claim_workflow_automation_due_run_at(due, &scheduled_for, None)
        .unwrap();
    let run = claim.run.as_ref().unwrap();
    db.mark_workflow_automation_run_waiting_approval(&run.id)
        .unwrap();

    let denied = db
        .deny_workflow_automation_run_at(&run.id, &scheduled_for)
        .unwrap();
    assert_eq!(denied.status, WorkflowAutomationRunStatus::Cancelled);
    let occurrence = db
        .get_workflow_automation_occurrence(denied.occurrence_id.as_deref().expect("occurrence id"))
        .unwrap();
    assert_eq!(
        occurrence.status,
        WorkflowAutomationOccurrenceStatus::Skipped
    );
    assert_eq!(
        occurrence.last_error.as_deref(),
        Some("pre_run_approval_denied")
    );
    assert_eq!(
        db.workflow_automation_occurrence_approval_state(&occurrence.id)
            .unwrap(),
        super::WorkflowAutomationApprovalState::Denied
    );
    let advanced = db.get_workflow_automation(&automation.id).unwrap();
    assert_eq!(advanced.status, "ready");
    assert!(
        super::parse_utc_timestamp(advanced.next_run_at.as_deref().unwrap()).unwrap()
            > super::parse_utc_timestamp(&scheduled_for).unwrap()
    );
}

#[test]
fn manual_run_now_uses_durable_approval_without_consuming_cron_cursor() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation_requiring_approval(&db, "manual-run-now", "0 9 * * *");
    let recurring_cursor = automation.next_run_at.clone().unwrap();
    let now = Utc::now().to_rfc3339();
    let due = db
        .workflow_automation_run_now_due_at(&automation.id, &now)
        .unwrap();
    assert_eq!(due.origin, WorkflowAutomationOccurrenceOrigin::ManualRunNow);
    let claim = db
        .claim_workflow_automation_due_run_at(due, &now, Some("run now"))
        .unwrap();
    let run = claim.run.as_ref().unwrap();
    assert!(db
        .mark_workflow_automation_run_waiting_approval(&run.id)
        .unwrap());
    assert_eq!(
        db.get_workflow_automation(&automation.id)
            .unwrap()
            .next_run_at
            .as_deref(),
        Some(recurring_cursor.as_str())
    );

    db.deny_workflow_automation_run_at(&run.id, &now).unwrap();
    let restored = db.get_workflow_automation(&automation.id).unwrap();
    assert_eq!(restored.status, "ready");
    assert_eq!(
        restored.next_run_at.as_deref(),
        Some(recurring_cursor.as_str())
    );
}

#[test]
fn starting_manual_run_now_preserves_recurring_cursor() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation(
        &db,
        "manual-run-now-start",
        "0 9 * * *",
        WorkflowAutomationScheduleConfig::default(),
    );
    let recurring_cursor = automation.next_run_at.clone().unwrap();
    let now = Utc::now().to_rfc3339();
    let due = db
        .workflow_automation_run_now_due_at(&automation.id, &now)
        .unwrap();
    let claim = db
        .claim_workflow_automation_due_run_at(due, &now, None)
        .unwrap();
    let task = create_test_agent_run(&db, "manual run now task");
    db.start_workflow_automation_run_at(&claim.run.as_ref().unwrap().id, &task.id, None, &now)
        .unwrap();
    assert_eq!(
        db.get_workflow_automation(&automation.id)
            .unwrap()
            .next_run_at
            .as_deref(),
        Some(recurring_cursor.as_str())
    );
}

#[test]
fn schedule_revision_remains_monotonic_across_trigger_kind_round_trip() {
    let db = Database::open_memory().unwrap();
    let scheduled = scheduled_automation(
        &db,
        "schedule-round-trip",
        "0 9 * * *",
        WorkflowAutomationScheduleConfig::default(),
    );
    let base = SaveWorkflowAutomationInput {
        id: Some(scheduled.id.clone()),
        name: scheduled.name.clone(),
        description: scheduled.description.clone(),
        workflow_template_id: scheduled.workflow_template_id.clone(),
        prompt: scheduled.prompt.clone(),
        trigger: WorkflowAutomationTrigger::Manual,
        source_scope: scheduled.source_scope.clone(),
        approval_policy: scheduled.approval_policy.clone(),
        enabled: true,
    };
    db.save_workflow_automation_with_schedule_config(
        &base,
        &WorkflowAutomationScheduleConfig::default(),
    )
    .unwrap();
    db.save_workflow_automation_with_schedule_config(
        &SaveWorkflowAutomationInput {
            trigger: WorkflowAutomationTrigger::Schedule {
                cron: "30 9 * * *".into(),
            },
            ..base
        },
        &WorkflowAutomationScheduleConfig::default(),
    )
    .unwrap();
    let revisions: Vec<i64> = {
        let conn = db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT revision FROM workflow_automation_definition_revisions
                     WHERE automation_id = ?1 ORDER BY revision",
            )
            .unwrap();
        stmt.query_map([&scheduled.id], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(revisions, vec![1, 2]);
}

#[test]
fn editing_definition_snapshots_revision_and_explicitly_cancels_waiting_occurrence() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation_requiring_approval(&db, "revision-cancel", "0 9 * * *");
    let scheduled_for = automation.next_run_at.clone().unwrap();
    let due = db
        .list_due_workflow_automations(&scheduled_for)
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let claim = db
        .claim_workflow_automation_due_run_at(due, &scheduled_for, None)
        .unwrap();
    let run = claim.run.as_ref().unwrap().clone();
    db.mark_workflow_automation_run_waiting_approval(&run.id)
        .unwrap();

    db.save_workflow_automation_with_schedule_config(
        &SaveWorkflowAutomationInput {
            id: Some(automation.id.clone()),
            name: automation.name.clone(),
            description: automation.description.clone(),
            workflow_template_id: automation.workflow_template_id.clone(),
            prompt: "Run the revised definition only.".into(),
            trigger: automation.trigger.clone(),
            source_scope: automation.source_scope.clone(),
            approval_policy: automation.approval_policy.clone(),
            enabled: true,
        },
        &WorkflowAutomationScheduleConfig::default(),
    )
    .unwrap();

    assert_eq!(
        db.get_workflow_automation_run(&run.id).unwrap().status,
        WorkflowAutomationRunStatus::Cancelled
    );
    let occurrence = db
        .get_workflow_automation_occurrence(run.occurrence_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(
        occurrence.status,
        WorkflowAutomationOccurrenceStatus::Cancelled
    );
    assert_eq!(
        occurrence.last_error.as_deref(),
        Some("definition_superseded")
    );
    assert!(db
        .list_workflow_automation_runs_waiting_approval()
        .unwrap()
        .is_empty());
    let snapshot_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM workflow_automation_definition_revisions
                 WHERE automation_id = ?1",
            [&automation.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(snapshot_count, 2);
}

#[test]
fn damaged_unknown_or_missing_schedule_config_is_projected_fail_closed() {
    for scenario in ["damaged", "unknown-version", "missing"] {
        let db = Database::open_memory().unwrap();
        let automation = scheduled_automation(
            &db,
            scenario,
            "0 9 * * *",
            WorkflowAutomationScheduleConfig::default(),
        );
        match scenario {
            "damaged" => {
                db.conn()
                    .execute(
                        "UPDATE workflow_automation_schedule_configs
                             SET config_json = '{' WHERE automation_id = ?1",
                        [&automation.id],
                    )
                    .unwrap();
            }
            "unknown-version" => {
                db.conn()
                    .execute(
                        "UPDATE workflow_automation_schedule_configs
                             SET config_json = '{\"version\":99}' WHERE automation_id = ?1",
                        [&automation.id],
                    )
                    .unwrap();
            }
            "missing" => {
                db.conn()
                    .execute(
                        "DELETE FROM workflow_automation_schedule_configs
                             WHERE automation_id = ?1",
                        [&automation.id],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }

        let loaded = db.get_workflow_automation(&automation.id).unwrap();
        assert!(!loaded.enabled, "{scenario}");
        assert_eq!(loaded.status, "needs_review", "{scenario}");
        assert!(loaded.next_run_at.is_none(), "{scenario}");
        assert!(loaded.schedule_config.legacy_needs_review, "{scenario}");
        assert!(
            db.list_due_workflow_automations("2099-01-01T09:00:00Z")
                .unwrap()
                .is_empty(),
            "{scenario}"
        );
    }
}

#[test]
fn live_occurrence_claim_is_idempotent_and_expired_lease_creates_new_attempt() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation(
        &db,
        "lease",
        "0 9 * * *",
        WorkflowAutomationScheduleConfig::default(),
    );
    let due = db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let first = db
        .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:00Z", None)
        .unwrap();
    let duplicate = db
        .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:30Z", None)
        .unwrap();
    assert!(duplicate.run.is_none());
    assert_eq!(
        duplicate.skip_reason.as_deref(),
        Some("already_claimed_live")
    );
    assert_eq!(
        duplicate.occurrence.as_ref().unwrap().id,
        first.occurrence.as_ref().unwrap().id
    );
    let run_count: i64 = db
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM workflow_automation_runs WHERE automation_id = ?1",
            [&automation.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(run_count, 1);

    let reclaimed = db
        .claim_workflow_automation_due_run_at(due, "2099-01-01T09:03:00Z", None)
        .unwrap();
    assert_eq!(reclaimed.run.as_ref().unwrap().attempt, 2);
    assert_eq!(
        reclaimed.occurrence.as_ref().unwrap().id,
        first.occurrence.as_ref().unwrap().id
    );
}

#[test]
fn superseded_occurrence_attempt_cannot_start_after_lease_reclaim() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation(
        &db,
        "lease-fence",
        "0 9 * * *",
        WorkflowAutomationScheduleConfig::default(),
    );
    let due = db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let first = db
        .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:00Z", None)
        .unwrap();
    let reclaimed = db
        .claim_workflow_automation_due_run_at(due, "2099-01-01T09:03:00Z", None)
        .unwrap();
    let first_run = first.run.as_ref().unwrap();
    let reclaimed_run = reclaimed.run.as_ref().unwrap();
    let stale_task = create_test_agent_run(&db, "stale scheduled start");

    let stale_start = db.start_workflow_automation_run_at(
        &first_run.id,
        &stale_task.id,
        None,
        "2099-01-01T09:03:01Z",
    );
    assert!(stale_start.is_err());
    assert_eq!(
        db.get_workflow_automation_run(&first_run.id)
            .unwrap()
            .status,
        WorkflowAutomationRunStatus::Cancelled
    );

    let current_task = create_test_agent_run(&db, "current scheduled start");
    db.start_workflow_automation_run_at(
        &reclaimed_run.id,
        &current_task.id,
        None,
        "2099-01-01T09:03:01Z",
    )
    .unwrap();
    assert_eq!(
        db.get_workflow_automation_occurrence(reclaimed.occurrence.as_ref().unwrap().id.as_str())
            .unwrap()
            .status,
        WorkflowAutomationOccurrenceStatus::Running
    );
}

#[test]
fn claim_reloads_the_authoritative_definition_snapshot_and_revision() {
    let db = Database::open_memory().unwrap();
    let mut first_config = WorkflowAutomationScheduleConfig::default();
    first_config.execution_policy.model = Some("old-model".into());
    let automation = scheduled_automation(&db, "definition-snapshot", "0 0 1 1 *", first_config);
    let stale_due = db
        .list_due_workflow_automations("2099-01-01T00:00:00Z")
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();

    let mut current_config = WorkflowAutomationScheduleConfig::default();
    current_config.execution_policy.model = Some("current-model".into());
    db.save_workflow_automation_with_schedule_config(
        &SaveWorkflowAutomationInput {
            id: Some(automation.id.clone()),
            name: automation.name.clone(),
            description: automation.description.clone(),
            workflow_template_id: automation.workflow_template_id.clone(),
            prompt: "Run the current definition snapshot.".into(),
            trigger: automation.trigger.clone(),
            source_scope: automation.source_scope.clone(),
            approval_policy: automation.approval_policy.clone(),
            enabled: true,
        },
        &current_config,
    )
    .unwrap();

    let claim = db
        .claim_workflow_automation_due_run_at(stale_due, "2099-01-01T00:00:00Z", None)
        .unwrap();
    assert_eq!(claim.occurrence.as_ref().unwrap().definition_revision, 2);
    assert_eq!(
        claim
            .due_run
            .automation
            .schedule_config
            .execution_policy
            .model
            .as_deref(),
        Some("current-model")
    );
    assert!(claim
        .due_run
        .prompt
        .contains("Run the current definition snapshot."));
    assert!(!claim.due_run.prompt.contains("old-model"));
}

#[test]
fn overlap_policy_is_enforced_inside_atomic_claim() {
    for (policy, expects_run, reason) in [
        (
            WorkflowScheduleOverlapPolicy::Skip,
            false,
            Some("overlap_active"),
        ),
        (WorkflowScheduleOverlapPolicy::Allow, true, None),
    ] {
        let db = Database::open_memory().unwrap();
        let mut config = WorkflowAutomationScheduleConfig::default();
        config.overlap_policy = policy;
        let automation = scheduled_automation(&db, "overlap", "0 9 * * *", config);
        let existing = db
            .record_workflow_automation_run(&automation.id, None, "queued", Some("existing"))
            .unwrap();
        let task = create_test_agent_run(&db, "existing active run");
        db.start_workflow_automation_run(&existing.id, &task.id, None)
            .unwrap();
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let claim = db
            .claim_workflow_automation_due_run_at(due, "2099-01-01T09:00:00Z", None)
            .unwrap();
        assert_eq!(claim.run.is_some(), expects_run);
        assert_eq!(claim.skip_reason.as_deref(), reason);
    }
}

#[test]
fn isolated_schedules_lock_one_source_across_automation_definitions() {
    let db = Database::open_memory().unwrap();
    let mut config = WorkflowAutomationScheduleConfig::default();
    config.execution_policy.workspace_policy = WorkflowScheduleWorkspacePolicy::IsolatedPatch;
    config.execution_policy.orchestration_profile = "codeUltra".into();
    config.execution_policy.source_root_fingerprint = Some("blake3:test-source".into());
    let save = |name: &str, source_id: &str| {
        db.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: None,
                name: name.into(),
                description: String::new(),
                workflow_template_id: "report_brief".into(),
                prompt: "Apply one isolated patch.".into(),
                trigger: WorkflowAutomationTrigger::Schedule {
                    cron: "0 9 * * *".into(),
                },
                source_scope: vec![source_id.into()],
                approval_policy: WorkflowAutomationApprovalPolicy {
                    require_before_run: false,
                    allowed_tools: vec!["edit_file".into()],
                    risk_level: "high".into(),
                },
                enabled: true,
            },
            &config,
        )
        .unwrap()
    };
    let first = save("isolated-lock-first", "source-canonical");
    let second = save("isolated-lock-second", "source-alias");
    let at = [
        first.next_run_at.clone().unwrap(),
        second.next_run_at.clone().unwrap(),
    ]
    .into_iter()
    .max()
    .unwrap();
    let due = db.list_due_workflow_automations(&at).unwrap();
    let first_due = due
        .iter()
        .find(|item| item.automation.id == first.id)
        .unwrap()
        .clone();
    let second_due = due
        .iter()
        .find(|item| item.automation.id == second.id)
        .unwrap()
        .clone();

    assert!(db
        .claim_workflow_automation_due_run_at(first_due, &at, None)
        .unwrap()
        .run
        .is_some());
    let blocked = db
        .claim_workflow_automation_due_run_at(second_due, &at, None)
        .unwrap();
    assert!(blocked.run.is_none());
    assert_eq!(
        blocked.skip_reason.as_deref(),
        Some("source_workspace_locked")
    );
    assert_eq!(
        blocked.occurrence.as_ref().unwrap().status,
        WorkflowAutomationOccurrenceStatus::Planned
    );
}

#[test]
fn misfire_policy_skips_or_runs_latest_occurrence() {
    for (policy, expects_run) in [
        (WorkflowScheduleMisfirePolicy::Skip, false),
        (WorkflowScheduleMisfirePolicy::RunLatest, true),
    ] {
        let db = Database::open_memory().unwrap();
        let mut config = WorkflowAutomationScheduleConfig::default();
        config.misfire_policy = policy;
        config.misfire_grace_seconds = 60;
        let automation = scheduled_automation(&db, "misfire", "0 9 * * *", config);
        let due = db
            .list_due_workflow_automations("2099-01-01T09:00:00Z")
            .unwrap()
            .into_iter()
            .find(|item| item.automation.id == automation.id)
            .unwrap();
        let claim = db
            .claim_workflow_automation_due_run_at(due, "2099-01-01T09:00:00Z", None)
            .unwrap();
        assert_eq!(claim.run.is_some(), expects_run);
        if policy == WorkflowScheduleMisfirePolicy::Skip {
            assert_eq!(claim.skip_reason.as_deref(), Some("misfire_grace_exceeded"));
            assert_eq!(
                claim.occurrence.unwrap().status,
                WorkflowAutomationOccurrenceStatus::Skipped
            );
        }
    }
}

#[test]
fn run_latest_materializes_the_last_missed_occurrence_at_or_before_now() {
    let db = Database::open_memory().unwrap();
    let mut config = WorkflowAutomationScheduleConfig::default();
    config.misfire_policy = WorkflowScheduleMisfirePolicy::RunLatest;
    let automation = scheduled_automation(&db, "latest-misfire", "0 0 1 1 *", config);
    let due = db
        .list_due_workflow_automations("2099-08-27T12:34:56Z")
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();

    let claim = db
        .claim_workflow_automation_due_run_at(due, "2099-08-27T12:34:56Z", None)
        .unwrap();
    assert_eq!(
        claim.occurrence.as_ref().unwrap().scheduled_for,
        "2099-01-01T00:00:00+00:00"
    );
    assert_eq!(
        claim.run.as_ref().unwrap().scheduled_for.as_deref(),
        Some("2099-01-01T00:00:00+00:00")
    );
    assert_eq!(
        claim.due_run.scheduled_for.as_deref(),
        Some("2099-01-01T00:00:00+00:00")
    );
}

#[test]
fn launch_failure_retries_same_occurrence_with_a_new_attempt_run() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation(
        &db,
        "retry",
        "0 9 * * *",
        WorkflowAutomationScheduleConfig::default(),
    );
    let due = db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let first = db
        .claim_workflow_automation_due_run_at(due.clone(), "2099-01-01T09:00:00Z", None)
        .unwrap();
    let first_run = first.run.as_ref().unwrap();
    db.mark_workflow_automation_launch_failed_for_retry(
        &first_run.id,
        "provider unavailable",
        "2099-01-01T09:00:00Z",
    )
    .unwrap();
    let second = db
        .claim_workflow_automation_due_run_at(due, "2099-01-01T09:05:01Z", None)
        .unwrap();
    let second_run = second.run.as_ref().unwrap();
    assert_ne!(first_run.id, second_run.id);
    assert_eq!(second_run.attempt, 2);
    assert_eq!(first_run.occurrence_id, second_run.occurrence_id);
    assert_eq!(
        db.get_workflow_automation_run(&first_run.id)
            .unwrap()
            .status,
        WorkflowAutomationRunStatus::Cancelled
    );
}

#[test]
fn retry_wait_occurrence_is_not_reclassified_as_a_misfire() {
    let db = Database::open_memory().unwrap();
    let mut config = WorkflowAutomationScheduleConfig::default();
    config.misfire_policy = WorkflowScheduleMisfirePolicy::Skip;
    config.misfire_grace_seconds = 60;
    let automation = scheduled_automation(&db, "retry-misfire", "0 9 * * *", config);
    let due = db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let scheduled_for = due.scheduled_for.clone().unwrap();
    let first = db
        .claim_workflow_automation_due_run_at(due.clone(), &scheduled_for, None)
        .unwrap();
    let first_run = first.run.as_ref().unwrap();
    db.mark_workflow_automation_launch_failed_for_retry(
        &first_run.id,
        "provider unavailable",
        &scheduled_for,
    )
    .unwrap();
    let retry_now = (super::parse_utc_timestamp(&scheduled_for).unwrap()
        + chrono::Duration::minutes(10))
    .to_rfc3339();

    let retry = db
        .claim_workflow_automation_due_run_at(due, &retry_now, None)
        .unwrap();
    assert!(retry.skip_reason.is_none());
    assert_eq!(retry.run.as_ref().unwrap().attempt, 2);
    assert_eq!(first_run.occurrence_id, retry.run.unwrap().occurrence_id);
}

#[test]
fn starting_occurrence_advances_schedule_and_completion_updates_occurrence() {
    let db = Database::open_memory().unwrap();
    let automation = scheduled_automation(
        &db,
        "advance",
        "0 9 * * *",
        WorkflowAutomationScheduleConfig::default(),
    );
    let due = db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .into_iter()
        .find(|item| item.automation.id == automation.id)
        .unwrap();
    let claim = db
        .claim_workflow_automation_due_run_at(due, "2099-01-01T09:00:00Z", None)
        .unwrap();
    let run = claim.run.as_ref().unwrap();
    let task = create_test_agent_run(&db, "scheduled start");
    db.start_workflow_automation_run_at(&run.id, &task.id, None, "2099-01-01T09:00:00Z")
        .unwrap();
    let advanced = db.get_workflow_automation(&automation.id).unwrap();
    assert!(
        super::parse_utc_timestamp(advanced.next_run_at.as_deref().unwrap()).unwrap()
            > super::parse_utc_timestamp("2099-01-01T09:00:00Z").unwrap()
    );
    db.transition_workflow_automation_run(&run.id, "completed", Some("done"))
        .unwrap();
    assert_eq!(
        db.get_workflow_automation_occurrence(
            claim
                .occurrence
                .as_ref()
                .map(|item| item.id.as_str())
                .unwrap()
        )
        .unwrap()
        .status,
        WorkflowAutomationOccurrenceStatus::Completed
    );
}

#[test]
fn folder_trigger_detects_matching_files_and_advances_after_run() {
    let db = Database::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let saved = db
        .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
            id: None,
            name: "PDF actions".into(),
            description: "Extract actions when PDFs appear.".into(),
            workflow_template_id: "document_compare".into(),
            prompt: "Extract action items from new PDFs.".into(),
            trigger: crate::workflow_automation::WorkflowAutomationTrigger::Folder {
                path: dir.path().display().to_string(),
                pattern: "*.pdf".into(),
            },
            source_scope: vec![],
            approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                require_before_run: true,
                allowed_tools: vec!["read_file".into()],
                risk_level: "medium".into(),
            },
            enabled: true,
        })
        .unwrap();

    assert!(db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .is_empty());
    std::fs::write(dir.path().join("incoming.pdf"), b"%PDF-1.4").unwrap();
    let due = db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].automation.id, saved.id);
    assert!(due[0].due_reason.contains("folder trigger"));

    db.record_workflow_automation_run(&saved.id, None, "completed", Some("done"))
        .unwrap();
    assert!(db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .is_empty());
}

#[test]
fn due_workflow_claim_creates_queued_run_and_advances_folder_trigger() {
    let db = Database::open_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let saved = db
        .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
            id: None,
            name: "Claim PDFs".into(),
            description: "Claim new PDFs for processing.".into(),
            workflow_template_id: "document_compare".into(),
            prompt: "Review new PDFs.".into(),
            trigger: crate::workflow_automation::WorkflowAutomationTrigger::Folder {
                path: dir.path().display().to_string(),
                pattern: "*.pdf".into(),
            },
            source_scope: vec!["source-a".into()],
            approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                require_before_run: true,
                allowed_tools: vec!["read_file".into()],
                risk_level: "medium".into(),
            },
            enabled: true,
        })
        .unwrap();
    std::fs::write(dir.path().join("incoming.pdf"), b"%PDF-1.4").unwrap();

    let claim = db
        .claim_due_workflow_automation_run(&saved.id, "2099-01-01T09:00:00Z", None)
        .unwrap();
    let run = claim.run.as_ref().expect("folder claim creates a run");

    assert_eq!(claim.due_run.automation.id, saved.id);
    assert_eq!(run.automation_id, saved.id);
    assert_eq!(run.status, WorkflowAutomationRunStatus::Queued);
    assert_eq!(
        run.summary.as_deref(),
        Some(claim.due_run.due_reason.as_str())
    );
    assert_eq!(
        db.get_workflow_automation(&saved.id).unwrap().status,
        "queued"
    );
    assert!(db
        .list_due_workflow_automations("2099-01-01T09:00:00Z")
        .unwrap()
        .is_empty());
}

#[test]
fn workflow_automation_run_binds_to_agent_task_run_and_transitions() {
    let db = Database::open_memory().unwrap();
    let automation = db
        .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
            id: None,
            name: "Manual brief".into(),
            description: "Run a scoped brief on demand.".into(),
            workflow_template_id: "report_brief".into(),
            prompt: "Summarize this source scope.".into(),
            trigger: crate::workflow_automation::WorkflowAutomationTrigger::Manual,
            source_scope: vec!["source-a".into()],
            approval_policy: crate::workflow_automation::WorkflowAutomationApprovalPolicy {
                require_before_run: false,
                allowed_tools: vec![],
                risk_level: "low".into(),
            },
            enabled: true,
        })
        .unwrap();
    let queued_run = db
        .record_workflow_automation_run(&automation.id, None, "queued", Some("queued"))
        .unwrap();
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
    let user = add_user_message(&db, &conversation.id, "Run the brief");
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, Some("workflow"))
        .unwrap();
    let task_run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user.id,
            "Manual brief",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();

    let running = db
        .start_workflow_automation_run(&queued_run.id, &task_run.id, Some("Agent session started"))
        .unwrap();

    assert_eq!(running.status, WorkflowAutomationRunStatus::Running);
    assert_eq!(running.task_run_id.as_deref(), Some(task_run.id.as_str()));
    assert_eq!(running.summary.as_deref(), Some("Agent session started"));
    assert_eq!(
        db.get_workflow_automation(&automation.id).unwrap().status,
        "running"
    );

    let completed = db
        .transition_workflow_automation_run(&queued_run.id, "completed", Some("done"))
        .unwrap();

    assert_eq!(completed.status, WorkflowAutomationRunStatus::Completed);
    assert_eq!(completed.summary.as_deref(), Some("done"));
    assert!(completed.finished_at.is_some());
    assert_eq!(
        db.get_workflow_automation(&automation.id).unwrap().status,
        "completed"
    );

    let restart_err = db
        .start_workflow_automation_run(&queued_run.id, &task_run.id, None)
        .unwrap_err();
    assert!(restart_err.to_string().contains("terminal task state"));
}

#[test]
fn workflow_scheduler_events_persist_payload_and_filter_by_automation() {
    let db = Database::open_memory().unwrap();
    let automation = db
        .save_workflow_automation(&crate::workflow_automation::SaveWorkflowAutomationInput {
            id: None,
            name: "Scheduler audit".into(),
            description: "Audit scheduler decisions.".into(),
            workflow_template_id: "report_brief".into(),
            prompt: "Summarize due work.".into(),
            trigger: crate::workflow_automation::WorkflowAutomationTrigger::Manual,
            source_scope: vec![],
            approval_policy: Default::default(),
            enabled: true,
        })
        .unwrap();
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
    let user = add_user_message(&db, &conversation.id, "Run the scheduler audit");
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, Some("workflow"))
        .unwrap();
    let task_run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user.id,
            "Scheduler audit",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
    let run = db
        .record_workflow_automation_run(
            &automation.id,
            Some(&task_run.id),
            "queued",
            Some("queued"),
        )
        .unwrap();

    let event = db
        .record_workflow_automation_scheduler_event(
            Some(&automation.id),
            Some(&run.id),
            WorkflowSchedulerEventType::LaunchSucceeded,
            Some("running"),
            "Scheduler launched workflow",
            Some(&serde_json::json!({
                "queueId": format!("workflow_due:{}", automation.id),
                "delivery": "scheduler"
            })),
        )
        .unwrap();

    assert_eq!(event.automation_id.as_deref(), Some(automation.id.as_str()));
    assert_eq!(event.run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(event.event_type, "launch_succeeded");
    assert_eq!(event.status.as_deref(), Some("running"));
    assert_eq!(event.payload["delivery"], "scheduler");

    let events = db
        .list_workflow_automation_scheduler_events(Some(&automation.id), 10)
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, event.id);

    let all_events = db
        .list_workflow_automation_scheduler_events(None, 10)
        .unwrap();
    assert_eq!(all_events.len(), 1);

    let run_events = db
        .list_workflow_automation_scheduler_events_for_run(&run.id, 10)
        .unwrap();
    assert_eq!(run_events.len(), 1);
    assert_eq!(run_events[0].id, event.id);

    let unrelated_run_events = db
        .list_workflow_automation_scheduler_events_for_run("missing-run", 10)
        .unwrap();
    assert!(unrelated_run_events.is_empty());

    let task_run_events = db
        .list_workflow_automation_scheduler_events_for_task_run(&task_run.id, 10)
        .unwrap();
    assert_eq!(task_run_events.len(), 1);
    assert_eq!(task_run_events[0].id, event.id);

    let unrelated_task_run_events = db
        .list_workflow_automation_scheduler_events_for_task_run("missing-task-run", 10)
        .unwrap();
    assert!(unrelated_task_run_events.is_empty());
}

#[test]
fn task_resume_checkpoint_builds_a_resume_prompt() {
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
    let user = add_user_message(&db, &conversation.id, "Compare these documents");
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, Some("knowledge"))
        .unwrap();
    let run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user.id,
            "Document comparison",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
    let plan = serde_json::json!({ "steps": [{ "id": "map", "status": "completed" }] });
    db.update_agent_task_run_progress(
        &run.id,
        Some("running"),
        Some("compare"),
        Some("knowledge"),
        Some("Mapped the input documents"),
        Some(&plan),
        None,
    )
    .unwrap();

    let checkpoint = db
        .create_task_resume_checkpoint(&run.id, "user_pause")
        .unwrap();
    assert!(checkpoint.resume_prompt.contains("Document comparison"));
    assert!(checkpoint
        .resume_prompt
        .contains("Do not redo completed tool work"));
    assert!(checkpoint
        .resume_prompt
        .contains("Start by naming the resumed checkpoint"));
    assert!(checkpoint.state.get("run").is_some());

    let prompt = db.build_task_resume_prompt(&run.id).unwrap();
    assert!(prompt.prompt.contains("Resume this Nexa task"));
}

#[test]
fn task_resume_checkpoint_can_embed_live_turn_state() {
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
    let user = add_user_message(&db, &conversation.id, "Research for a while");
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, Some("web"))
        .unwrap();
    let run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user.id,
            "Long research task",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
    let live_state = serde_json::json!({
        "kind": "longTaskLiveState",
        "iteration": 3,
        "taskPlan": {
            "objective": "Research for a while",
            "steps": []
        }
    });

    let checkpoint = db
        .create_task_resume_checkpoint_with_state(&run.id, "auto_tool_round_3", Some(&live_state))
        .unwrap();

    assert_eq!(
        checkpoint.state["liveTurnState"]["kind"].as_str(),
        Some("longTaskLiveState")
    );
    assert_eq!(
        checkpoint.state["liveTurnState"]["taskPlan"]["objective"].as_str(),
        Some("Research for a while")
    );
    assert!(checkpoint
        .resume_prompt
        .contains("Prefer liveTurnState.taskPlan"));
}

#[test]
fn task_resume_checkpoint_carries_partial_assistant_output_forward() {
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
    let user = add_user_message(&db, &conversation.id, "Explain the result");
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, Some("chat"))
        .unwrap();
    let run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &user.id,
            "Partial response",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
    db.save_agent_run_event(&AgentRunEvent::output_delta(
        &run.id,
        Some(&turn.id),
        1,
        "answer-block",
        StreamBlockChannel::Answer,
        0,
        "Partial ",
    ))
    .unwrap();
    db.save_agent_run_event(&AgentRunEvent::output_delta(
        &run.id,
        Some(&turn.id),
        2,
        "answer-block",
        StreamBlockChannel::Answer,
        8,
        "answer",
    ))
    .unwrap();

    let checkpoint = db
        .create_task_resume_checkpoint(&run.id, "user_stop")
        .unwrap();

    assert_eq!(
        checkpoint.state["partialAssistantOutput"]["text"].as_str(),
        Some("Partial answer")
    );
    assert!(checkpoint.resume_prompt.contains("Partial answer"));
    assert!(checkpoint
        .resume_prompt
        .contains("continue after it without repeating it"));
}

#[test]
fn task_checkpoint_resume_requeues_the_original_turn_and_run_atomically() {
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
    let original_message = add_user_message(&db, &conversation.id, "Continue this work");
    let turn = db
        .create_conversation_turn(&conversation.id, &original_message.id, Some("chat"))
        .unwrap();
    let run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &original_message.id,
            "Checkpoint resume",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
    let checkpoint = pause_task_run_with_checkpoint(&db, &run.id, "user_pause");
    {
        let conn = db.conn();
        conn.execute(
                "UPDATE conversation_turns SET status = 'paused', finished_at = datetime('now') WHERE id = ?1",
                [&turn.id],
            )
            .unwrap();
        conn.execute(
            "UPDATE agent_task_runs SET finished_at = datetime('now') WHERE id = ?1",
            [&run.id],
        )
        .unwrap();
    }
    let response = unpersisted_user_message(&conversation.id, &checkpoint.resume_prompt);

    let launch = db
        .resume_agent_turn_from_checkpoint(
            &response,
            Some("anthropic"),
            Some("claude-sonnet-4"),
            "checkpoint-launch-1",
            &checkpoint.id,
        )
        .unwrap();

    assert_eq!(launch.conversation_id, conversation.id);
    assert_eq!(launch.turn_id, turn.id);
    assert_eq!(launch.run_id, run.id);
    assert_eq!(launch.user_message_id, response.id);
    assert_eq!(launch.status, "queued");
    assert!(!launch.reused);
    let resumed_run = db.get_agent_task_run(&run.id).unwrap();
    assert_eq!(resumed_run.status, "queued");
    assert_eq!(resumed_run.phase, "queued");
    assert_eq!(resumed_run.provider.as_deref(), Some("anthropic"));
    assert_eq!(resumed_run.model.as_deref(), Some("claude-sonnet-4"));
    assert_eq!(resumed_run.user_message_id, original_message.id);
    assert!(resumed_run.finished_at.is_none());
    let resumed_turn = db.get_conversation_turn(&turn.id).unwrap();
    assert_eq!(resumed_turn.status, "running");
    assert!(resumed_turn.finished_at.is_none());
    let messages = db.get_messages(&conversation.id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].id, response.id);
    assert_eq!(messages[1].content, checkpoint.resume_prompt);
    assert_eq!(
        messages[1]
            .artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.get("kind"))
            .and_then(serde_json::Value::as_str),
        Some("checkpointContinuation")
    );
    let (launch_key, response_message_id): (Option<String>, Option<String>) = db
        .conn()
        .query_row(
            "SELECT launch_idempotency_key, response_message_id
                 FROM task_resume_checkpoints WHERE id = ?1",
            [&checkpoint.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(launch_key.as_deref(), Some("checkpoint-launch-1"));
    assert_eq!(response_message_id.as_deref(), Some(response.id.as_str()));
}

#[test]
fn task_checkpoint_resume_replays_one_message_only_for_the_same_key_and_prompt() {
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
    let original_message = add_user_message(&db, &conversation.id, "Resume safely");
    let turn = db
        .create_conversation_turn(&conversation.id, &original_message.id, None)
        .unwrap();
    let run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &original_message.id,
            "Idempotent resume",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
    let checkpoint = pause_task_run_with_checkpoint(&db, &run.id, "user_pause");
    let first = unpersisted_user_message(&conversation.id, &checkpoint.resume_prompt);
    let launched = db
        .resume_agent_turn_from_checkpoint(&first, None, None, "stable-resume-key", &checkpoint.id)
        .unwrap();
    {
        let conn = db.conn();
        conn.execute(
            "UPDATE conversation_turns
                 SET status = 'paused', finished_at = NULL
                 WHERE id = ?1",
            [&turn.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE agent_task_runs
                 SET status = 'paused', phase = 'paused', finished_at = NULL
                 WHERE id = ?1",
            [&run.id],
        )
        .unwrap();
    }
    let retry = unpersisted_user_message(&conversation.id, &checkpoint.resume_prompt);
    let recovered = db
        .resume_agent_turn_from_checkpoint(&retry, None, None, "stable-resume-key", &checkpoint.id)
        .unwrap();
    assert!(!recovered.reused);
    assert_eq!(recovered.status, "queued");
    assert_eq!(recovered.user_message_id, launched.user_message_id);
    let replayed = db
        .resume_agent_turn_from_checkpoint(&retry, None, None, "stable-resume-key", &checkpoint.id)
        .unwrap();
    assert!(replayed.reused);
    assert_eq!(replayed.user_message_id, launched.user_message_id);
    assert_eq!(db.get_messages(&conversation.id).unwrap().len(), 2);

    let different_key = db.resume_agent_turn_from_checkpoint(
        &retry,
        None,
        None,
        "different-resume-key",
        &checkpoint.id,
    );
    assert!(matches!(different_key, Err(CoreError::InvalidInput(_))));
    let different_prompt = unpersisted_user_message(&conversation.id, "not the checkpoint prompt");
    let different_prompt = db.resume_agent_turn_from_checkpoint(
        &different_prompt,
        None,
        None,
        "stable-resume-key",
        &checkpoint.id,
    );
    assert!(matches!(different_prompt, Err(CoreError::InvalidInput(_))));
    assert_eq!(db.get_messages(&conversation.id).unwrap().len(), 2);
}

#[test]
fn task_checkpoint_resume_rejects_stale_checkpoints_and_terminal_runs() {
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
    let original_message = add_user_message(&db, &conversation.id, "Resume latest only");
    let turn = db
        .create_conversation_turn(&conversation.id, &original_message.id, None)
        .unwrap();
    let run = db
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &original_message.id,
            "Stale resume",
            Some("openai"),
            Some("gpt-5"),
        )
        .unwrap();
    let stale = pause_task_run_with_checkpoint(&db, &run.id, "first_pause");
    let stale_message = unpersisted_user_message(&conversation.id, &stale.resume_prompt);
    db.resume_agent_turn_from_checkpoint(&stale_message, None, None, "stale-resume-key", &stale.id)
        .unwrap();
    let latest = db
        .create_task_resume_checkpoint(&run.id, "second_pause")
        .unwrap();
    {
        let conn = db.conn();
        conn.execute(
            "UPDATE conversation_turns SET status = 'paused' WHERE id = ?1",
            [&turn.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE agent_task_runs SET status = 'paused', phase = 'paused' WHERE id = ?1",
            [&run.id],
        )
        .unwrap();
    }
    let stale_result = db.resume_agent_turn_from_checkpoint(
        &stale_message,
        None,
        None,
        "stale-resume-key",
        &stale.id,
    );
    assert!(matches!(stale_result, Err(CoreError::InvalidInput(_))));

    db.finish_agent_task_run(&run.id, "completed", Some("done"), None, None)
        .unwrap();
    let terminal_message = unpersisted_user_message(&conversation.id, &latest.resume_prompt);
    let terminal_result = db.resume_agent_turn_from_checkpoint(
        &terminal_message,
        None,
        None,
        "terminal-resume-key",
        &latest.id,
    );
    assert!(matches!(terminal_result, Err(CoreError::InvalidInput(_))));
    assert_eq!(db.get_messages(&conversation.id).unwrap().len(), 2);
}

#[test]
fn skill_governance_snapshot_surfaces_usage_and_stale_candidates() {
    let db = Database::open_memory().unwrap();
    let skill = db
        .save_skill(&crate::skills::SaveSkillInput {
            id: None,
            name: "Evidence Review".into(),
            description: "Verify claims against cited sources.".into(),
            content: "## Trigger\nUse for evidence review.\n\n## Workflow\nCheck citations."
                .to_string(),
            enabled: true,
            resource_bundle: Vec::new(),
        })
        .unwrap();
    db.record_skill_usage_event(&crate::workflow_automation::RecordSkillUsageInput {
        skill_id: skill.id.clone(),
        conversation_id: None,
        task_run_id: None,
        outcome: "failed".into(),
        evidence: serde_json::json!({ "reason": "missing citations" }),
    })
    .unwrap();
    for reason in ["bad source", "unverifiable"] {
        db.record_skill_usage_event(&crate::workflow_automation::RecordSkillUsageInput {
            skill_id: skill.id.clone(),
            conversation_id: None,
            task_run_id: None,
            outcome: "failed".into(),
            evidence: serde_json::json!({ "reason": reason }),
        })
        .unwrap();
    }
    db.record_skill_usage_event(&crate::workflow_automation::RecordSkillUsageInput {
        skill_id: "builtin-research-synthesis".into(),
        conversation_id: None,
        task_run_id: None,
        outcome: "success".into(),
        evidence: serde_json::json!({ "name": "Research Synthesis" }),
    })
    .unwrap();

    let snapshot = db.learning_governance_snapshot().unwrap();
    assert_eq!(snapshot.skill_stats.len(), 2);
    let failed = snapshot
        .skill_stats
        .iter()
        .find(|item| item.skill_id == skill.id)
        .unwrap();
    assert_eq!(failed.failure_count, 3);
    assert!(!failed.enabled);
    assert!(failed.disable_recommended);
    assert!(snapshot
        .skill_stats
        .iter()
        .any(|item| item.skill_id == "builtin-research-synthesis" && item.success_count == 1));
    assert!(snapshot
        .recommendations
        .iter()
        .any(|item| item.contains("Review")));
}

#[test]
fn investigation_graph_uses_events_artifacts_and_evidence_nodes() {
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
    let user = add_user_message(&db, &conversation.id, "Research source-backed answer");
    let turn = db
        .create_conversation_turn(&conversation.id, &user.id, Some("knowledge"))
        .unwrap();
    let run = db
        .create_agent_task_run(&conversation.id, &turn.id, &user.id, "Research", None, None)
        .unwrap();
    let plan = serde_json::json!({ "steps": [{ "id": "research", "status": "in_progress" }] });
    db.update_agent_task_run_progress(
        &run.id,
        Some("running"),
        Some("research"),
        Some("knowledge"),
        None,
        Some(&plan),
        None,
    )
    .unwrap();
    db.record_agent_task_run_event(
        &run.id,
        "tool",
        "fetch_url completed",
        Some("completed"),
        Some(&serde_json::json!({
            "tool": "fetch_url",
            "url": "https://example.com/report",
            "citation": "[cite:web:1]"
        })),
    )
    .unwrap();
    db.create_agent_task_artifact(
        &run.id,
        &crate::conversation::CreateAgentTaskArtifactInput {
            kind: "report".into(),
            title: "Brief".into(),
            summary: Some("Evidence-backed brief".into()),
            content: "Claim with [cite:web:1]".into(),
            paths: vec![],
            payload: Some(serde_json::json!({ "openQuestions": ["freshness"] })),
            source: Some("agent".into()),
        },
    )
    .unwrap();

    let graph = db.build_investigation_graph(&run.id).unwrap();
    assert!(graph.nodes.iter().any(|node| node.node_type == "source"));
    assert!(graph.nodes.iter().any(|node| node.node_type == "artifact"));
    assert!(graph.open_questions.iter().any(|item| item == "freshness"));
}

#[test]
fn browser_evidence_payload_is_source_scoped_and_auditable() {
    let payload = crate::workflow_automation::browser_evidence_payload(
        "https://example.com/report",
        "https://example.com/report",
        "Example Report",
        "Readable excerpt",
        "readable_text",
    );
    assert_eq!(payload["kind"], "browserEvidence");
    assert_eq!(payload["source"]["url"], "https://example.com/report");
    assert_eq!(payload["capture"]["method"], "readable_text");
}
