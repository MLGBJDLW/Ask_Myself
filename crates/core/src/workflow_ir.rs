//! Versioned workflow intermediate representation for deterministic orchestration.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::intelligence::{AgentTaskPlan, DelegationMode, EvidenceMode, PlanStepStatus};
use crate::quality_profile::ResolvedOrchestrationProfile;

pub const WORKFLOW_IR_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowNodeStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowIsolation {
    SharedReadOnly,
    ParentOwnedWrite,
    IsolatedPatchWorkspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModelRoutingClass {
    Fast,
    Strong,
    IndependentReviewer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum VerificationGateKind {
    EvidenceSufficiency,
    CitationIntegrity,
    Tests,
    Lint,
    Typecheck,
    Build,
    WriteIsolation,
    IndependentReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VerificationGate {
    pub id: String,
    pub kind: VerificationGateKind,
    pub required: bool,
    pub passed: Option<bool>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRetryPolicy {
    pub max_attempts: u8,
    pub retry_affected_nodes_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowArtifactContract {
    pub required_sections: Vec<String>,
    pub structured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDeliverable {
    pub claims: Vec<String>,
    pub evidence: Vec<String>,
    pub files_touched: Vec<String>,
    pub tests: Vec<String>,
    pub uncertainties: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNode {
    pub id: String,
    pub phase: String,
    pub title: String,
    pub dependencies: Vec<String>,
    pub parallel_group: Option<String>,
    pub model_policy: ModelRoutingClass,
    pub allowed_tools: Vec<String>,
    pub isolation: WorkflowIsolation,
    pub write_scope: Vec<String>,
    pub retry_policy: WorkflowRetryPolicy,
    pub artifact_contract: WorkflowArtifactContract,
    pub status: WorkflowNodeStatus,
    pub attempts: u8,
    pub deliverable: Option<WorkflowDeliverable>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEvidenceEntry {
    pub claim: String,
    pub status: String,
    pub source_ids: Vec<String>,
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCheckpoint {
    pub revision: u32,
    pub completed_node_ids: Vec<String>,
    pub active_node_ids: Vec<String>,
    pub failed_node_ids: Vec<String>,
    pub remaining_delegated_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCompletionContract {
    pub require_all_nodes_succeeded: bool,
    pub require_verification_gates: bool,
    pub require_evidence_ledger: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowIr {
    pub version: u8,
    pub id: String,
    pub route_kind: String,
    pub orchestration_profile: String,
    pub nexus_enabled: bool,
    pub max_parallel: u32,
    pub min_evidence_sources: u8,
    pub nodes: Vec<WorkflowNode>,
    pub verification_gates: Vec<VerificationGate>,
    pub evidence_ledger: Vec<WorkflowEvidenceEntry>,
    pub checkpoint: WorkflowCheckpoint,
    pub completion_contract: WorkflowCompletionContract,
}

impl WorkflowIr {
    pub fn task_plan_checkpoint(&self, plan: &AgentTaskPlan) -> serde_json::Value {
        let mut value = serde_json::to_value(plan)
            .unwrap_or_else(|_| serde_json::json!({ "error": "serializeTaskPlan" }));
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "workflowIr".to_string(),
                serde_json::to_value(self)
                    .unwrap_or_else(|_| serde_json::json!({ "error": "serializeWorkflowIr" })),
            );
        }
        value
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != WORKFLOW_IR_VERSION {
            return Err(format!("Unsupported workflow IR version {}.", self.version));
        }
        if self.nodes.is_empty() {
            return Err("Workflow IR must contain at least one node.".to_string());
        }
        let ids = self
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        if ids.len() != self.nodes.len() {
            return Err("Workflow IR node identifiers must be unique.".to_string());
        }
        for node in &self.nodes {
            if node
                .dependencies
                .iter()
                .any(|dependency| dependency == &node.id)
            {
                return Err(format!("Workflow node '{}' depends on itself.", node.id));
            }
            if let Some(missing) = node
                .dependencies
                .iter()
                .find(|dependency| !ids.contains(dependency.as_str()))
            {
                return Err(format!(
                    "Workflow node '{}' has missing dependency '{}'.",
                    node.id, missing
                ));
            }
        }

        let mut indegree = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node.dependencies.len()))
            .collect::<HashMap<_, _>>();
        let mut queue = indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<VecDeque<_>>();
        let mut visited = 0usize;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            for node in self
                .nodes
                .iter()
                .filter(|node| node.dependencies.iter().any(|dependency| dependency == id))
            {
                let degree = indegree
                    .get_mut(node.id.as_str())
                    .expect("validated workflow node is indexed");
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(&node.id);
                }
            }
        }
        if visited != self.nodes.len() {
            return Err("Workflow IR dependencies contain a cycle.".to_string());
        }
        Ok(())
    }

    pub fn ready_node_ids(&self) -> Vec<String> {
        let succeeded = self
            .nodes
            .iter()
            .filter(|node| node.status == WorkflowNodeStatus::Succeeded)
            .map(|node| node.id.as_str())
            .collect::<HashSet<_>>();
        self.nodes
            .iter()
            .filter(|node| {
                node.status == WorkflowNodeStatus::Pending
                    && node
                        .dependencies
                        .iter()
                        .all(|dependency| succeeded.contains(dependency.as_str()))
            })
            .take(self.max_parallel as usize)
            .map(|node| node.id.clone())
            .collect()
    }

    /// Build the first Nexus reconnaissance wave from the validated DAG.
    /// Workers are deliberately read-only: file mutations remain owned by the
    /// parent agent or an explicitly isolated patch workspace.
    pub fn reconnaissance_batch_arguments(
        &self,
        user_objective: &str,
    ) -> Option<serde_json::Value> {
        let ready = self.ready_node_ids().into_iter().collect::<HashSet<_>>();
        let nodes = self
            .nodes
            .iter()
            .filter(|node| node.phase == "reconnaissance" && ready.contains(&node.id))
            .collect::<Vec<_>>();
        if nodes.is_empty() {
            return None;
        }

        const READ_ONLY_TOOLS: &[&str] = &[
            "code_intelligence",
            "glob_files",
            "search_files",
            "read_file",
            "read_files",
            "search_knowledge_base",
            "retrieve_evidence",
            "web_search",
            "fetch_url",
            "tool_search",
        ];
        let tasks = nodes
            .iter()
            .map(|node| {
                let allowed_tools = node
                    .allowed_tools
                    .iter()
                    .filter(|tool| READ_ONLY_TOOLS.contains(&tool.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "id": node.id,
                    "task": format!("{} Do not edit files; return independent findings for the parent agent.", node.title),
                    "role": "independent repository reconnaissance specialist",
                    "model_policy": &node.model_policy,
                    "context": format!("Parent objective: {user_objective}"),
                    "expected_output": "A structured evidence report with claims, evidence, files touched, tests, and uncertainties.",
                    "acceptance_criteria": [
                        "Separate verified observations from inference.",
                        "Cite concrete files, symbols, commands, or sources.",
                        "Call out uncertainty and suggested verification."
                    ],
                    "allowed_tools": allowed_tools,
                    "parallel_group": node.parallel_group,
                    "deliverable_style": "structured evidence report",
                    "return_sections": node.artifact_contract.required_sections,
                    "max_iterations": 3,
                    "timeout_secs": 180
                })
            })
            .collect::<Vec<_>>();
        Some(serde_json::json!({
            "tasks": tasks,
            "batch_goal": format!("Independently investigate the first parallel wave for: {user_objective}"),
            "parallel_group": "workflow-ir-reconnaissance",
            "max_parallel": self.max_parallel.min(nodes.len() as u32)
        }))
    }

    /// Merge a `spawn_subagent_batch` artifact into the workflow checkpoint.
    /// Each worker maps back to a node by its stable task id so one failed
    /// branch can be retried without invalidating successful siblings.
    pub fn apply_reconnaissance_batch_result(
        &mut self,
        node_ids: &[String],
        artifacts: Option<&serde_json::Value>,
        batch_failed: bool,
        fallback_summary: &str,
    ) {
        if let Some(remaining) = artifacts
            .and_then(|value| value.pointer("/budgetAfter/remainingTokens"))
            .and_then(serde_json::Value::as_u64)
        {
            self.checkpoint.remaining_delegated_tokens = remaining.min(u32::MAX as u64) as u32;
        }
        let runs = artifacts
            .and_then(|value| value.get("runs"))
            .and_then(serde_json::Value::as_array);
        for node_id in node_ids {
            let run = runs.and_then(|runs| {
                runs.iter()
                    .find(|run| run.get("id").and_then(serde_json::Value::as_str) == Some(node_id))
            });
            let failed = batch_failed
                || run
                    .and_then(|run| run.get("isError"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(run.is_none());
            if failed {
                let detail = run
                    .and_then(|run| run.get("errorMessage").or_else(|| run.get("result")))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(fallback_summary);
                let _ = self.fail_node(node_id, detail);
                continue;
            }

            let result = run
                .and_then(|run| run.get("result"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(fallback_summary);
            let evidence = run
                .and_then(|run| run.get("evidenceHandoff"))
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    item.get("chunkId")
                        .or_else(|| item.get("path"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            let claim = result
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("Reconnaissance completed")
                .chars()
                .take(500)
                .collect::<String>();
            let _ = self.complete_node(
                node_id,
                WorkflowDeliverable {
                    claims: vec![claim],
                    evidence,
                    files_touched: Vec::new(),
                    tests: Vec::new(),
                    uncertainties: Vec::new(),
                },
            );
        }
    }

    pub fn apply_checkpoint_to_task_plan(&self, plan: &mut AgentTaskPlan) {
        for step in &mut plan.steps {
            if self.checkpoint.completed_node_ids.contains(&step.id) {
                step.status = PlanStepStatus::Completed;
            }
        }
        if !plan
            .steps
            .iter()
            .any(|step| step.status == PlanStepStatus::InProgress)
        {
            if let Some(next) = plan
                .steps
                .iter_mut()
                .find(|step| step.status == PlanStepStatus::Pending)
            {
                next.status = PlanStepStatus::InProgress;
            }
        }
    }

    pub fn sync_from_task_plan(&mut self, plan: &AgentTaskPlan) {
        let mut changed = false;
        for step in &plan.steps {
            if step.status != PlanStepStatus::Completed {
                continue;
            }
            let Some(node) = self.nodes.iter_mut().find(|node| node.id == step.id) else {
                continue;
            };
            if node.status == WorkflowNodeStatus::Succeeded {
                continue;
            }
            node.status = WorkflowNodeStatus::Succeeded;
            node.attempts = node.attempts.max(1);
            node.deliverable.get_or_insert_with(|| WorkflowDeliverable {
                claims: vec![format!("Completed task-plan step: {}", step.title)],
                evidence: Vec::new(),
                files_touched: Vec::new(),
                tests: Vec::new(),
                uncertainties: Vec::new(),
            });
            changed = true;
        }
        if changed {
            self.refresh_checkpoint();
        }
    }

    pub fn observe_tool_result(
        &mut self,
        _call_id: &str,
        tool_name: &str,
        is_error: bool,
        artifacts: Option<&serde_json::Value>,
        content: &str,
    ) {
        if tool_name == "run_shell" {
            self.record_executed_verification(is_error, artifacts);
        }
        if is_error {
            self.refresh_checkpoint();
            return;
        }
        if matches!(
            tool_name,
            "search_knowledge_base"
                | "retrieve_evidence"
                | "web_search"
                | "fetch_url"
                | "browser_evidence_capture"
        ) {
            let claim = content
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("Evidence retrieval completed")
                .chars()
                .take(500)
                .collect::<String>();
            let mut source_ids = Vec::new();
            collect_source_ids(artifacts, &mut source_ids);
            for citation in crate::cache::extract_citations(content) {
                if !source_ids.contains(&citation) {
                    source_ids.push(citation);
                }
            }
            self.evidence_ledger.push(WorkflowEvidenceEntry {
                claim,
                status: "verified".to_string(),
                source_ids,
                node_id: self
                    .checkpoint
                    .active_node_ids
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "runtime-evidence".to_string()),
            });
        }

        if tool_name == "judge_subagent_results"
            && artifacts
                .and_then(|value| value.get("kind"))
                .and_then(serde_json::Value::as_str)
                == Some("subagent_judgement")
        {
            self.record_gate("independent-review", true, content);
        }
        self.refresh_checkpoint();
    }

    pub fn observe_final_answer_audit(&mut self, audit: &serde_json::Value) {
        let status_for = |name: &str| {
            audit
                .get("checks")
                .and_then(serde_json::Value::as_array)
                .and_then(|checks| {
                    checks.iter().find(|check| {
                        check.get("name").and_then(serde_json::Value::as_str) == Some(name)
                    })
                })
                .and_then(|check| check.get("status"))
                .and_then(serde_json::Value::as_str)
        };
        if let Some(status) = status_for("Evidence requirement") {
            let citation_count = audit
                .get("citationCount")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            let explicit_insufficiency = audit
                .get("explicitInsufficiency")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            self.record_gate(
                "evidence-sufficiency",
                status == "passed"
                    && (explicit_insufficiency
                        || citation_count >= u64::from(self.min_evidence_sources.max(1))),
                format!(
                    "Final answer evidence audit: {status}; citations={citation_count}; required={}",
                    self.min_evidence_sources
                ),
            );
        }
        if let Some(status) = status_for("Citation coverage") {
            self.record_gate(
                "citation-integrity",
                status == "passed",
                format!("Final answer citation audit: {status}"),
            );
        }
    }

    pub fn completion_blockers(&self) -> Vec<String> {
        let mut blockers = self
            .nodes
            .iter()
            .filter(|node| node.status != WorkflowNodeStatus::Succeeded)
            .map(|node| format!("node:{}:{:?}", node.id, node.status))
            .collect::<Vec<_>>();
        blockers.extend(
            self.verification_gates
                .iter()
                .filter(|gate| gate.required && gate.passed != Some(true))
                .map(|gate| format!("gate:{}:{:?}", gate.id, gate.passed)),
        );
        if self.completion_contract.require_evidence_ledger
            && self
                .evidence_ledger
                .iter()
                .filter(|entry| entry.status == "verified")
                .flat_map(|entry| entry.source_ids.iter())
                .collect::<HashSet<_>>()
                .len()
                < usize::from(self.min_evidence_sources.max(1))
        {
            blockers.push(format!(
                "evidenceLedger:requires{}Sources",
                self.min_evidence_sources.max(1)
            ));
        }
        blockers
    }

    fn record_executed_verification(
        &mut self,
        is_error: bool,
        artifacts: Option<&serde_json::Value>,
    ) {
        let Some(execution) = artifacts.and_then(|value| value.get("execution")) else {
            return;
        };
        let Some(program) = execution.get("program").and_then(serde_json::Value::as_str) else {
            return;
        };
        let args = execution
            .get("args")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let program_name = std::path::Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program)
            .trim_end_matches(".exe")
            .trim_end_matches(".cmd")
            .to_ascii_lowercase();
        if program.contains('/') || program.contains('\\') {
            return;
        }
        let informational_only = args.iter().any(|argument| {
            matches!(
                argument.as_str(),
                "-h" | "--help"
                    | "-v"
                    | "--version"
                    | "--no-run"
                    | "--list"
                    | "--listtests"
                    | "--collect-only"
            )
        });
        if informational_only {
            return;
        }
        let first = args.first().map(String::as_str);
        let second = args.get(1).map(String::as_str);
        let package_script = if matches!(program_name.as_str(), "npm" | "pnpm" | "yarn" | "bun") {
            if first == Some("run") {
                second
            } else {
                first
            }
        } else {
            None
        };
        let script_is = |name: &str| {
            package_script
                .is_some_and(|script| script == name || script.starts_with(&format!("{name}:")))
        };
        let npx_tool = (program_name == "npx").then_some(first).flatten();
        let mut gate_ids = Vec::new();
        if matches!(program_name.as_str(), "pytest" | "vitest" | "jest")
            || (program_name == "playwright" && first == Some("test"))
            || (program_name == "cargo" && first == Some("test"))
            || (matches!(program_name.as_str(), "go" | "dotnet") && first == Some("test"))
            || (program_name == "node" && first == Some("--test"))
            || script_is("test")
            || matches!(npx_tool, Some("pytest" | "vitest" | "jest"))
            || (npx_tool == Some("playwright") && second == Some("test"))
        {
            gate_ids.push("tests");
        }
        if program_name == "eslint"
            || (program_name == "ruff" && first == Some("check"))
            || (program_name == "cargo" && first == Some("clippy"))
            || script_is("lint")
            || npx_tool == Some("eslint")
            || (npx_tool == Some("ruff") && second == Some("check"))
        {
            gate_ids.push("lint");
        }
        if matches!(program_name.as_str(), "tsc" | "mypy" | "pyright")
            || (program_name == "cargo" && first == Some("check"))
            || script_is("typecheck")
            || script_is("type-check")
            || matches!(npx_tool, Some("tsc" | "mypy" | "pyright"))
        {
            gate_ids.push("typecheck");
        }
        if (program_name == "cargo" && first == Some("build"))
            || (matches!(program_name.as_str(), "go" | "dotnet") && first == Some("build"))
            || script_is("build")
            || script_is("compile")
            || script_is("package")
            || (program_name == "cmake" && first == Some("--build"))
            || (program_name == "make" && (first.is_none() || first == Some("all")))
        {
            gate_ids.push("build");
        }
        let exit_code = execution
            .get("exitCode")
            .and_then(serde_json::Value::as_i64);
        let timed_out = execution
            .get("timedOut")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let passed = !is_error && exit_code == Some(0) && !timed_out;
        let command = format!("{} {}", program_name, args.join(" "));
        for gate_id in gate_ids {
            self.record_gate(
                gate_id,
                passed,
                format!(
                    "Runtime command `{}` exited {:?}.",
                    command.trim(),
                    exit_code
                ),
            );
        }
    }

    pub fn start_node(&mut self, node_id: &str) -> Result<(), String> {
        if !self.ready_node_ids().iter().any(|id| id == node_id) {
            return Err(format!("Workflow node '{node_id}' is not ready."));
        }
        let node = self
            .nodes
            .iter_mut()
            .find(|node| node.id == node_id)
            .expect("ready workflow node exists");
        node.status = WorkflowNodeStatus::Running;
        node.attempts = node.attempts.saturating_add(1);
        self.refresh_checkpoint();
        Ok(())
    }

    pub fn complete_node(
        &mut self,
        node_id: &str,
        deliverable: WorkflowDeliverable,
    ) -> Result<(), String> {
        let node = self
            .nodes
            .iter_mut()
            .find(|node| node.id == node_id)
            .ok_or_else(|| format!("Unknown workflow node '{node_id}'."))?;
        if node.status != WorkflowNodeStatus::Running {
            return Err(format!("Workflow node '{node_id}' is not running."));
        }
        for claim in &deliverable.claims {
            self.evidence_ledger.push(WorkflowEvidenceEntry {
                claim: claim.clone(),
                status: if deliverable.evidence.is_empty() {
                    "unknown".to_string()
                } else {
                    "verified".to_string()
                },
                source_ids: deliverable.evidence.clone(),
                node_id: node_id.to_string(),
            });
        }
        node.deliverable = Some(deliverable);
        node.status = WorkflowNodeStatus::Succeeded;
        self.refresh_checkpoint();
        Ok(())
    }

    pub fn fail_node(&mut self, node_id: &str, detail: &str) -> Result<bool, String> {
        let node = self
            .nodes
            .iter_mut()
            .find(|node| node.id == node_id)
            .ok_or_else(|| format!("Unknown workflow node '{node_id}'."))?;
        if node.status != WorkflowNodeStatus::Running {
            return Err(format!("Workflow node '{node_id}' is not running."));
        }
        let retry = node.attempts < node.retry_policy.max_attempts;
        node.status = if retry {
            WorkflowNodeStatus::Pending
        } else {
            WorkflowNodeStatus::Failed
        };
        node.deliverable = Some(WorkflowDeliverable {
            claims: Vec::new(),
            evidence: Vec::new(),
            files_touched: Vec::new(),
            tests: Vec::new(),
            uncertainties: vec![detail.to_string()],
        });
        self.refresh_checkpoint();
        Ok(retry)
    }

    pub fn record_gate(&mut self, gate_id: &str, passed: bool, detail: impl Into<String>) {
        if let Some(gate) = self
            .verification_gates
            .iter_mut()
            .find(|gate| gate.id == gate_id)
        {
            gate.passed = Some(passed);
            gate.detail = Some(detail.into());
        }
        self.refresh_checkpoint();
    }

    pub fn completion_allowed(&self) -> bool {
        let nodes_pass = !self.completion_contract.require_all_nodes_succeeded
            || self
                .nodes
                .iter()
                .all(|node| node.status == WorkflowNodeStatus::Succeeded);
        let gates_pass = !self.completion_contract.require_verification_gates
            || self
                .verification_gates
                .iter()
                .filter(|gate| gate.required)
                .all(|gate| gate.passed == Some(true));
        let evidence_pass = !self.completion_contract.require_evidence_ledger
            || self
                .evidence_ledger
                .iter()
                .filter(|entry| entry.status == "verified")
                .flat_map(|entry| entry.source_ids.iter())
                .collect::<HashSet<_>>()
                .len()
                >= usize::from(self.min_evidence_sources.max(1));
        nodes_pass && gates_pass && evidence_pass
    }

    pub fn requires_runtime_write_isolation(&self) -> bool {
        self.verification_gates
            .iter()
            .any(|gate| gate.required && gate.kind == VerificationGateKind::WriteIsolation)
    }

    pub fn ready_to_promote_isolated_writes(&self) -> bool {
        let nodes_pass = !self.completion_contract.require_all_nodes_succeeded
            || self
                .nodes
                .iter()
                .all(|node| node.status == WorkflowNodeStatus::Succeeded);
        let other_gates_pass = self
            .verification_gates
            .iter()
            .filter(|gate| gate.required && gate.kind != VerificationGateKind::WriteIsolation)
            .all(|gate| gate.passed == Some(true));
        let evidence_pass = !self.completion_contract.require_evidence_ledger
            || self
                .evidence_ledger
                .iter()
                .filter(|entry| entry.status == "verified")
                .flat_map(|entry| entry.source_ids.iter())
                .collect::<HashSet<_>>()
                .len()
                >= usize::from(self.min_evidence_sources.max(1));
        nodes_pass && other_gates_pass && evidence_pass
    }

    pub fn record_runtime_write_isolation(&mut self, passed: bool, detail: impl Into<String>) {
        self.record_gate("write-isolation", passed, detail);
    }

    pub fn to_prompt_section(&self) -> String {
        let workflow = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        format!(
            "## Runtime Workflow IR\n\n\
             The runtime compiled and validated this versioned DAG. Follow node dependencies, return structured deliverables, and do not bypass required verification gates. Runtime checkpoints and retry limits are authoritative.\n\n\
             ```json\n{workflow}\n```"
        )
    }

    fn refresh_checkpoint(&mut self) {
        self.checkpoint.revision = self.checkpoint.revision.saturating_add(1);
        self.checkpoint.completed_node_ids = self
            .nodes
            .iter()
            .filter(|node| node.status == WorkflowNodeStatus::Succeeded)
            .map(|node| node.id.clone())
            .collect();
        self.checkpoint.active_node_ids = self
            .nodes
            .iter()
            .filter(|node| node.status == WorkflowNodeStatus::Running)
            .map(|node| node.id.clone())
            .collect();
        self.checkpoint.failed_node_ids = self
            .nodes
            .iter()
            .filter(|node| node.status == WorkflowNodeStatus::Failed)
            .map(|node| node.id.clone())
            .collect();
    }
}

pub fn compile_workflow_ir(
    plan: &AgentTaskPlan,
    profile: &ResolvedOrchestrationProfile,
    nexus_enabled: bool,
) -> Result<WorkflowIr, String> {
    let parallel_recon = (nexus_enabled || profile.profile.is_ultra())
        && plan.steps.len() > 1
        && plan_supports_parallel_reconnaissance(plan);
    let retry_policy = WorkflowRetryPolicy {
        max_attempts: profile.retry_limit.saturating_add(1),
        retry_affected_nodes_only: true,
    };
    let mut nodes = Vec::new();
    if parallel_recon {
        for (id, title) in [
            (
                "runtime-recon-surface",
                "Map the relevant files, symbols, sources, and constraints without editing.",
            ),
            (
                "runtime-recon-risk",
                "Independently inspect likely failure modes, risks, and verification strategy without editing.",
            ),
        ] {
            nodes.push(WorkflowNode {
                id: id.to_string(),
                phase: "reconnaissance".to_string(),
                title: title.to_string(),
                dependencies: Vec::new(),
                parallel_group: Some("reconnaissance".to_string()),
                model_policy: ModelRoutingClass::Fast,
                allowed_tools: vec![
                    "code_intelligence".to_string(),
                    "glob_files".to_string(),
                    "search_files".to_string(),
                    "read_file".to_string(),
                    "read_files".to_string(),
                    "search_knowledge_base".to_string(),
                    "retrieve_evidence".to_string(),
                    "web_search".to_string(),
                    "fetch_url".to_string(),
                ],
                isolation: WorkflowIsolation::SharedReadOnly,
                write_scope: Vec::new(),
                retry_policy: retry_policy.clone(),
                artifact_contract: WorkflowArtifactContract {
                    required_sections: vec![
                        "claims".to_string(),
                        "evidence".to_string(),
                        "filesTouched".to_string(),
                        "tests".to_string(),
                        "uncertainties".to_string(),
                    ],
                    structured: true,
                },
                status: WorkflowNodeStatus::Pending,
                attempts: 0,
                deliverable: None,
            });
        }
    }
    let reconnaissance_dependencies = nodes
        .iter()
        .filter(|node| node.phase == "reconnaissance")
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let mut previous_plan_node_id: Option<String> = None;
    for (index, step) in plan.steps.iter().enumerate() {
        let dependency = if index == 0 {
            reconnaissance_dependencies.clone()
        } else {
            previous_plan_node_id
                .as_ref()
                .map(|id| vec![id.clone()])
                .unwrap_or_default()
        };
        let is_last = index + 1 == plan.steps.len();
        let mutation_capable = step
            .required_tools
            .iter()
            .any(|tool| tool_may_mutate_workspace(tool));
        nodes.push(WorkflowNode {
            id: step.id.clone(),
            phase: if is_last {
                "synthesis".to_string()
            } else {
                "execution".to_string()
            },
            title: step.title.clone(),
            dependencies: dependency,
            parallel_group: None,
            model_policy: if is_last || mutation_capable {
                ModelRoutingClass::Strong
            } else {
                ModelRoutingClass::Fast
            },
            allowed_tools: step.required_tools.clone(),
            isolation: if profile.require_isolated_writes && mutation_capable {
                WorkflowIsolation::IsolatedPatchWorkspace
            } else if mutation_capable {
                WorkflowIsolation::ParentOwnedWrite
            } else {
                WorkflowIsolation::SharedReadOnly
            },
            write_scope: Vec::new(),
            retry_policy: retry_policy.clone(),
            artifact_contract: WorkflowArtifactContract {
                required_sections: vec![
                    "claims".to_string(),
                    "evidence".to_string(),
                    "filesTouched".to_string(),
                    "tests".to_string(),
                    "uncertainties".to_string(),
                ],
                structured: true,
            },
            status: WorkflowNodeStatus::Pending,
            attempts: 0,
            deliverable: None,
        });
        previous_plan_node_id = Some(step.id.clone());
    }

    let has_isolated_write_nodes = nodes
        .iter()
        .any(|node| node.isolation == WorkflowIsolation::IsolatedPatchWorkspace);

    let code_route = plan.route_kind.eq_ignore_ascii_case("CodebaseOperation");
    let mut verification_gates = vec![VerificationGate {
        id: "evidence-sufficiency".to_string(),
        kind: VerificationGateKind::EvidenceSufficiency,
        required: plan.evidence_policy.mode == EvidenceMode::Required
            || (profile.min_evidence_sources > 1 && plan.evidence_policy.allow_web),
        passed: None,
        detail: None,
    }];
    if plan.evidence_policy.require_citations {
        verification_gates.push(VerificationGate {
            id: "citation-integrity".to_string(),
            kind: VerificationGateKind::CitationIntegrity,
            required: true,
            passed: None,
            detail: None,
        });
    }
    if code_route || profile.require_isolated_writes {
        for (id, kind) in [
            ("tests", VerificationGateKind::Tests),
            ("lint", VerificationGateKind::Lint),
            ("typecheck", VerificationGateKind::Typecheck),
            ("build", VerificationGateKind::Build),
        ] {
            verification_gates.push(VerificationGate {
                id: id.to_string(),
                kind,
                required: true,
                passed: None,
                detail: None,
            });
        }
    }
    if has_isolated_write_nodes {
        verification_gates.push(VerificationGate {
            id: "write-isolation".to_string(),
            kind: VerificationGateKind::WriteIsolation,
            required: true,
            passed: None,
            detail: None,
        });
    }
    if profile.require_independent_verifier || nexus_enabled {
        verification_gates.push(VerificationGate {
            id: "independent-review".to_string(),
            kind: VerificationGateKind::IndependentReview,
            required: true,
            passed: None,
            detail: None,
        });
    }

    let mut workflow = WorkflowIr {
        version: WORKFLOW_IR_VERSION,
        id: uuid::Uuid::new_v4().to_string(),
        route_kind: plan.route_kind.clone(),
        orchestration_profile: profile.profile.as_str().to_string(),
        nexus_enabled,
        max_parallel: profile.max_parallel,
        min_evidence_sources: profile
            .min_evidence_sources
            .max(plan.evidence_policy.min_sources),
        nodes,
        verification_gates,
        evidence_ledger: Vec::new(),
        checkpoint: WorkflowCheckpoint {
            revision: 0,
            completed_node_ids: Vec::new(),
            active_node_ids: Vec::new(),
            failed_node_ids: Vec::new(),
            remaining_delegated_tokens: profile.delegated_token_budget,
        },
        completion_contract: WorkflowCompletionContract {
            require_all_nodes_succeeded: true,
            require_verification_gates: nexus_enabled || profile.require_independent_verifier,
            require_evidence_ledger: plan.evidence_policy.mode == EvidenceMode::Required,
        },
    };
    workflow.refresh_checkpoint();
    workflow.validate()?;
    Ok(workflow)
}

fn tool_may_mutate_workspace(tool: &str) -> bool {
    matches!(
        tool,
        "create_file" | "edit_file" | "multi_edit" | "project_tool" | "run_shell"
    )
}

fn plan_supports_parallel_reconnaissance(plan: &AgentTaskPlan) -> bool {
    match plan.delegation.mode {
        DelegationMode::Disabled => false,
        DelegationMode::Recommended => true,
        DelegationMode::Optional => {
            let objective = plan.objective.to_ascii_lowercase();
            let complexity_markers = [
                "complex",
                "cross-module",
                "cross module",
                "multiple",
                "compare",
                "research",
                "regression",
                "refactor",
                "audit",
                "investigate",
                "implement",
                "verify",
                "复杂",
                "多个",
                "跨模块",
                "对比",
                "研究",
                "回归",
                "重构",
                "审计",
                "调查",
                "实现",
                "验证",
            ];
            let marker_count = complexity_markers
                .iter()
                .filter(|marker| objective.contains(*marker))
                .count();
            marker_count >= 2 || plan.objective.chars().count() >= 120
        }
    }
}

fn collect_source_ids(value: Option<&serde_json::Value>, output: &mut Vec<String>) {
    let Some(value) = value else {
        return;
    };
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "chunkId" | "sourceId" | "url" | "path") {
                    if let Some(value) = value.as_str() {
                        if !value.is_empty() && !output.iter().any(|existing| existing == value) {
                            output.push(value.to_string());
                        }
                    }
                }
                collect_source_ids(Some(value), output);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_source_ids(Some(value), output);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::{build_task_plan, TaskPlanningInput};
    use crate::quality_profile::{
        resolve_orchestration_profile, OrchestrationProfile, OrchestrationProfileInput,
    };

    fn plan() -> AgentTaskPlan {
        build_task_plan(TaskPlanningInput {
            user_query: "Research and implement the change, then test it",
            route_kind: "CodebaseOperation",
            has_sources: false,
            source_scope_count: 0,
            collection_context: false,
        })
    }

    fn profile() -> ResolvedOrchestrationProfile {
        resolve_orchestration_profile(OrchestrationProfileInput {
            profile: OrchestrationProfile::CodeUltra,
            custom: None,
            max_iterations: 20,
            max_parallel: None,
            max_calls_per_turn: None,
            delegated_token_budget: None,
            verification_reserve_percent: None,
        })
    }

    #[test]
    fn compiler_builds_a_valid_parallel_dag_with_gates() {
        let workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        assert!(workflow.validate().is_ok());
        assert!(workflow.ready_node_ids().len() >= 2);
        assert!(workflow
            .verification_gates
            .iter()
            .any(|gate| gate.kind == VerificationGateKind::Tests));
        assert!(workflow
            .nodes
            .iter()
            .any(|node| node.artifact_contract.structured));
        let args = workflow
            .reconnaissance_batch_arguments("Investigate a regression")
            .expect("nexus should compile an automatic reconnaissance wave");
        assert_eq!(args["tasks"].as_array().unwrap().len(), 2);
        assert!(args["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["model_policy"] == "fast"));
        assert!(args["tasks"].as_array().unwrap().iter().all(|task| {
            !task["allowed_tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool == "run_shell" || tool == "edit_file")
        }));
    }

    #[test]
    fn batch_results_checkpoint_siblings_independently() {
        let mut workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        let ready = workflow.ready_node_ids();
        for id in &ready {
            workflow.start_node(id).unwrap();
        }
        let artifacts = serde_json::json!({
            "runs": [
                { "id": ready[0], "isError": false, "result": "Found the relevant module.", "evidenceHandoff": [{ "chunkId": "chunk-1" }] },
                { "id": ready[1], "isError": true, "errorMessage": "diagnostic timed out" }
            ]
        });
        workflow.apply_reconnaissance_batch_result(&ready, Some(&artifacts), false, "batch");
        assert_eq!(workflow.nodes[0].status, WorkflowNodeStatus::Succeeded);
        assert_eq!(workflow.nodes[1].status, WorkflowNodeStatus::Pending);
        assert_eq!(workflow.evidence_ledger[0].source_ids, vec!["chunk-1"]);
        let retry = workflow
            .reconnaissance_batch_arguments("Retry only the failed worker")
            .expect("a single failed reconnaissance node remains schedulable");
        assert_eq!(retry["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(retry["tasks"][0]["id"], ready[1]);
    }

    #[test]
    fn verification_artifacts_drive_named_release_gates() {
        let mut workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        let artifact = serde_json::json!({
            "checks": [
                { "name": "unit tests", "status": "passed" },
                { "name": "cargo clippy", "status": "passed" },
                { "name": "Typecheck", "status": "passed" },
                { "name": "production build", "status": "passed" },
                { "name": "worktree isolation", "status": "passed" },
                { "name": "independent code review", "status": "passed" }
            ]
        });
        workflow.observe_tool_result(
            "verify-1",
            "record_verification",
            false,
            Some(&artifact),
            "passed",
        );
        for kind in [
            VerificationGateKind::Tests,
            VerificationGateKind::Lint,
            VerificationGateKind::Typecheck,
            VerificationGateKind::Build,
        ] {
            assert_eq!(
                workflow
                    .verification_gates
                    .iter()
                    .find(|gate| gate.kind == kind)
                    .and_then(|gate| gate.passed),
                None,
                "model-authored verification must not pass an execution gate"
            );
        }
        assert_eq!(
            workflow
                .verification_gates
                .iter()
                .find(|gate| gate.kind == VerificationGateKind::WriteIsolation)
                .and_then(|gate| gate.passed),
            None,
            "model-authored verification must not pass a controller-owned isolation gate"
        );
        assert_eq!(
            workflow
                .verification_gates
                .iter()
                .find(|gate| gate.kind == VerificationGateKind::IndependentReview)
                .and_then(|gate| gate.passed),
            None,
            "model-authored verification must not pass a runtime reviewer gate"
        );
        workflow.observe_tool_result(
            "judge-1",
            "judge_subagent_results",
            false,
            Some(&serde_json::json!({ "kind": "subagent_judgement" })),
            "independent judge completed",
        );
        let failed_test = serde_json::json!({
            "kind": "commandExecution",
            "execution": {
                "program": "cargo",
                "args": ["test"],
                "exitCode": 1,
                "timedOut": false
            }
        });
        workflow.observe_tool_result(
            "test-failed",
            "run_shell",
            true,
            Some(&failed_test),
            "command failed",
        );
        assert_eq!(
            workflow
                .verification_gates
                .iter()
                .find(|gate| gate.kind == VerificationGateKind::Tests)
                .and_then(|gate| gate.passed),
            Some(false)
        );
        for (call_id, program, args) in [
            ("spoof-echo", "echo", vec!["test"]),
            ("spoof-path", "/tmp/workspace/cargo", vec!["test"]),
            ("spoof-help", "cargo", vec!["test", "--no-run"]),
        ] {
            let artifact = serde_json::json!({
                "kind": "commandExecution",
                "execution": {
                    "program": program,
                    "args": args,
                    "exitCode": 0,
                    "timedOut": false
                }
            });
            workflow.observe_tool_result(
                call_id,
                "run_shell",
                false,
                Some(&artifact),
                "command passed",
            );
        }
        assert_eq!(
            workflow
                .verification_gates
                .iter()
                .find(|gate| gate.kind == VerificationGateKind::Tests)
                .and_then(|gate| gate.passed),
            Some(false),
            "untrusted or non-executing lookalike commands must not pass tests"
        );
        for (call_id, program, args) in [
            ("test-1", "cargo", vec!["test"]),
            ("lint-1", "cargo", vec!["clippy"]),
            ("typecheck-1", "cargo", vec!["check"]),
            ("build-1", "cargo", vec!["build"]),
        ] {
            let artifact = serde_json::json!({
                "kind": "commandExecution",
                "execution": {
                    "program": program,
                    "args": args,
                    "exitCode": 0,
                    "timedOut": false
                }
            });
            workflow.observe_tool_result(
                call_id,
                "run_shell",
                false,
                Some(&artifact),
                "command passed",
            );
        }
        workflow.record_runtime_write_isolation(true, "controller promoted isolated patch");
        assert!(workflow
            .verification_gates
            .iter()
            .filter(|gate| gate.required)
            .all(|gate| gate.id == "evidence-sufficiency" || gate.passed == Some(true)));
        assert!(workflow
            .verification_gates
            .iter()
            .any(|gate| gate.kind == VerificationGateKind::WriteIsolation));
    }

    #[test]
    fn scheduler_retries_only_the_failed_node_and_checkpoints_progress() {
        let mut workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        let node_id = workflow.ready_node_ids()[0].clone();
        workflow.start_node(&node_id).unwrap();
        let revision = workflow.checkpoint.revision;
        assert!(workflow.fail_node(&node_id, "test failure").unwrap());
        assert!(workflow.checkpoint.revision > revision);
        assert!(workflow.ready_node_ids().contains(&node_id));
    }

    #[test]
    fn validation_rejects_cycles() {
        let mut workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        let first = workflow.nodes[0].id.clone();
        let last = workflow.nodes.last().unwrap().id.clone();
        workflow.nodes[0].dependencies = vec![last];
        assert!(workflow.validate().unwrap_err().contains("cycle"));
        assert!(!first.is_empty());
    }

    #[test]
    fn simple_optional_tasks_do_not_fan_out() {
        let simple = build_task_plan(TaskPlanningInput {
            user_query: "Rename one file",
            route_kind: "FileOperation",
            has_sources: false,
            source_scope_count: 0,
            collection_context: false,
        });
        let workflow = compile_workflow_ir(&simple, &profile(), true).unwrap();
        assert_eq!(workflow.ready_node_ids().len(), 1);
        assert!(workflow
            .reconnaissance_batch_arguments(&simple.objective)
            .is_none());
    }

    #[test]
    fn evidence_ledger_never_treats_a_tool_call_id_as_a_source() {
        let mut workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        workflow.observe_tool_result(
            "call-123",
            "web_search",
            false,
            None,
            "Verified result without a source identifier",
        );
        assert!(workflow.evidence_ledger[0].source_ids.is_empty());

        workflow.observe_tool_result(
            "call-456",
            "retrieve_evidence",
            false,
            None,
            "Supported claim [cite:chunk-real]",
        );
        assert_eq!(
            workflow.evidence_ledger[1].source_ids,
            vec!["chunk-real".to_string()]
        );
    }

    #[test]
    fn reconnaissance_nodes_never_replace_file_mutation_steps() {
        let file_plan = build_task_plan(TaskPlanningInput {
            user_query: "Investigate multiple file constraints, implement the requested edits, and verify every output across the project",
            route_kind: "FileOperation",
            has_sources: true,
            source_scope_count: 1,
            collection_context: false,
        });
        let mut workflow = compile_workflow_ir(&file_plan, &profile(), true).unwrap();
        let ready = workflow.ready_node_ids();
        assert_eq!(ready.len(), 2);
        assert!(ready.iter().all(|id| id.starts_with("runtime-recon-")));
        assert!(workflow
            .nodes
            .iter()
            .find(|node| node.id == "act")
            .is_some_and(|node| {
                node.phase == "execution"
                    && node.isolation == WorkflowIsolation::IsolatedPatchWorkspace
            }));

        for id in &ready {
            workflow.start_node(id).unwrap();
        }
        let artifacts = serde_json::json!({
            "runs": ready.iter().map(|id| serde_json::json!({
                "id": id,
                "isError": false,
                "result": "read-only findings"
            })).collect::<Vec<_>>()
        });
        workflow.apply_reconnaissance_batch_result(&ready, Some(&artifacts), false, "batch");
        let mut checkpointed_plan = file_plan.clone();
        workflow.apply_checkpoint_to_task_plan(&mut checkpointed_plan);
        assert_ne!(
            checkpointed_plan
                .steps
                .iter()
                .find(|step| step.id == "act")
                .unwrap()
                .status,
            PlanStepStatus::Completed
        );
    }
}
