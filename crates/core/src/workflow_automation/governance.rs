use super::*;

impl Database {
    pub fn record_skill_usage_event(&self, input: &RecordSkillUsageInput) -> Result<(), CoreError> {
        let skill_id = normalize_required(&input.skill_id, "Skill id", 160)?;

        let outcome = normalize_required(&input.outcome, "Skill usage outcome", 40)?;

        let evidence_json = serde_json::to_string(&input.evidence)?;

        let conn = self.conn();

        conn.execute(
            "INSERT INTO skill_usage_events

             (id, skill_id, conversation_id, task_run_id, outcome, evidence_json)

             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                new_id(),
                &skill_id,
                input.conversation_id,
                input.task_run_id,
                &outcome,
                evidence_json
            ],
        )?;

        if matches!(outcome.as_str(), "failed" | "failure" | "error") {
            let (failure_count, success_count): (i64, i64) = conn.query_row(
                "SELECT

                    SUM(CASE WHEN outcome IN ('failed', 'failure', 'error') THEN 1 ELSE 0 END),

                    SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END)

                 FROM skill_usage_events

                 WHERE skill_id = ?1",
                rusqlite::params![&skill_id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    ))
                },
            )?;

            if failure_count >= 3 && success_count == 0 {
                conn.execute(
                    "UPDATE skills

                     SET enabled = 0, updated_at = datetime('now')

                     WHERE id = ?1 AND enabled = 1",
                    rusqlite::params![&skill_id],
                )?;
            }
        }

        Ok(())
    }

    pub fn learning_governance_snapshot(&self) -> Result<LearningGovernanceSnapshot, CoreError> {
        let conn = self.conn();

        let mut stats_by_id: HashMap<String, SkillUsageStats> = HashMap::new();

        {
            let mut stmt = conn.prepare(
                "SELECT id, name, enabled

                 FROM skills

                 ORDER BY updated_at DESC",
            )?;

            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? != 0,
                ))
            })?;

            for row in rows {
                let (skill_id, name, enabled) = row?;

                stats_by_id.insert(
                    skill_id.clone(),
                    SkillUsageStats {
                        skill_id,

                        name,

                        enabled,

                        usage_count: 0,

                        success_count: 0,

                        failure_count: 0,

                        last_used_at: None,

                        recent_failure_evidence: None,

                        disable_recommended: false,
                    },
                );
            }
        }

        {
            let mut stmt = conn.prepare(

                "SELECT skill_id,

                        COUNT(id) AS usage_count,

                        SUM(CASE WHEN outcome = 'success' THEN 1 ELSE 0 END) AS success_count,

                        SUM(CASE WHEN outcome IN ('failed', 'failure', 'error') THEN 1 ELSE 0 END) AS failure_count,

                        MAX(created_at) AS last_used_at

                 FROM skill_usage_events

                 GROUP BY skill_id",

            )?;

            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as u32,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0).max(0) as u32,
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0).max(0) as u32,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;

            for row in rows {
                let (skill_id, usage_count, success_count, failure_count, last_used_at) = row?;

                let entry =
                    stats_by_id
                        .entry(skill_id.clone())
                        .or_insert_with(|| SkillUsageStats {
                            skill_id: skill_id.clone(),

                            name: skill_id,

                            enabled: true,

                            usage_count: 0,

                            success_count: 0,

                            failure_count: 0,

                            last_used_at: None,

                            recent_failure_evidence: None,

                            disable_recommended: false,
                        });

                entry.usage_count = usage_count;

                entry.success_count = success_count;

                entry.failure_count = failure_count;

                entry.last_used_at = last_used_at;

                entry.disable_recommended = failure_count >= 3 && success_count == 0;
            }
        }

        let mut failure_evidence = HashMap::new();

        {
            let mut evidence_stmt = conn.prepare(
                "SELECT skill_id, evidence_json

                 FROM skill_usage_events

                 WHERE outcome IN ('failed', 'failure', 'error')

                 ORDER BY datetime(created_at) DESC",
            )?;

            let rows = evidence_stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;

            for row in rows {
                let (skill_id, evidence_json) = row?;

                failure_evidence.entry(skill_id).or_insert_with(|| {
                    serde_json::from_str::<Value>(&evidence_json)
                        .unwrap_or_else(|_| serde_json::json!({}))
                });
            }
        }

        let mut stats = stats_by_id.into_values().collect::<Vec<_>>();

        for stat in &mut stats {
            stat.recent_failure_evidence = failure_evidence.get(&stat.skill_id).cloned();

            stat.disable_recommended = stat.failure_count >= 3 && stat.success_count == 0;
        }

        stats.sort_by(|left, right| {
            right
                .usage_count
                .cmp(&left.usage_count)
                .then_with(|| right.last_used_at.cmp(&left.last_used_at))
                .then_with(|| left.name.cmp(&right.name))
        });

        let pending_proposals = conn.query_row(
            "SELECT COUNT(*) FROM skill_change_proposals WHERE status = 'pending'",
            [],
            |row| row.get::<_, i64>(0),
        )? as u32;

        let procedural_memory_count = conn.query_row(
            "SELECT COUNT(*) FROM agent_procedural_memories",
            [],
            |row| row.get::<_, i64>(0),
        )? as u32;

        let memory_injection_count =
            conn.query_row("SELECT COUNT(*) FROM memory_injection_events", [], |row| {
                row.get::<_, i64>(0)
            })? as u32;

        let mut recommendations = Vec::new();

        let failed_skills = stats.iter().filter(|item| item.failure_count > 0).count();

        if failed_skills > 0 {
            recommendations.push(format!(
                "Review {failed_skills} skill(s) with recent failure evidence before broad reuse."
            ));
        }

        let stale_skills = stats
            .iter()
            .filter(|item| item.usage_count == 0 && item.enabled)
            .count();

        if stale_skills > 0 {
            recommendations.push(format!(

                "Consider disabling or rewriting {stale_skills} enabled skill(s) with no recorded usage."

            ));
        }

        if pending_proposals > 0 {
            recommendations.push(format!(

                "Review {pending_proposals} pending skill proposal(s) before they affect future tasks."

            ));
        }

        Ok(LearningGovernanceSnapshot {
            skill_stats: stats,

            pending_proposals,

            procedural_memory_count,

            memory_injection_count,

            recommendations,
        })
    }

    pub fn record_memory_injection_event(
        &self,

        memory_id: &str,

        conversation_id: Option<&str>,

        turn_id: Option<&str>,

        query: &str,

        reason: &str,

        score: Option<f32>,
    ) -> Result<(), CoreError> {
        let conn = self.conn();

        conn.execute(
            "INSERT INTO memory_injection_events

             (id, memory_id, conversation_id, turn_id, query, reason, score)

             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                new_id(),
                memory_id,
                conversation_id,
                turn_id,
                query.trim(),
                reason.trim(),
                score,
            ],
        )?;

        Ok(())
    }
}
