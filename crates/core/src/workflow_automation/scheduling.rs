use super::*;

fn prepare_workflow_occurrence_claim(
    tx: &Transaction<'_>,
    mut due_run: WorkflowAutomationDueRun,
    mut cached_scheduled_for: String,
    now: DateTime<Utc>,
) -> Result<PreparedWorkflowOccurrenceClaim, CoreError> {
    let authoritative_automation = tx.query_row(
        &format!("{WORKFLOW_AUTOMATION_SELECT} WHERE id = ?1"),
        rusqlite::params![&due_run.automation.id],
        workflow_automation_from_row,
    )?;
    if !authoritative_automation.enabled {
        return Err(CoreError::InvalidInput(
            "Workflow occurrence was already claimed, rescheduled, or disabled".into(),
        ));
    }
    if due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow
        && !matches!(
            authoritative_automation.trigger,
            WorkflowAutomationTrigger::Schedule { .. }
        )
    {
        return Err(CoreError::InvalidInput(
            "Only scheduled definitions support durable run-now occurrences".into(),
        ));
    }
    due_run.automation = authoritative_automation;
    due_run.prompt = automation_prompt(&due_run.automation);
    due_run.due_reason = if due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow {
        "manual run requested".to_string()
    } else {
        due_run.automation.trigger.label()
    };
    let definition_revision: i64 = tx.query_row(
        "SELECT revision FROM workflow_automation_schedule_configs WHERE automation_id = ?1",
        rusqlite::params![&due_run.automation.id],
        |row| row.get(0),
    )?;
    let pending_candidate = tx
        .query_row(
            "SELECT id, automation_id, definition_revision, scheduled_for, status,
                    attempt_count, retry_at, last_error, lease_token, lease_expires_at,
                    o.created_at, o.updated_at, COALESCE(g.origin, 'schedule'),
                    g.resume_next_run_at
             FROM workflow_automation_occurrences o
             LEFT JOIN workflow_automation_occurrence_origins g ON g.occurrence_id = o.id
             WHERE o.automation_id = ?1 AND o.definition_revision = ?2
               AND o.status IN ('planned', 'claimed', 'retry_wait', 'waiting_approval')
             ORDER BY datetime(o.created_at) DESC, o.id DESC
             LIMIT 1",
            rusqlite::params![&due_run.automation.id, definition_revision],
            |row| {
                Ok((
                    workflow_automation_occurrence_from_row(row)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            },
        )
        .optional()?;
    let pending_occurrence = pending_candidate
        .map(|(occurrence, origin, resume_next_run_at)| {
            Ok::<_, CoreError>((
                occurrence,
                WorkflowAutomationOccurrenceOrigin::parse(&origin)?,
                resume_next_run_at,
            ))
        })
        .transpose()?
        .filter(|(occurrence, origin, _)| {
            *origin == due_run.origin
                && (due_run.origin == WorkflowAutomationOccurrenceOrigin::Schedule
                    || occurrence.scheduled_for == cached_scheduled_for)
        });
    if let Some((pending, origin, _)) = pending_occurrence.as_ref() {
        cached_scheduled_for = pending.scheduled_for.clone();
        due_run.origin = *origin;
    }
    if due_run.origin == WorkflowAutomationOccurrenceOrigin::Schedule
        && due_run.automation.next_run_at.as_deref() != Some(cached_scheduled_for.as_str())
        && pending_occurrence.is_none()
    {
        return Err(CoreError::InvalidInput(
            "Workflow occurrence was already claimed, rescheduled, or disabled".into(),
        ));
    }
    let cached_scheduled_at = parse_utc_timestamp(&cached_scheduled_for).ok_or_else(|| {
        CoreError::InvalidInput(format!(
            "Invalid workflow scheduled occurrence '{cached_scheduled_for}'"
        ))
    })?;
    if cached_scheduled_at > now {
        return Err(CoreError::InvalidInput(format!(
            "Workflow occurrence '{cached_scheduled_for}' is not due yet"
        )));
    }
    let scheduled_for = if let Some((pending, _, _)) = pending_occurrence.as_ref() {
        pending.scheduled_for.clone()
    } else if due_run.origin == WorkflowAutomationOccurrenceOrigin::Schedule
        && due_run.automation.schedule_config.misfire_policy
            == WorkflowScheduleMisfirePolicy::RunLatest
        && cached_scheduled_at < now
    {
        let WorkflowAutomationTrigger::Schedule { cron } = &due_run.automation.trigger else {
            return Err(CoreError::Internal(
                "A scheduled occurrence must retain a schedule trigger".into(),
            ));
        };
        latest_workflow_cron_occurrence_at_or_before(
            cron,
            &due_run.automation.schedule_config.timezone,
            cached_scheduled_at,
            now,
        )?
        .to_rfc3339()
    } else {
        cached_scheduled_for
    };
    let scheduled_at = parse_utc_timestamp(&scheduled_for).ok_or_else(|| {
        CoreError::InvalidInput(format!(
            "Invalid workflow scheduled occurrence '{scheduled_for}'"
        ))
    })?;
    due_run.scheduled_for = Some(scheduled_for.clone());
    let resume_next_run_at = pending_occurrence
        .as_ref()
        .and_then(|(_, _, resume)| resume.clone())
        .or_else(|| {
            (due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow)
                .then(|| due_run.automation.next_run_at.clone())
                .flatten()
        });
    let next_run_at = if due_run.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow {
        resume_next_run_at.clone()
    } else {
        next_run_for_trigger(
            &due_run.automation.trigger,
            &due_run.automation.schedule_config,
            due_run.automation.enabled,
            now,
        )?
    };
    let existing = if let Some((occurrence, _, _)) = pending_occurrence {
        Some(occurrence)
    } else {
        tx.query_row(
            "SELECT id, automation_id, definition_revision, scheduled_for, status,
                    attempt_count, retry_at, last_error, lease_token, lease_expires_at,
                    o.created_at, o.updated_at
             FROM workflow_automation_occurrences o
             JOIN workflow_automation_occurrence_origins g ON g.occurrence_id = o.id
             WHERE o.automation_id = ?1 AND o.definition_revision = ?2
               AND o.scheduled_for = ?3 AND g.origin = ?4",
            rusqlite::params![
                &due_run.automation.id,
                definition_revision,
                &scheduled_for,
                due_run.origin.as_str()
            ],
            workflow_automation_occurrence_from_row,
        )
        .optional()?
    };
    let occurrence_id = existing
        .as_ref()
        .map(|item| item.id.clone())
        .unwrap_or_else(new_id);
    if existing.is_none() {
        tx.execute(
            "INSERT INTO workflow_automation_occurrences
                 (id, automation_id, definition_revision, scheduled_for, status)
             VALUES (?1, ?2, ?3, ?4, 'planned')",
            rusqlite::params![
                &occurrence_id,
                &due_run.automation.id,
                definition_revision,
                &scheduled_for
            ],
        )?;
        tx.execute(
            "INSERT INTO workflow_automation_occurrence_origins
                 (occurrence_id, origin, resume_next_run_at)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![&occurrence_id, due_run.origin.as_str(), &resume_next_run_at],
        )?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO workflow_automation_occurrence_approvals
             (occurrence_id, state)
         VALUES (?1, ?2)",
        rusqlite::params![
            &occurrence_id,
            if due_run.automation.approval_policy.require_before_run {
                WorkflowAutomationApprovalState::Pending.as_str()
            } else {
                WorkflowAutomationApprovalState::NotRequired.as_str()
            }
        ],
    )?;
    Ok(PreparedWorkflowOccurrenceClaim {
        due_run,
        definition_revision,
        scheduled_for,
        scheduled_at,
        next_run_at,
        occurrence_id,
        existing,
    })
}

fn workflow_has_active_run(
    tx: &Transaction<'_>,
    automation_id: &str,
    occurrence_id: &str,
) -> Result<bool, CoreError> {
    tx.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM workflow_automation_runs
             WHERE automation_id = ?1
               AND (occurrence_id IS NULL OR occurrence_id != ?2)
               AND status IN ('queued', 'running', 'initializing', 'in_progress',
                              'waiting_approval', 'paused', 'resuming', 'cancelling')
         )",
        rusqlite::params![automation_id, occurrence_id],
        |row| row.get(0),
    )
    .map_err(CoreError::Database)
}

