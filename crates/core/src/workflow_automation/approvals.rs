use super::*;

impl Database {
    pub fn workflow_automation_occurrence_approval_state(
        &self,

        occurrence_id: &str,
    ) -> Result<WorkflowAutomationApprovalState, CoreError> {
        let state = self
            .conn()
            .query_row(
                "SELECT state FROM workflow_automation_occurrence_approvals

                 WHERE occurrence_id = ?1",
                rusqlite::params![occurrence_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| "pending".to_string());

        WorkflowAutomationApprovalState::from_str(&state)
    }

    /// Atomically turns a claimed occurrence into one durable, actionable

    /// approval request. The definition remains enabled and the due timestamp

    /// remains fenced by the occurrence; repeated scheduler ticks observe the

    /// same waiting run instead of manufacturing a new one.

    pub fn mark_workflow_automation_run_waiting_approval(
        &self,

        run_id: &str,
    ) -> Result<bool, CoreError> {
        let mut conn = self.conn();

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let candidate = tx
            .query_row(
                "SELECT r.automation_id, r.occurrence_id, r.definition_revision

                 FROM workflow_automation_runs r

                 JOIN workflow_automations a ON a.id = r.automation_id

                 JOIN workflow_automation_schedule_configs c ON c.automation_id = r.automation_id

                 WHERE r.id = ?1 AND r.status = 'queued'

                   AND r.occurrence_id IS NOT NULL

                   AND c.revision = r.definition_revision

                   AND COALESCE(json_extract(a.approval_policy_json, '$.requireBeforeRun'), 1) = 1",
                rusqlite::params![run_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;

        let Some((automation_id, occurrence_id, definition_revision)) = candidate else {
            let _already_waiting: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workflow_automation_runs

                               WHERE id = ?1 AND status = 'waiting_approval')",
                rusqlite::params![run_id],
                |row| row.get(0),
            )?;

            tx.commit()?;

            return Ok(false);
        };

        let occurrence_updated = tx.execute(
            "UPDATE workflow_automation_occurrences

             SET status = 'waiting_approval', lease_token = NULL,

                 lease_expires_at = NULL, updated_at = datetime('now')

             WHERE id = ?1 AND status = 'claimed'",
            rusqlite::params![&occurrence_id],
        )?;

        if occurrence_updated != 1 {
            return Err(CoreError::InvalidInput(format!(
                "Workflow occurrence {occurrence_id} is not claimable for approval"
            )));
        }

        tx.execute(
            "INSERT INTO workflow_automation_occurrence_approvals

                 (occurrence_id, state, requested_at, resolved_at, updated_at)

             VALUES (?1, 'pending', datetime('now'), NULL, datetime('now'))

             ON CONFLICT(occurrence_id) DO UPDATE SET

                 state = 'pending',

                 requested_at = COALESCE(workflow_automation_occurrence_approvals.requested_at,

                                         datetime('now')),

                 resolved_at = NULL,

                 updated_at = datetime('now')",
            rusqlite::params![&occurrence_id],
        )?;

        tx.execute(
            "UPDATE workflow_automation_runs SET status = 'waiting_approval'

             WHERE id = ?1 AND status = 'queued'",
            rusqlite::params![run_id],
        )?;

        tx.execute(
            "UPDATE workflow_automations SET status = 'waiting_approval',

                 updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![&automation_id],
        )?;

        let payload = serde_json::json!({

            "occurrenceId": occurrence_id,

            "definitionRevision": definition_revision,

            "durableApproval": true,

        });

        insert_scheduler_event(
            &tx,
            SchedulerEventRecord {
                automation_id: Some(&automation_id),

                run_id: Some(run_id),

                event_type: WorkflowSchedulerEventType::ApprovalRequested,

                status: Some("waiting_approval"),

                summary: "Scheduled occurrence is waiting for pre-run approval",

                payload: Some(&payload),
            },
        )?;

        tx.commit()?;

        Ok(true)
    }

    pub fn list_workflow_automation_runs_waiting_approval(
        &self,
    ) -> Result<Vec<WorkflowAutomationRun>, CoreError> {
        let conn = self.conn();

        let mut stmt = conn.prepare(
            "SELECT r.id, r.automation_id, r.task_run_id, r.status, r.summary,

                    r.created_at, r.finished_at, r.occurrence_id, r.scheduled_for,

                    r.definition_revision, r.attempt

             FROM workflow_automation_runs r

             JOIN workflow_automation_occurrence_approvals p

               ON p.occurrence_id = r.occurrence_id

             WHERE r.status = 'waiting_approval' AND p.state = 'pending'

             ORDER BY datetime(r.created_at) ASC, r.id ASC",
        )?;

        let rows = stmt.query_map([], workflow_automation_run_from_row)?;

        let mut runs = Vec::new();

        for row in rows {
            runs.push(row?);
        }

        Ok(runs)
    }

    pub fn approve_workflow_automation_run_at(
        &self,

        run_id: &str,

        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationDueRunClaim, CoreError> {
        match self.resolve_workflow_automation_approval_at(
            run_id,
            now_rfc3339,
            WorkflowApprovalDecision::Approve,
        )? {
            WorkflowApprovalResolution::Approved(claim) => Ok(claim),

            WorkflowApprovalResolution::Denied(_) => Err(CoreError::Internal(
                "Approval transaction returned the wrong decision".into(),
            )),
        }
    }

    pub fn deny_workflow_automation_run_at(
        &self,

        run_id: &str,

        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationRun, CoreError> {
        match self.resolve_workflow_automation_approval_at(
            run_id,
            now_rfc3339,
            WorkflowApprovalDecision::Deny,
        )? {
            WorkflowApprovalResolution::Denied(run) => Ok(run),

            WorkflowApprovalResolution::Approved(_) => Err(CoreError::Internal(
                "Denial transaction returned the wrong decision".into(),
            )),
        }
    }

    fn resolve_workflow_automation_approval_at(
        &self,

        run_id: &str,

        now_rfc3339: &str,

        decision: WorkflowApprovalDecision,
    ) -> Result<WorkflowApprovalResolution, CoreError> {
        let decision_label = match decision {
            WorkflowApprovalDecision::Approve => "approval",

            WorkflowApprovalDecision::Deny => "denial",
        };

        let now = parse_utc_timestamp(now_rfc3339).ok_or_else(|| {
            CoreError::InvalidInput(format!(
                "Invalid workflow {decision_label} time '{now_rfc3339}'"
            ))
        })?;

        let mut conn = self.conn();

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let pending = fetch_pending_workflow_approval(&tx, run_id)?;

        let approval_state = match decision {
            WorkflowApprovalDecision::Approve => "approved",

            WorkflowApprovalDecision::Deny => "denied",
        };

        tx.execute(
            "UPDATE workflow_automation_occurrence_approvals

             SET state = ?2, resolved_at = datetime('now'), updated_at = datetime('now')

             WHERE occurrence_id = ?1 AND state = 'pending'",
            rusqlite::params![&pending.occurrence_id, approval_state],
        )?;

        match decision {
            WorkflowApprovalDecision::Approve => {
                let lease_token = new_id();

                let lease_expires_at = (now + Duration::minutes(2)).to_rfc3339();

                tx.execute(

                    "UPDATE workflow_automation_occurrences

                     SET status = 'claimed', lease_token = ?2, lease_expires_at = ?3,

                         updated_at = datetime('now') WHERE id = ?1 AND status = 'waiting_approval'",

                    rusqlite::params![

                        &pending.occurrence_id,

                        &lease_token,

                        &lease_expires_at

                    ],

                )?;

                tx.execute(
                    "UPDATE workflow_automation_runs SET status = 'queued'

                     WHERE id = ?1 AND status = 'waiting_approval'",
                    rusqlite::params![run_id],
                )?;

                tx.execute(

                    "UPDATE workflow_automations SET status = 'queued', updated_at = datetime('now')

                     WHERE id = ?1",

                    rusqlite::params![&pending.automation.id],

                )?;
            }

            WorkflowApprovalDecision::Deny => {
                let next_run_at =
                    if pending.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow {
                        pending.resume_next_run_at.clone()
                    } else {
                        next_run_for_trigger(
                            &pending.automation.trigger,
                            &pending.automation.schedule_config,
                            pending.automation.enabled,
                            now,
                        )?
                    };

                tx.execute(
                    "UPDATE workflow_automation_occurrences

                     SET status = 'skipped', last_error = 'pre_run_approval_denied',

                         retry_at = NULL, lease_token = NULL, lease_expires_at = NULL,

                         updated_at = datetime('now') WHERE id = ?1",
                    rusqlite::params![&pending.occurrence_id],
                )?;

                tx.execute(
                    "UPDATE workflow_automation_runs

                     SET status = 'cancelled', summary = 'Pre-run approval denied',

                         finished_at = datetime('now') WHERE id = ?1",
                    rusqlite::params![run_id],
                )?;

                tx.execute(

                    "UPDATE workflow_automations

                     SET status = CASE WHEN enabled = 1 THEN 'ready' ELSE 'disabled' END,

                         next_run_at = ?2,

                         last_run_at = CASE WHEN trigger_kind = 'folder' THEN ?3 ELSE last_run_at END,

                         updated_at = datetime('now') WHERE id = ?1",

                    rusqlite::params![&pending.automation.id, &next_run_at, now_rfc3339],

                )?;
            }
        }

        let (event_status, event_summary) = match decision {
            WorkflowApprovalDecision::Approve => {
                ("queued", "Scheduled occurrence was approved for launch")
            }

            WorkflowApprovalDecision::Deny => {
                ("cancelled", "Scheduled occurrence was denied before launch")
            }
        };

        let payload = serde_json::json!({

            "occurrenceId": pending.occurrence_id,

            "definitionRevision": pending.run.definition_revision,

            "decision": approval_state,

        });

        insert_scheduler_event(
            &tx,
            SchedulerEventRecord {
                automation_id: Some(&pending.automation.id),

                run_id: Some(run_id),

                event_type: WorkflowSchedulerEventType::ApprovalResolved,

                status: Some(event_status),

                summary: event_summary,

                payload: Some(&payload),
            },
        )?;

        let run = fetch_workflow_run(&tx, run_id)?;

        let occurrence = matches!(decision, WorkflowApprovalDecision::Approve)
            .then(|| fetch_workflow_occurrence(&tx, &pending.occurrence_id))
            .transpose()?;

        tx.commit()?;

        drop(conn);

        match decision {
            WorkflowApprovalDecision::Approve => {
                let due_reason =
                    if pending.origin == WorkflowAutomationOccurrenceOrigin::ManualRunNow {
                        "manual run requested".to_string()
                    } else {
                        pending.automation.trigger.label()
                    };

                Ok(WorkflowApprovalResolution::Approved(
                    WorkflowAutomationDueRunClaim {
                        due_run: WorkflowAutomationDueRun {
                            prompt: automation_prompt(&pending.automation),

                            due_reason,

                            scheduled_for: run.scheduled_for.clone(),

                            origin: pending.origin,

                            automation: pending.automation,
                        },

                        occurrence,

                        run: Some(run),

                        skip_reason: None,
                    },
                ))
            }

            WorkflowApprovalDecision::Deny => Ok(WorkflowApprovalResolution::Denied(run)),
        }
    }
}
