use super::*;
impl Database {
    pub fn record_workflow_automation_scheduler_event(
        &self,
        automation_id: Option<&str>,
        run_id: Option<&str>,
        event_type: WorkflowSchedulerEventType,
        status: Option<&str>,
        summary: &str,
        payload: Option<&Value>,
    ) -> Result<WorkflowAutomationSchedulerEvent, CoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = insert_scheduler_event(
            &tx,
            SchedulerEventRecord {
                automation_id,
                run_id,
                event_type,
                status,
                summary,
                payload,
            },
        )?;
        tx.commit()?;
        drop(conn);
        self.get_workflow_automation_scheduler_event(&id)
    }

    pub fn get_workflow_automation_scheduler_event(
        &self,
        id: &str,
    ) -> Result<WorkflowAutomationSchedulerEvent, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
             FROM workflow_automation_scheduler_events WHERE id = ?1",
            rusqlite::params![id],
            workflow_scheduler_event_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Workflow automation scheduler event {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_workflow_automation_scheduler_events(
        &self,
        automation_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorkflowAutomationSchedulerEvent>, CoreError> {
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn();
        let mut out = Vec::new();
        if let Some(automation_id) = automation_id {
            let mut stmt = conn.prepare(
                "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
                 FROM workflow_automation_scheduler_events
                 WHERE automation_id = ?1
                 ORDER BY datetime(created_at) DESC, id DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![automation_id, limit],
                workflow_scheduler_event_from_row,
            )?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
                 FROM workflow_automation_scheduler_events
                 ORDER BY datetime(created_at) DESC, id DESC
                 LIMIT ?1",
            )?;
            let rows =
                stmt.query_map(rusqlite::params![limit], workflow_scheduler_event_from_row)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    pub fn workflow_automation_scheduler_retry_decision(
        &self,
        automation_id: &str,
        now_rfc3339: &str,
    ) -> Result<WorkflowAutomationSchedulerRetryDecision, CoreError> {
        let automation_id = automation_id.trim();
        if automation_id.is_empty() {
            return Err(CoreError::InvalidInput(
                "Workflow automation id is required for scheduler retry decision".to_string(),
            ));
        }
        let events = self.list_workflow_automation_scheduler_events(
            Some(automation_id),
            SCHEDULER_RETRY_EVENT_LOOKBACK_LIMIT,
        )?;
        workflow_automation_scheduler_retry_decision_from_events(&events, now_rfc3339)
    }

    pub fn list_workflow_automation_scheduler_events_for_run(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkflowAutomationSchedulerEvent>, CoreError> {
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, automation_id, run_id, event_type, status, summary, payload_json, created_at
             FROM workflow_automation_scheduler_events
             WHERE run_id = ?1
             ORDER BY datetime(created_at) ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![run_id, limit],
            workflow_scheduler_event_from_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_workflow_automation_scheduler_events_for_task_run(
        &self,
        task_run_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkflowAutomationSchedulerEvent>, CoreError> {
        let task_run_id = task_run_id.trim();
        if task_run_id.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 500) as i64;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT e.id, e.automation_id, e.run_id, e.event_type, e.status, e.summary, e.payload_json, e.created_at
             FROM workflow_automation_scheduler_events e
             INNER JOIN workflow_automation_runs r ON r.id = e.run_id
             WHERE r.task_run_id = ?1
             ORDER BY datetime(e.created_at) ASC, e.id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![task_run_id, limit],
            workflow_scheduler_event_from_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}