fn workflow_has_isolated_source_lock(
    tx: &Transaction<'_>,
    prepared: &PreparedWorkflowOccurrenceClaim,
) -> Result<bool, CoreError> {
    if prepared
        .due_run
        .automation
        .schedule_config
        .execution_policy
        .workspace_policy
        != WorkflowScheduleWorkspacePolicy::IsolatedPatch
    {
        return Ok(false);
    }
    let source_fingerprint = prepared
        .due_run
        .automation
        .schedule_config
        .execution_policy
        .source_root_fingerprint
        .as_deref()
        .ok_or_else(|| {
            CoreError::InvalidInput(
                "Isolated scheduled patch lost its canonical source fingerprint before claim"
                    .into(),
            )
        })?;
    tx.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM workflow_automation_runs r
             JOIN workflow_automation_definition_revisions d
               ON d.automation_id = r.automation_id
              AND d.revision = r.definition_revision
             WHERE r.automation_id != ?1
               AND r.status IN ('queued', 'running', 'initializing', 'in_progress',
                                'waiting_approval', 'paused', 'resuming', 'cancelling')
               AND json_extract(d.schedule_config_json,
                                '$.executionPolicy.workspacePolicy') = 'isolated_patch'
               AND json_extract(d.schedule_config_json,
                                '$.executionPolicy.sourceRootFingerprint') = ?2
         )",
        rusqlite::params![&prepared.due_run.automation.id, source_fingerprint],
        |row| row.get(0),
    )
    .map_err(CoreError::Database)
}

