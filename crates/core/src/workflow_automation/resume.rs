use super::*;
impl Database {
    /// Atomically append the exact checkpoint prompt and re-queue its original
    /// turn/run. The checkpoint is a one-shot launch boundary: retries with
    /// the same key return the first response message, while stale checkpoints
    /// and changed launch input fail closed.
    pub fn resume_agent_turn_from_checkpoint(
        &self,
        message: &ConversationMessage,
        provider: Option<&str>,
        model: Option<&str>,
        idempotency_key: &str,
        checkpoint_id: &str,
    ) -> Result<AgentTurnLaunchRecord, CoreError> {
        let idempotency_key =
            normalize_required(idempotency_key, "Checkpoint launch idempotency key", 256)?;
        let checkpoint_id = normalize_required(checkpoint_id, "Task resume checkpoint id", 256)?;
        if message.role != Role::User {
            return Err(CoreError::InvalidInput(
                "Checkpoint response message must have the user role".to_string(),
            ));
        }
        if message.conversation_id.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "Checkpoint response conversation id cannot be empty".to_string(),
            ));
        }
        let tool_calls_json = if message.tool_calls.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&message.tool_calls)?)
        };
        let artifacts_json = Some(serde_json::to_string(&serde_json::json!({
            "kind": "checkpointContinuation",
            "version": 1,
            "checkpointId": &checkpoint_id,
        }))?);
        let image_attachments_json = message
            .image_attachments
            .as_ref()
            .filter(|attachments| !attachments.is_empty())
            .map(serde_json::to_string)
            .transpose()?;
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let checkpoint = tx
            .query_row(
                "SELECT checkpoint.run_id, checkpoint.resume_prompt,
                        checkpoint.launch_idempotency_key,
                        checkpoint.response_message_id,
                        run.conversation_id, run.turn_id, run.status
                 FROM task_resume_checkpoints checkpoint
                 JOIN agent_task_runs run ON run.id = checkpoint.run_id
                 WHERE checkpoint.id = ?1",
                [&checkpoint_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                CoreError::NotFound(format!("Task resume checkpoint {checkpoint_id}"))
            })?;
        let (
            run_id,
            resume_prompt,
            persisted_launch_key,
            response_message_id,
            conversation_id,
            turn_id,
            run_status,
        ) = checkpoint;
        if conversation_id != message.conversation_id {
            return Err(CoreError::InvalidInput(
                "Task resume checkpoint belongs to a different conversation".to_string(),
            ));
        }
        if message.content != resume_prompt {
            return Err(CoreError::InvalidInput(
                "Checkpoint response must exactly match the durable resume prompt".to_string(),
            ));
        }
        let latest_checkpoint_id: String = tx.query_row(
            "SELECT id FROM task_resume_checkpoints
             WHERE run_id = ?1
             ORDER BY datetime(created_at) DESC, rowid DESC
             LIMIT 1",
            [&run_id],
            |row| row.get(0),
        )?;
        if latest_checkpoint_id != checkpoint_id {
            return Err(CoreError::InvalidInput(format!(
                "Task resume checkpoint {checkpoint_id} is stale"
            )));
        }
        // A committed response is the idempotency record. If startup restored
        // the durable pause because the original launch never committed its
        // started marker, the same key may atomically queue that response one
        // more time without appending another transcript message.
        if let Some(response_message_id) = response_message_id {
            if persisted_launch_key.as_deref() != Some(idempotency_key.as_str()) {
                return Err(CoreError::InvalidInput(
                    "Task resume checkpoint was already launched with a different idempotency key"
                        .to_string(),
                ));
            }
            let persisted_response = tx
                .query_row(
                    "SELECT sort_order, content
                     FROM messages
                     WHERE id = ?1 AND conversation_id = ?2 AND role = 'user'",
                    rusqlite::params![&response_message_id, &conversation_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?
                .ok_or_else(|| {
                    CoreError::Internal(format!(
                        "Checkpoint {checkpoint_id} references a missing response message"
                    ))
                })?;
            if persisted_response.1 != resume_prompt {
                return Err(CoreError::Internal(format!(
                    "Checkpoint {checkpoint_id} response message no longer matches its prompt"
                )));
            }
            let replayed_after_restart = if run_status == "paused" {
                let run_updated = tx.execute(
                    "UPDATE agent_task_runs
                     SET status = 'queued', phase = 'queued',
                         summary = 'Resuming from checkpoint', error_message = NULL,
                         finished_at = NULL, updated_at = datetime('now')
                     WHERE id = ?1 AND status = 'paused'",
                    [&run_id],
                )?;
                if run_updated != 1 {
                    return Err(CoreError::InvalidInput(
                        "Task changed while its checkpoint response was being replayed".to_string(),
                    ));
                }
                let turn_updated = tx.execute(
                    "UPDATE conversation_turns
                     SET status = 'running', finished_at = NULL, updated_at = datetime('now')
                     WHERE id = ?1 AND conversation_id = ?2 AND status = 'paused'",
                    rusqlite::params![&turn_id, &conversation_id],
                )?;
                if turn_updated != 1 {
                    return Err(CoreError::InvalidInput(
                        "Conversation turn changed while its checkpoint response was being replayed"
                            .to_string(),
                    ));
                }
                tx.execute(
                    "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
                    [&conversation_id],
                )?;
                true
            } else {
                false
            };
            tx.commit()?;
            return Ok(AgentTurnLaunchRecord {
                conversation_id,
                user_message_id: response_message_id,
                user_message_sort_order: persisted_response.0,
                turn_id,
                run_id,
                status: if replayed_after_restart {
                    "queued".to_string()
                } else {
                    run_status
                },
                reused: !replayed_after_restart,
            });
        }
        if persisted_launch_key
            .as_deref()
            .is_some_and(|key| key != idempotency_key.as_str())
        {
            return Err(CoreError::InvalidInput(
                "Task resume checkpoint was already claimed by a different idempotency key"
                    .to_string(),
            ));
        }
        if run_status != "paused" {
            return Err(CoreError::InvalidInput(format!(
                "Task resume checkpoint {checkpoint_id} cannot resume task from status {run_status}"
            )));
        }
        let user_message_sort_order = tx.query_row(
            "SELECT COALESCE(MAX(sort_order), -1) + 1
             FROM messages WHERE conversation_id = ?1",
            [&conversation_id],
            |row| row.get::<_, i64>(0),
        )?;
        tx.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_call_id,
             tool_calls_json, artifacts_json, token_count, sort_order, thinking,
             image_attachments_json)
             VALUES (?1, ?2, 'user', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                &message.id,
                &conversation_id,
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
        let checkpoint_updated = tx.execute(
            "UPDATE task_resume_checkpoints
             SET launch_idempotency_key = ?2, response_message_id = ?3
             WHERE id = ?1
               AND response_message_id IS NULL
               AND (launch_idempotency_key IS NULL OR launch_idempotency_key = ?2)",
            rusqlite::params![&checkpoint_id, &idempotency_key, &message.id],
        )?;
        if checkpoint_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Task resume checkpoint changed while it was being launched".to_string(),
            ));
        }
        let run_updated = tx.execute(
            "UPDATE agent_task_runs
             SET status = 'queued', phase = 'queued',
                 summary = 'Resuming from checkpoint', error_message = NULL,
                 provider = COALESCE(?2, provider), model = COALESCE(?3, model),
                 finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND status = 'paused'",
            rusqlite::params![&run_id, provider, model],
        )?;
        if run_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Task changed while its checkpoint was being resumed".to_string(),
            ));
        }
        let turn_updated = tx.execute(
            "UPDATE conversation_turns
             SET status = 'running', finished_at = NULL, updated_at = datetime('now')
             WHERE id = ?1 AND conversation_id = ?2",
            rusqlite::params![&turn_id, &conversation_id],
        )?;
        if turn_updated != 1 {
            return Err(CoreError::InvalidInput(
                "Conversation turn changed while its checkpoint was being resumed".to_string(),
            ));
        }
        tx.execute(
            "UPDATE conversations SET updated_at = datetime('now') WHERE id = ?1",
            [&conversation_id],
        )?;
        tx.commit()?;
        Ok(AgentTurnLaunchRecord {
            conversation_id,
            user_message_id: message.id.clone(),
            user_message_sort_order,
            turn_id,
            run_id,
            status: "queued".to_string(),
            reused: false,
        })
    }

    pub fn create_task_resume_checkpoint(
        &self,
        run_id: &str,
        reason: &str,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        self.create_task_resume_checkpoint_with_state(run_id, reason, None)
    }

    pub fn create_task_resume_checkpoint_with_state(
        &self,
        run_id: &str,
        reason: &str,
        live_state: Option<&Value>,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let checkpoint = self.prepare_task_resume_checkpoint(run_id, reason, live_state)?;
        let conn = self.conn();
        Self::insert_task_resume_checkpoint_on_connection(&conn, &checkpoint)
    }

    /// Build a checkpoint without making it durable. The Run Event outbox uses
    /// this draft so the checkpoint row can share the pause event transaction.
    pub(crate) fn prepare_task_resume_checkpoint(
        &self,
        run_id: &str,
        reason: &str,
        live_state: Option<&Value>,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let run = self.get_agent_task_run(run_id)?;
        let events = self
            .get_agent_task_run_events(run_id)?
            .into_iter()
            .rev()
            .take(20)
            .collect::<Vec<_>>();
        let mut events = events;
        events.reverse();
        let artifacts = self
            .list_agent_task_artifacts(run_id)
            .unwrap_or_else(|_| Vec::new());
        let mut state = serde_json::json!({
            "run": run,
            "recentEvents": events,
            "artifacts": artifacts,
            "checkpointedAt": Utc::now().to_rfc3339(),
        });
        if let Some(partial_output) = partial_assistant_output(&self.list_agent_run_events(run_id)?)
        {
            if let Some(map) = state.as_object_mut() {
                map.insert("partialAssistantOutput".to_string(), partial_output);
            }
        }
        if let Some(live_state) = live_state {
            if let Some(map) = state.as_object_mut() {
                map.insert("liveTurnState".to_string(), live_state.clone());
            }
        }
        let run = self.get_agent_task_run(run_id)?;
        let checkpoint_id = new_id();
        let resume_prompt = build_resume_prompt(&run, &checkpoint_id, reason, &state);
        Ok(TaskResumeCheckpoint {
            id: checkpoint_id,
            run_id: run_id.to_string(),
            reason: reason.trim().to_string(),
            status: run.status,
            phase: run.phase,
            state,
            resume_prompt,
            created_at: String::new(),
        })
    }

    pub(crate) fn insert_task_resume_checkpoint_on_connection(
        connection: &rusqlite::Connection,
        checkpoint: &TaskResumeCheckpoint,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let state_json = serde_json::to_string(&checkpoint.state)?;
        connection.execute(
            "INSERT INTO task_resume_checkpoints
             (id, run_id, reason, status, phase, state_json, resume_prompt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                &checkpoint.id,
                &checkpoint.run_id,
                &checkpoint.reason,
                &checkpoint.status,
                &checkpoint.phase,
                &state_json,
                &checkpoint.resume_prompt,
            ],
        )?;
        Self::get_task_resume_checkpoint_on_connection(connection, &checkpoint.id)
    }

    pub(crate) fn get_task_resume_checkpoint_on_connection(
        connection: &rusqlite::Connection,
        checkpoint_id: &str,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        connection
            .query_row(
                "SELECT id, run_id, reason, status, phase, state_json, resume_prompt, created_at
                 FROM task_resume_checkpoints WHERE id = ?1",
                rusqlite::params![checkpoint_id],
                task_resume_checkpoint_from_row,
            )
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("Task resume checkpoint {checkpoint_id}"))
                }
                other => CoreError::Database(other),
            })
    }

    pub fn get_task_resume_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<TaskResumeCheckpoint, CoreError> {
        let conn = self.conn();
        Self::get_task_resume_checkpoint_on_connection(&conn, checkpoint_id)
    }

    pub fn latest_task_resume_checkpoint(
        &self,
        run_id: &str,
    ) -> Result<Option<TaskResumeCheckpoint>, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, run_id, reason, status, phase, state_json, resume_prompt, created_at
             FROM task_resume_checkpoints
             WHERE run_id = ?1
             ORDER BY datetime(created_at) DESC, id DESC
             LIMIT 1",
            rusqlite::params![run_id],
            task_resume_checkpoint_from_row,
        )
        .optional()
        .map_err(CoreError::Database)
    }

    pub fn list_task_resume_checkpoints(
        &self,
        run_id: &str,
    ) -> Result<Vec<TaskResumeCheckpoint>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, run_id, reason, status, phase, state_json, resume_prompt, created_at
             FROM task_resume_checkpoints
             WHERE run_id = ?1
             ORDER BY datetime(created_at) DESC, id DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![run_id], task_resume_checkpoint_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn build_task_resume_prompt(&self, run_id: &str) -> Result<TaskResumePrompt, CoreError> {
        let run = self.get_agent_task_run(run_id)?;
        let checkpoint = self
            .latest_task_resume_checkpoint(run_id)?
            .ok_or_else(|| CoreError::NotFound(format!("Resume checkpoint for task {run_id}")))?;
        Ok(TaskResumePrompt {
            run,
            prompt: checkpoint.resume_prompt.clone(),
            checkpoint,
        })
    }
}
