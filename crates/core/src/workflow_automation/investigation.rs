use super::*;
impl Database {
    pub fn build_investigation_graph(&self, run_id: &str) -> Result<InvestigationGraph, CoreError> {
        let run = self.get_agent_task_run(run_id)?;
        let events = self.get_agent_task_run_events(run_id)?;
        let artifacts = self.list_agent_task_artifacts(run_id)?;
        let persisted_artifacts = self
            .list_persisted_agent_task_artifacts(run_id)
            .unwrap_or_else(|_| Vec::new());
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut citations = BTreeSet::new();
        let mut open_questions = BTreeSet::new();
        nodes.push(InvestigationGraphNode {
            id: "question".to_string(),
            node_type: "question".to_string(),
            label: run.title.clone(),
            summary: run.summary.clone(),
            status: Some(run.status.clone()),
            source_url: None,
            created_at: Some(run.created_at.clone()),
        });
        if let Some(plan) = &run.plan {
            nodes.push(InvestigationGraphNode {
                id: "plan".to_string(),
                node_type: "plan".to_string(),
                label: "Task plan".to_string(),
                summary: Some(
                    "Route, source scope, evidence policy, and planned steps.".to_string(),
                ),
                status: Some(run.phase.clone()),
                source_url: None,
                created_at: Some(run.updated_at.clone()),
            });
            edges.push(InvestigationGraphEdge {
                from: "question".to_string(),
                to: "plan".to_string(),
                label: "planned as".to_string(),
            });
            collect_open_questions(plan, &mut open_questions);
        }
        for (index, event) in events.iter().enumerate() {
            if let Some(payload) = &event.payload {
                collect_string_field(payload, "citation", &mut citations);
                collect_string_field(payload, "cite", &mut citations);
                collect_open_questions(payload, &mut open_questions);
                if let Some(url) = payload
                    .get("url")
                    .and_then(|value| value.as_str())
                    .or_else(|| payload.get("finalUrl").and_then(|value| value.as_str()))
                {
                    let id = format!("source:{index}");
                    nodes.push(InvestigationGraphNode {
                        id: id.clone(),
                        node_type: "source".to_string(),
                        label: event.label.clone(),
                        summary: event.status.clone(),
                        status: event.status.clone(),
                        source_url: Some(url.to_string()),
                        created_at: Some(event.created_at.clone()),
                    });
                    edges.push(InvestigationGraphEdge {
                        from: "plan".to_string(),
                        to: id,
                        label: "gathered".to_string(),
                    });
                    continue;
                }
            }
            if matches!(
                event.event_type.as_str(),
                "tool" | "subtask" | "verification"
            ) {
                let id = format!("event:{index}");
                nodes.push(InvestigationGraphNode {
                    id: id.clone(),
                    node_type: event.event_type.clone(),
                    label: event.label.clone(),
                    summary: event.status.clone(),
                    status: event.status.clone(),
                    source_url: None,
                    created_at: Some(event.created_at.clone()),
                });
                edges.push(InvestigationGraphEdge {
                    from: "plan".to_string(),
                    to: id,
                    label: "recorded".to_string(),
                });
            }
        }
        for artifact in artifacts {
            collect_open_questions(&artifact.payload, &mut open_questions);
            collect_citations_from_text(&artifact.payload.to_string(), &mut citations);
            let node = artifact_to_node(&artifact);
            edges.push(InvestigationGraphEdge {
                from: "plan".to_string(),
                to: node.id.clone(),
                label: "produced".to_string(),
            });
            nodes.push(node);
        }
        for artifact in persisted_artifacts {
            if let Some(payload) = &artifact.payload {
                collect_open_questions(payload, &mut open_questions);
                collect_citations_from_text(&payload.to_string(), &mut citations);
            }
            collect_citations_from_text(&artifact.content, &mut citations);
            let node = persisted_artifact_to_node(&artifact);
            edges.push(InvestigationGraphEdge {
                from: "plan".to_string(),
                to: node.id.clone(),
                label: "saved".to_string(),
            });
            nodes.push(node);
        }
        Ok(InvestigationGraph {
            run_id: run.id,
            nodes,
            edges,
            citations: citations.into_iter().collect(),
            open_questions: open_questions.into_iter().collect(),
        })
    }

    pub fn record_browser_evidence_capture(
        &self,
        url: &str,
        final_url: &str,
        title: &str,
        excerpt: &str,
        method: &str,
    ) -> Result<BrowserEvidenceCapture, CoreError> {
        let payload = browser_evidence_payload(url, final_url, title, excerpt, method);
        let id = new_id();
        let payload_json = serde_json::to_string(&payload)?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO browser_evidence_captures
             (id, url, final_url, title, excerpt, method, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![&id, url, final_url, title, excerpt, method, payload_json],
        )?;
        drop(conn);
        self.get_browser_evidence_capture(&id)
    }

    pub fn get_browser_evidence_capture(
        &self,
        id: &str,
    ) -> Result<BrowserEvidenceCapture, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, url, final_url, title, excerpt, method, payload_json, created_at
             FROM browser_evidence_captures WHERE id = ?1",
            rusqlite::params![id],
            browser_evidence_from_row,
        )
        .map_err(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Browser evidence capture {id}"))
            }
            other => CoreError::Database(other),
        })
    }
}