fn decide_workflow_occurrence_claim(
    tx: &Transaction<'_>,
    prepared: &PreparedWorkflowOccurrenceClaim,
    now: DateTime<Utc>,
) -> Result<WorkflowOccurrenceClaimDecision, CoreError> {
    if let Some(current) = prepared.existing.as_ref() {
        if current.status == WorkflowAutomationOccurrenceStatus::WaitingApproval {
            return Ok(WorkflowOccurrenceClaimDecision::Skip("waiting_approval"));
        }
        let lease_is_live = current
            .lease_expires_at
            .as_deref()
            .and_then(parse_utc_timestamp)
            .is_some_and(|expires| expires > now);
        if current.status == WorkflowAutomationOccurrenceStatus::Claimed && lease_is_live {
            return Ok(WorkflowOccurrenceClaimDecision::Skip(
                "already_claimed_live",
            ));
        }
        if matches!(
            current.status,
            WorkflowAutomationOccurrenceStatus::Running
                | WorkflowAutomationOccurrenceStatus::Completed
                | WorkflowAutomationOccurrenceStatus::Skipped
                | WorkflowAutomationOccurrenceStatus::Failed
                | WorkflowAutomationOccurrenceStatus::Cancelled
                | WorkflowAutomationOccurrenceStatus::TimedOut
                | WorkflowAutomationOccurrenceStatus::Disabled
        ) {
            tx.execute(
                "UPDATE workflow_automations
                 SET next_run_at = ?2, status = CASE WHEN status = 'queued' THEN 'ready' ELSE status END,
                     updated_at = datetime('now') WHERE id = ?1",
                rusqlite::params![&prepared.due_run.automation.id, &prepared.next_run_at],
            )?;
            return Ok(WorkflowOccurrenceClaimDecision::Skip("already_consumed"));
        }
        if current
            .retry_at
            .as_deref()
            .and_then(parse_utc_timestamp)
            .is_some_and(|retry_at| retry_at > now)
        {
            return Ok(WorkflowOccurrenceClaimDecision::Skip("retry_backoff"));
        }
    }
    if workflow_has_isolated_source_lock(tx, prepared)? {
        return Ok(WorkflowOccurrenceClaimDecision::Skip(
            "source_workspace_locked",
        ));
    }
    let active_run_exists =
        workflow_has_active_run(tx, &prepared.due_run.automation.id, &prepared.occurrence_id)?;
    if active_run_exists
        && prepared.due_run.automation.schedule_config.overlap_policy
            == WorkflowScheduleOverlapPolicy::Skip
    {
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = 'skipped', last_error = 'overlap_policy_skip',
                 retry_at = NULL, lease_token = NULL, lease_expires_at = NULL,
                 updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&prepared.occurrence_id],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET next_run_at = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&prepared.due_run.automation.id, &prepared.next_run_at],
        )?;
        return Ok(WorkflowOccurrenceClaimDecision::Skip("overlap_active"));
    }
    let attempt = prepared
        .existing
        .as_ref()
        .map_or(1, |item| item.attempt_count.saturating_add(1));
    if attempt as usize > SCHEDULER_RETRY_MAX_ATTEMPTS {
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = 'failed', lease_token = NULL, lease_expires_at = NULL,
                 updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&prepared.occurrence_id],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET next_run_at = ?2, status = 'ready', updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&prepared.due_run.automation.id, &prepared.next_run_at],
        )?;
        return Ok(WorkflowOccurrenceClaimDecision::Skip("retry_exhausted"));
    }
    if attempt > 1 {
        tx.execute(
            "UPDATE workflow_automation_runs
             SET status = 'cancelled',
                 summary = COALESCE(summary, 'Occurrence lease superseded by a newer attempt'),
                 finished_at = COALESCE(finished_at, datetime('now'))
             WHERE occurrence_id = ?1 AND status = 'queued' AND attempt < ?2",
            rusqlite::params![&prepared.occurrence_id, i64::from(attempt)],
        )?;
    }
    let misfire_expired = prepared.due_run.automation.schedule_config.misfire_policy
        == WorkflowScheduleMisfirePolicy::Skip
        && attempt == 1
        && now
            > prepared.scheduled_at
                + Duration::seconds(i64::from(
                    prepared
                        .due_run
                        .automation
                        .schedule_config
                        .misfire_grace_seconds,
                ));
    if misfire_expired {
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = 'skipped', retry_at = NULL, lease_token = NULL,
                 lease_expires_at = NULL, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&prepared.occurrence_id],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET next_run_at = ?2, status = 'ready', updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&prepared.due_run.automation.id, &prepared.next_run_at],
        )?;
        return Ok(WorkflowOccurrenceClaimDecision::Skip(
            "misfire_grace_exceeded",
        ));
    }
    Ok(WorkflowOccurrenceClaimDecision::Queue { attempt })
}

fn finish_workflow_claim_skipped(
    tx: Transaction<'_>,
    prepared: PreparedWorkflowOccurrenceClaim,
    skip_reason: &'static str,
) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
    let occurrence = fetch_workflow_occurrence(&tx, &prepared.occurrence_id)?;
    tx.commit()?;
    Ok(WorkflowAutomationDueRunClaim {
        due_run: prepared.due_run,
        occurrence: Some(occurrence),
        run: None,
        skip_reason: Some(skip_reason.to_string()),
    })
}

fn queue_workflow_occurrence_claim(
    tx: &Transaction<'_>,
    prepared: &PreparedWorkflowOccurrenceClaim,
    attempt: u32,
    lease_token: &str,
    lease_expires_at: &str,
    summary: Option<&str>,
) -> Result<(WorkflowAutomationRun, WorkflowAutomationOccurrence), CoreError> {
    tx.execute(
        "UPDATE workflow_automation_occurrences
         SET status = 'claimed', attempt_count = ?2, retry_at = NULL,
             lease_token = ?3, lease_expires_at = ?4, updated_at = datetime('now')
         WHERE id = ?1",
        rusqlite::params![
            &prepared.occurrence_id,
            i64::from(attempt),
            lease_token,
            lease_expires_at
        ],
    )?;
    let run_id = new_id();
    tx.execute(
        "INSERT INTO workflow_automation_runs
             (id, automation_id, task_run_id, status, summary, occurrence_id,
              scheduled_for, definition_revision, attempt)
         VALUES (?1, ?2, NULL, 'queued', ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            &run_id,
            &prepared.due_run.automation.id,
            summary.or(Some(prepared.due_run.due_reason.as_str())),
            &prepared.occurrence_id,
            &prepared.scheduled_for,
            prepared.definition_revision,
            i64::from(attempt)
        ],
    )?;
    tx.execute(
        "UPDATE workflow_automations
         SET status = 'queued', updated_at = datetime('now') WHERE id = ?1",
        rusqlite::params![&prepared.due_run.automation.id],
    )?;
    Ok((
        fetch_workflow_run(tx, &run_id)?,
        fetch_workflow_occurrence(tx, &prepared.occurrence_id)?,
    ))
}

impl Database {
    pub fn save_workflow_automation(
        &self,
        input: &SaveWorkflowAutomationInput,
    ) -> Result<WorkflowAutomation, CoreError> {
        self.save_workflow_automation_with_schedule_config(
            input,
            &WorkflowAutomationScheduleConfig::default(),
        )
    }

    pub fn save_workflow_automation_with_schedule_config(
        &self,
        input: &SaveWorkflowAutomationInput,
        schedule_config: &WorkflowAutomationScheduleConfig,
    ) -> Result<WorkflowAutomation, CoreError> {
        let name = normalize_required(&input.name, "Automation name", AUTOMATION_NAME_MAX_CHARS)?;
        let description = normalize_optional(&input.description, AUTOMATION_DESCRIPTION_MAX_CHARS)?;
        let workflow_template_id =
            normalize_required(&input.workflow_template_id, "Workflow template", 120)?;
        let prompt = normalize_required(
            &input.prompt,
            "Automation prompt",
            AUTOMATION_PROMPT_MAX_CHARS,
        )?;
        let source_scope = normalize_string_list(&input.source_scope);
        let trigger_json = serde_json::to_string(&input.trigger)?;
        let source_scope_json = serde_json::to_string(&source_scope)?;
        let approval_policy_json = serde_json::to_string(&input.approval_policy)?;
        let trigger_kind = input.trigger.kind();
        let next_run_at =
            next_run_for_trigger(&input.trigger, schedule_config, input.enabled, Utc::now())?;
        let schedule_config_json = serde_json::to_string(schedule_config)?;
        let enabled = if input.enabled { 1 } else { 0 };

        let id = input.id.clone().unwrap_or_else(new_id);
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM workflow_automations WHERE id = ?1)",
            rusqlite::params![&id],
            |row| row.get(0),
        )?;
        let is_schedule = matches!(&input.trigger, WorkflowAutomationTrigger::Schedule { .. });
        let previous_definition_revision = tx
            .query_row(
                "SELECT revision FROM workflow_automation_schedule_configs WHERE automation_id = ?1",
                rusqlite::params![&id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let latest_definition_revision = tx.query_row(
            "SELECT MAX(revision) FROM workflow_automation_definition_revisions
             WHERE automation_id = ?1",
            rusqlite::params![&id],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        let definition_revision = latest_definition_revision
            .map(|revision| revision.saturating_add(1))
            .unwrap_or(1);
        if exists {
            tx.execute(
                "UPDATE workflow_automations
                 SET name = ?2,
                     description = ?3,
                     workflow_template_id = ?4,
                     prompt = ?5,
                     trigger_json = ?6,
                     trigger_kind = ?7,
                     source_scope_json = ?8,
                     approval_policy_json = ?9,
                     enabled = ?10,
                     status = CASE WHEN ?10 = 1 THEN 'ready' ELSE 'disabled' END,
                     next_run_at = ?11,
                     updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![
                    &id,
                    &name,
                    &description,
                    &workflow_template_id,
                    &prompt,
                    &trigger_json,
                    trigger_kind,
                    &source_scope_json,
                    &approval_policy_json,
                    enabled,
                    &next_run_at,
                ],
            )?;
        } else {
            tx.execute(
                "INSERT INTO workflow_automations
                 (id, name, description, workflow_template_id, prompt, trigger_json,
                  trigger_kind, source_scope_json, approval_policy_json, enabled, status, next_run_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                         CASE WHEN ?10 = 1 THEN 'ready' ELSE 'disabled' END, ?11)",
                rusqlite::params![
                    &id,
                    &name,
                    &description,
                    &workflow_template_id,
                    &prompt,
                    &trigger_json,
                    trigger_kind,
                    &source_scope_json,
                    &approval_policy_json,
                    enabled,
                    &next_run_at,
                ],
            )?;
        }
        if is_schedule {
            tx.execute(
                "INSERT INTO workflow_automation_schedule_configs
                      (automation_id, config_json, revision, updated_at)
                  VALUES (?1, ?2, ?3, datetime('now'))
                  ON CONFLICT(automation_id) DO UPDATE SET
                      config_json = excluded.config_json,
                      revision = excluded.revision,
                      updated_at = datetime('now')",
                rusqlite::params![&id, &schedule_config_json, definition_revision],
            )?;
            tx.execute(
                "INSERT INTO workflow_automation_definition_revisions
                     (automation_id, revision, name, description, workflow_template_id,
                      prompt, trigger_json, trigger_kind, source_scope_json,
                      approval_policy_json, schedule_config_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    &id,
                    definition_revision,
                    &name,
                    &description,
                    &workflow_template_id,
                    &prompt,
                    &trigger_json,
                    trigger_kind,
                    &source_scope_json,
                    &approval_policy_json,
                    &schedule_config_json,
                ],
            )?;
        } else {
            tx.execute(
                "DELETE FROM workflow_automation_schedule_configs WHERE automation_id = ?1",
                rusqlite::params![&id],
            )?;
        }
        if let Some(previous_revision) = previous_definition_revision {
            tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = 'cancelled', last_error = 'definition_superseded',
                     retry_at = NULL, lease_token = NULL, lease_expires_at = NULL,
                     updated_at = datetime('now')
                 WHERE automation_id = ?1 AND definition_revision = ?2
                   AND status IN ('planned', 'claimed', 'retry_wait', 'waiting_approval')",
                rusqlite::params![&id, previous_revision],
            )?;
            tx.execute(
                "UPDATE workflow_automation_runs
                 SET status = 'cancelled',
                     summary = COALESCE(summary, 'Definition superseded before execution'),
                     finished_at = COALESCE(finished_at, datetime('now'))
                 WHERE automation_id = ?1 AND definition_revision = ?2
                   AND status IN ('queued', 'waiting_approval')",
                rusqlite::params![&id, previous_revision],
            )?;
            let payload = serde_json::json!({
                "previousDefinitionRevision": previous_revision,
                "definitionRevision": is_schedule.then_some(definition_revision),
                "resolution": "cancelled_pending_occurrences",
            });
            insert_scheduler_event(
                &tx,
                SchedulerEventRecord {
                    automation_id: Some(&id),
                    run_id: None,
                    event_type: WorkflowSchedulerEventType::DefinitionSuperseded,
                    status: Some("cancelled"),
                    summary: "Pending occurrences were cancelled because the definition changed",
                    payload: Some(&payload),
                },
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation(&id)
    }

    pub fn get_workflow_automation(&self, id: &str) -> Result<WorkflowAutomation, CoreError> {
        let conn = self.conn();
        conn.query_row(
            &format!("{WORKFLOW_AUTOMATION_SELECT} WHERE id = ?1"),
            rusqlite::params![id],
            workflow_automation_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Workflow automation {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_workflow_automations(&self) -> Result<Vec<WorkflowAutomation>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "{WORKFLOW_AUTOMATION_SELECT} ORDER BY enabled DESC, updated_at DESC, name ASC"
        ))?;
        let rows = stmt.query_map([], workflow_automation_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn set_workflow_automation_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<WorkflowAutomation, CoreError> {
        let existing = self.get_workflow_automation(id)?;
        self.save_workflow_automation_with_schedule_config(
            &SaveWorkflowAutomationInput {
                id: Some(existing.id),
                name: existing.name,
                description: existing.description,
                workflow_template_id: existing.workflow_template_id,
                prompt: existing.prompt,
                trigger: existing.trigger,
                source_scope: existing.source_scope,
                approval_policy: existing.approval_policy,
                enabled,
            },
            &existing.schedule_config,
        )
    }

    pub fn delete_workflow_automation(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "DELETE FROM workflow_automations WHERE id = ?1",
            rusqlite::params![id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Workflow automation {id}")));
        }
        Ok(())
    }

    /// Builds an immediate occurrence for a saved scheduled definition without
    /// consuming or moving its recurring cron cursor. The occurrence is still
    /// claimed by the same durable scheduler seam as timer-generated work.
    pub fn workflow_automation_run_now_due_at(
        &self,
        automation_id: &str,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationDueRun, CoreError> {
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow run-now time '{now_rfc3339}'"))
        })?;
        let automation = self.get_workflow_automation(automation_id)?;
        if !automation.enabled {
            return Err(CoreError::InvalidInput(format!(
                "Workflow automation '{automation_id}' is disabled"
            )));
        }
        if !matches!(
            automation.trigger,
            WorkflowAutomationTrigger::Schedule { .. }
        ) {
            return Err(CoreError::InvalidInput(format!(
                "Workflow automation '{automation_id}' is not scheduled"
            )));
        }
        Ok(WorkflowAutomationDueRun {
            prompt: automation_prompt(&automation),
            due_reason: "manual run requested".to_string(),
            scheduled_for: Some(now.to_rfc3339()),
            origin: WorkflowAutomationOccurrenceOrigin::ManualRunNow,
            automation,
        })
    }

    pub fn list_due_workflow_automations(
        &self,
        now_rfc3339: &str,
    ) -> Result<Vec<WorkflowAutomationDueRun>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "{WORKFLOW_AUTOMATION_SELECT}
             WHERE enabled = 1
               AND status != 'waiting_approval'
               AND trigger_kind IN ('schedule', 'folder')
               AND (
                    trigger_kind = 'folder'
                    OR (next_run_at IS NOT NULL AND next_run_at <= ?1)
                    OR EXISTS (
                        SELECT 1
                        FROM workflow_automation_occurrences o
                        JOIN workflow_automation_occurrence_origins g
                          ON g.occurrence_id = o.id
                        WHERE o.automation_id = workflow_automations.id
                          AND g.origin = 'manual_run_now'
                          AND (
                              o.status = 'planned'
                              OR (o.status = 'claimed'
                                  AND (o.lease_expires_at IS NULL OR o.lease_expires_at <= ?1))
                              OR (o.status = 'retry_wait'
                                  AND (o.retry_at IS NULL OR o.retry_at <= ?1))
                          )
                    )
               )
             ORDER BY COALESCE(next_run_at, updated_at) ASC, name ASC
             LIMIT 100"
        ))?;
        let rows = stmt.query_map(rusqlite::params![now_rfc3339], workflow_automation_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            let automation = row?;
            if !automation.enabled {
                continue;
            }
            let pending_manual = conn
                .query_row(
                    "SELECT o.scheduled_for
                     FROM workflow_automation_occurrences o
                     JOIN workflow_automation_occurrence_origins g ON g.occurrence_id = o.id
                     WHERE o.automation_id = ?1 AND g.origin = 'manual_run_now'
                       AND (
                           o.status = 'planned'
                           OR (o.status = 'claimed'
                               AND (o.lease_expires_at IS NULL OR o.lease_expires_at <= ?2))
                           OR (o.status = 'retry_wait'
                               AND (o.retry_at IS NULL OR o.retry_at <= ?2))
                       )
                     ORDER BY datetime(o.created_at) ASC, o.id ASC
                     LIMIT 1",
                    rusqlite::params![&automation.id, now_rfc3339],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let (due_reason, scheduled_for, origin) = match &automation.trigger {
                WorkflowAutomationTrigger::Schedule { .. } => {
                    if let Some(scheduled_for) = pending_manual {
                        (
                            "manual run requested".to_string(),
                            Some(scheduled_for),
                            WorkflowAutomationOccurrenceOrigin::ManualRunNow,
                        )
                    } else {
                        (
                            automation.trigger.label(),
                            automation.next_run_at.clone(),
                            WorkflowAutomationOccurrenceOrigin::Schedule,
                        )
                    }
                }
                WorkflowAutomationTrigger::Folder { .. } => {
                    if !folder_trigger_due(&automation.trigger, automation.last_run_at.as_deref())?
                    {
                        continue;
                    }
                    (
                        "folder trigger matched a new or updated file".to_string(),
                        automation.next_run_at.clone(),
                        WorkflowAutomationOccurrenceOrigin::Schedule,
                    )
                }
                WorkflowAutomationTrigger::Manual => continue,
            };
            out.push(WorkflowAutomationDueRun {
                prompt: automation_prompt(&automation),
                due_reason,
                scheduled_for,
                origin,
                automation,
            });
        }
        Ok(out)
    }

    pub fn claim_workflow_automation_due_run(
        &self,
        due_run: WorkflowAutomationDueRun,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        self.claim_workflow_automation_due_run_at(due_run, &Utc::now().to_rfc3339(), summary)
    }

    pub fn claim_workflow_automation_due_run_at(
        &self,
        due_run: WorkflowAutomationDueRun,
        now_rfc3339: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        let Some(cached_scheduled_for) = due_run.scheduled_for.clone() else {
            let run = self.record_workflow_automation_run(
                &due_run.automation.id,
                None,
                "queued",
                summary.or(Some(due_run.due_reason.as_str())),
            )?;
            return Ok(WorkflowAutomationDueRunClaim {
                due_run,
                occurrence: None,
                run: Some(run),
                skip_reason: None,
            });
        };
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow claim time '{now_rfc3339}'"))
        })?;
        let lease_token = new_id();
        let lease_expires_at = (now + Duration::minutes(2)).to_rfc3339();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prepared = prepare_workflow_occurrence_claim(&tx, due_run, cached_scheduled_for, now)?;
        match decide_workflow_occurrence_claim(&tx, &prepared, now)? {
            WorkflowOccurrenceClaimDecision::Skip(reason) => {
                finish_workflow_claim_skipped(tx, prepared, reason)
            }
            WorkflowOccurrenceClaimDecision::Queue { attempt } => {
                let (run, occurrence) = queue_workflow_occurrence_claim(
                    &tx,
                    &prepared,
                    attempt,
                    &lease_token,
                    &lease_expires_at,
                    summary,
                )?;
                tx.commit()?;
                drop(conn);
                Ok(WorkflowAutomationDueRunClaim {
                    due_run: prepared.due_run,
                    occurrence: Some(occurrence),
                    run: Some(run),
                    skip_reason: None,
                })
            }
        }
    }

    pub fn claim_due_workflow_automation_run(
        &self,
        automation_id: &str,
        now_rfc3339: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        let due_run = self
            .list_due_workflow_automations(now_rfc3339)?
            .into_iter()
            .find(|due| due.automation.id == automation_id)
            .ok_or_else(|| {
                CoreError::InvalidInput(format!(
                    "Workflow automation '{automation_id}' is not currently due."
                ))
            })?;
        self.claim_workflow_automation_due_run_at(due_run, now_rfc3339, summary)
    }

    pub fn preview_workflow_automation_prompt(&self, id: &str) -> Result<String, CoreError> {
        let automation = self.get_workflow_automation(id)?;
        Ok(automation_prompt(&automation))
    }

    pub fn record_workflow_automation_run(
        &self,
        automation_id: &str,
        task_run_id: Option<&str>,
        status: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        self.get_workflow_automation(automation_id)?;
        let status = WorkflowAutomationRunStatus::parse(status)?;
        let status = status.as_str();
        let id = new_id();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO workflow_automation_runs
             (id, automation_id, task_run_id, status, summary, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, CASE WHEN ?4 IN ('completed', 'failed', 'cancelled', 'timed_out', 'disabled') THEN datetime('now') ELSE NULL END)",
            rusqlite::params![&id, automation_id, task_run_id, status, summary],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET last_run_at = datetime('now'),
                  status = ?2,
                  updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![automation_id, status],
        )?;
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_run(&id)
    }

    pub fn start_workflow_automation_run(
        &self,
        run_id: &str,
        task_run_id: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        self.start_workflow_automation_run_at(
            run_id,
            task_run_id,
            summary,
            &Utc::now().to_rfc3339(),
        )
    }

    pub fn start_workflow_automation_run_at(
        &self,
        run_id: &str,
        task_run_id: &str,
        summary: Option<&str>,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid workflow start time '{now_rfc3339}'"))
        })?;
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = tx
            .query_row(
                &format!("{WORKFLOW_RUN_SELECT} WHERE id = ?1"),
                rusqlite::params![run_id],
                workflow_automation_run_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Workflow automation run {run_id}"))
                }
                other => CoreError::Database(other),
            })?;
        let current_state = crate::task_orchestrator::project_task_status(run.status.as_str())
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        crate::task_orchestrator::validate_task_transition(
            current_state,
            crate::task_orchestrator::TaskOrchestratorState::Running,
        )
        .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        if let Some(existing_task_run_id) = run.task_run_id.as_deref() {
            if existing_task_run_id != task_run_id {
                return Err(CoreError::InvalidInput(format!(
                    "Workflow automation run {run_id} is already bound to task run {existing_task_run_id}"
                )));
            }
        }
        if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let (occurrence_status, current_attempt, lease_token, lease_expires_at): (
                String,
                i64,
                Option<String>,
                Option<String>,
            ) = tx
                .query_row(
                    "SELECT status, attempt_count, lease_token, lease_expires_at
                     FROM workflow_automation_occurrences WHERE id = ?1",
                    rusqlite::params![occurrence_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!(
                        "Workflow automation occurrence {occurrence_id}"
                    )),
                    other => CoreError::Database(other),
                })?;
            let lease_is_live = lease_expires_at
                .as_deref()
                .and_then(parse_utc_timestamp)
                .is_some_and(|expires_at| expires_at > now);
            let attempt_is_authoritative = occurrence_status == "claimed"
                && current_attempt == i64::from(run.attempt)
                && lease_token
                    .as_deref()
                    .is_some_and(|token| !token.is_empty())
                && lease_is_live;
            if !attempt_is_authoritative {
                tx.execute(
                    "UPDATE workflow_automation_runs
                     SET status = 'cancelled',
                         summary = COALESCE(summary, 'Occurrence claim was superseded or expired'),
                         finished_at = COALESCE(finished_at, datetime('now'))
                     WHERE id = ?1 AND status = 'queued'",
                    rusqlite::params![run_id],
                )?;
                tx.commit()?;
                return Err(CoreError::InvalidInput(format!(
                    "Workflow automation run {run_id} no longer owns the occurrence claim"
                )));
            }
        }
        let task_run_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agent_task_runs WHERE id = ?1)",
            rusqlite::params![task_run_id],
            |row| row.get(0),
        )?;
        if !task_run_exists {
            return Err(CoreError::NotFound(format!("Agent task run {task_run_id}")));
        }
        let next_run_at = if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let (origin, resume_next_run_at): (String, Option<String>) = tx.query_row(
                "SELECT origin, resume_next_run_at
                 FROM workflow_automation_occurrence_origins WHERE occurrence_id = ?1",
                rusqlite::params![occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if WorkflowAutomationOccurrenceOrigin::parse(&origin)?
                == WorkflowAutomationOccurrenceOrigin::ManualRunNow
            {
                resume_next_run_at
            } else {
                let (trigger_json, enabled, schedule_config_json): (String, i64, Option<String>) =
                    tx.query_row(
                        "SELECT automation.trigger_json, automation.enabled, schedule.config_json
                     FROM workflow_automations automation
                     LEFT JOIN workflow_automation_schedule_configs schedule
                       ON schedule.automation_id = automation.id
                     WHERE automation.id = ?1",
                        rusqlite::params![&run.automation_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(|error| match error {
                        rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!(
                            "Workflow automation {}",
                            run.automation_id
                        )),
                        other => CoreError::Database(other),
                    })?;
                let trigger = serde_json::from_str(&trigger_json)?;
                let schedule_config = schedule_config_json
                    .as_deref()
                    .map(serde_json::from_str)
                    .transpose()?
                    .unwrap_or_default();
                next_run_for_trigger(&trigger, &schedule_config, enabled != 0, now)?
            }
        } else {
            None
        };

        let affected = tx.execute(
            "UPDATE workflow_automation_runs
             SET task_run_id = ?2,
                 status = 'running',
                 summary = COALESCE(?3, summary),
                 finished_at = NULL
             WHERE id = ?1",
            rusqlite::params![run_id, task_run_id, summary],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation run {run_id}"
            )));
        }
        if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let affected = tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = 'running', retry_at = NULL, lease_token = NULL,
                     lease_expires_at = NULL, updated_at = datetime('now')
                 WHERE id = ?1 AND status = 'claimed' AND attempt_count = ?2",
                rusqlite::params![occurrence_id, i64::from(run.attempt)],
            )?;
            if affected == 0 {
                return Err(CoreError::NotFound(format!(
                    "Workflow automation occurrence {occurrence_id}"
                )));
            }
        }
        let affected = tx.execute(
            "UPDATE workflow_automations
             SET status = 'running',
                  last_run_at = datetime('now'),
                  next_run_at = COALESCE(?2, next_run_at),
                  updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&run.automation_id, &next_run_at],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation {}",
                run.automation_id
            )));
        }
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_run(run_id)
    }

    pub fn mark_workflow_automation_launch_failed_for_retry(
        &self,
        run_id: &str,
        error: &str,
        now_rfc3339: &str,
    ) -> Result<Option<WorkflowAutomationOccurrence>, CoreError> {
        let run = self.get_workflow_automation_run(run_id)?;
        let Some(occurrence_id) = run.occurrence_id.as_deref() else {
            self.transition_workflow_automation_run(
                run_id,
                "cancelled",
                Some("Task Orchestrator launch failed before agent start"),
            )?;
            return Ok(None);
        };
        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Invalid workflow launch failure time '{now_rfc3339}'"
            ))
        })?;
        let error = normalize_optional(error, 2_000)?;
        let exhausted = run.attempt as usize >= SCHEDULER_RETRY_MAX_ATTEMPTS;
        let retry_at = (!exhausted).then(|| {
            let seconds = scheduler_retry_backoff_seconds(run.attempt as usize).unwrap_or(300);
            (now + Duration::seconds(seconds)).to_rfc3339()
        });
        let automation = self.get_workflow_automation(&run.automation_id)?;
        let (origin, resume_next_run_at): (String, Option<String>) = self.conn().query_row(
            "SELECT origin, resume_next_run_at
             FROM workflow_automation_occurrence_origins WHERE occurrence_id = ?1",
            rusqlite::params![occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let next_run_at = if WorkflowAutomationOccurrenceOrigin::parse(&origin)?
            == WorkflowAutomationOccurrenceOrigin::ManualRunNow
        {
            exhausted.then_some(resume_next_run_at).flatten()
        } else if exhausted {
            next_run_for_trigger(
                &automation.trigger,
                &automation.schedule_config,
                automation.enabled,
                now,
            )?
        } else {
            None
        };
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE workflow_automation_runs
             SET status = 'cancelled', summary = ?2, finished_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![run_id, &error],
        )?;
        tx.execute(
            "UPDATE workflow_automation_occurrences
             SET status = ?2, retry_at = ?3, last_error = ?4,
                 lease_token = NULL, lease_expires_at = NULL,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![
                occurrence_id,
                if exhausted { "failed" } else { "retry_wait" },
                &retry_at,
                &error
            ],
        )?;
        tx.execute(
            "UPDATE workflow_automations
             SET status = 'ready',
                 next_run_at = COALESCE(?2, next_run_at),
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&run.automation_id, &next_run_at],
        )?;
        let occurrence = fetch_workflow_occurrence(&tx, occurrence_id)?;
        tx.commit()?;
        Ok(Some(occurrence))
    }

    pub fn workflow_automation_has_active_run(
        &self,
        automation_id: &str,
    ) -> Result<bool, CoreError> {
        self.conn()
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM workflow_automation_runs
                     WHERE automation_id = ?1
                       AND task_run_id IS NOT NULL
                       AND status IN ('running', 'initializing', 'in_progress',
                                      'waiting_approval', 'paused', 'resuming', 'cancelling')
                 )",
                rusqlite::params![automation_id],
                |row| row.get(0),
            )
            .map_err(CoreError::Database)
    }

    pub fn get_workflow_automation_occurrence(
        &self,
        id: &str,
    ) -> Result<WorkflowAutomationOccurrence, CoreError> {
        self.conn()
            .query_row(
                &format!("{WORKFLOW_OCCURRENCE_SELECT} WHERE id = ?1"),
                rusqlite::params![id],
                workflow_automation_occurrence_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Workflow automation occurrence {id}"))
                }
                other => CoreError::Database(other),
            })
    }

    pub fn transition_workflow_automation_run(
        &self,
        run_id: &str,
        status: &str,
        summary: Option<&str>,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let target_status = WorkflowAutomationRunStatus::parse(status)?;
        let status = target_status.as_str();
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = tx
            .query_row(
                &format!("{WORKFLOW_RUN_SELECT} WHERE id = ?1"),
                rusqlite::params![run_id],
                workflow_automation_run_from_row,
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Workflow automation run {run_id}"))
                }
                other => CoreError::Database(other),
            })?;
        let current_state = crate::task_orchestrator::project_task_status(run.status.as_str())
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        let target_state = crate::task_orchestrator::project_task_status(status)
            .map_err(|err| CoreError::InvalidInput(err.to_string()))?
            .state;
        if current_state != target_state {
            crate::task_orchestrator::validate_task_transition(current_state, target_state)
                .map_err(|err| CoreError::InvalidInput(err.to_string()))?;
        }

        let affected = tx.execute(
            "UPDATE workflow_automation_runs
             SET status = ?2,
                 summary = COALESCE(?3, summary),
                 finished_at = CASE
                     WHEN ?2 IN ('completed', 'failed', 'cancelled', 'timed_out', 'disabled')
                     THEN COALESCE(finished_at, datetime('now'))
                     ELSE NULL
                 END
             WHERE id = ?1",
            rusqlite::params![run_id, status, summary],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation run {run_id}"
            )));
        }
        if let Some(occurrence_id) = run.occurrence_id.as_deref() {
            let affected = tx.execute(
                "UPDATE workflow_automation_occurrences
                 SET status = ?2,
                     lease_token = NULL,
                     lease_expires_at = NULL,
                     updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![occurrence_id, status],
            )?;
            if affected == 0 {
                return Err(CoreError::NotFound(format!(
                    "Workflow automation occurrence {occurrence_id}"
                )));
            }
        }
        let affected = tx.execute(
            "UPDATE workflow_automations
             SET status = ?2,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![&run.automation_id, status],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Workflow automation {}",
                run.automation_id
            )));
        }
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_run(run_id)
    }

    pub fn get_workflow_automation_run(
        &self,
        id: &str,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        let conn = self.conn();
        conn.query_row(
            &format!("{WORKFLOW_RUN_SELECT} WHERE id = ?1"),
            rusqlite::params![id],
            workflow_automation_run_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Workflow automation run {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn get_workflow_automation_run_for_task_run(
        &self,
        task_run_id: &str,
    ) -> Result<Option<WorkflowAutomationRun>, CoreError> {
        let conn = self.conn();
        conn.query_row(
            &format!(
                "{WORKFLOW_RUN_SELECT} WHERE task_run_id = ?1 ORDER BY datetime(created_at) DESC, id DESC LIMIT 1"
            ),
            rusqlite::params![task_run_id],
            workflow_automation_run_from_row,
        )
        .optional()
        .map_err(CoreError::Database)
    }
}
