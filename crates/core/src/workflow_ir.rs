//! Versioned workflow intermediate representation for deterministic orchestration.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::intelligence::{AgentTaskPlan, DelegationMode, EvidenceMode, PlanStepStatus};
use crate::quality_profile::{OrchestrationProfile, ResolvedOrchestrationProfile};

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
    BrowserVisualObservation,
    BrowserSessionObservation,
    DesktopObservation,
}

impl VerificationGateKind {
    fn is_interaction_observation(&self) -> bool {
        matches!(
            self,
            Self::BrowserVisualObservation
                | Self::BrowserSessionObservation
                | Self::DesktopObservation
        )
    }
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
    #[serde(default)]
    pub require_interaction_gates: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectVerificationSupport {
    pub tests: bool,
    pub lint: bool,
    pub typecheck: bool,
    pub build: bool,
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
    fn require_fresh_desktop_observation(&mut self, detail: &str) {
        self.completion_contract.require_interaction_gates = true;
        ensure_required_gate(
            &mut self.verification_gates,
            "desktop-observation",
            VerificationGateKind::DesktopObservation,
            detail,
        );
        self.refresh_checkpoint();
    }

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
        call_id: &str,
        tool_name: &str,
        is_error: bool,
        artifacts: Option<&serde_json::Value>,
        content: &str,
    ) {
        self.observe_tool_result_with_arguments(
            call_id, tool_name, None, is_error, artifacts, content,
        );
    }

    pub(crate) fn observe_tool_result_with_arguments(
        &mut self,
        _call_id: &str,
        tool_name: &str,
        tool_arguments: Option<&str>,
        is_error: bool,
        artifacts: Option<&serde_json::Value>,
        content: &str,
    ) {
        if tool_name == "run_shell" {
            self.record_executed_verification(is_error, artifacts);
        }
        let requires_desktop_observation =
            tool_result_requires_desktop_observation(tool_name, is_error, artifacts);
        if requires_desktop_observation {
            self.require_fresh_desktop_observation(if is_error {
                "Computer control may have crossed its commit boundary; a fresh computer_observe screenshot is required before completion."
            } else {
                "Successful computer_control requires a fresh computer_observe screenshot before completion."
            });
        }
        if is_error {
            self.refresh_checkpoint();
            return;
        }
        if tool_may_mutate_workspace(tool_name)
            || (tool_name == "browser_session"
                && browser_session_action_invalidates_observation(tool_arguments))
        {
            self.record_gate(
                "browser-visual-observation",
                false,
                format!(
                    "Successful `{tool_name}` invalidated the previous rendered visual observation."
                ),
            );
            self.record_gate(
                "browser-session-observation",
                false,
                format!(
                    "Successful `{tool_name}` invalidated the previous browser-session observation."
                ),
            );
        }
        let verified_browser_visual_observation =
            is_verified_browser_visual_observation(tool_name, artifacts)
                && (tool_name != "browser_session"
                    || normalized_tool_action(tool_arguments).as_deref() == Some("observe"));
        if verified_browser_visual_observation {
            self.record_gate(
                "browser-visual-observation",
                true,
                format!("Fresh rendered visual observation returned by `{tool_name}`."),
            );
            if tool_name == "browser_session" {
                self.record_gate(
                    "browser-session-observation",
                    true,
                    "Fresh pixel-bearing observation returned by browser_session.",
                );
            }
        }
        if tool_name == "computer_observe"
            && is_verified_desktop_observation(tool_arguments, artifacts)
        {
            self.record_gate(
                "desktop-observation",
                true,
                "Fresh desktop observation returned by computer_observe.",
            );
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
        let mut blockers = if self.completion_contract.require_all_nodes_succeeded {
            self.nodes
                .iter()
                .filter(|node| node.status != WorkflowNodeStatus::Succeeded)
                .map(|node| format!("node:{}:{:?}", node.id, node.status))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        blockers.extend(
            self.verification_gates
                .iter()
                .filter(|gate| self.completion_gate_is_enforced(gate) && gate.passed != Some(true))
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
        let third = args.get(2).map(String::as_str);
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
        let python_module = matches!(program_name.as_str(), "python" | "python3" | "py")
            .then_some((first, second))
            .filter(|(flag, _)| *flag == Some("-m"))
            .and_then(|(_, module)| module);
        let mut gate_ids = Vec::new();
        if matches!(program_name.as_str(), "pytest" | "vitest" | "jest")
            || (program_name == "playwright" && first == Some("test"))
            || (program_name == "cargo" && first == Some("test"))
            || (matches!(program_name.as_str(), "go" | "dotnet") && first == Some("test"))
            || (program_name == "node" && first == Some("--test"))
            || matches!(python_module, Some("pytest" | "unittest"))
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
            || (python_module == Some("ruff") && third == Some("check"))
        {
            gate_ids.push("lint");
        }
        if matches!(program_name.as_str(), "tsc" | "mypy" | "pyright")
            || (program_name == "cargo" && first == Some("check"))
            || script_is("typecheck")
            || script_is("type-check")
            || matches!(npx_tool, Some("tsc" | "mypy" | "pyright"))
            || matches!(python_module, Some("mypy" | "pyright"))
        {
            gate_ids.push("typecheck");
        }
        if (program_name == "cargo" && first == Some("build"))
            || (matches!(program_name.as_str(), "go" | "dotnet") && first == Some("build"))
            || script_is("build")
            || script_is("compile")
            || script_is("package")
            || python_module == Some("build")
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
        let gates_pass = self
            .verification_gates
            .iter()
            .filter(|gate| self.completion_gate_is_enforced(gate))
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

    fn completion_gate_is_enforced(&self, gate: &VerificationGate) -> bool {
        gate.required
            && (self.completion_contract.require_verification_gates
                || (self.completion_contract.require_interaction_gates
                    && gate.kind.is_interaction_observation()))
    }

    pub fn requires_completion_audit(&self) -> bool {
        self.completion_contract.require_verification_gates
            || self.completion_contract.require_interaction_gates
            || self.completion_contract.require_evidence_ledger
    }

    pub fn completion_repair_guidance(&self) -> String {
        let mut actions = Vec::new();
        if self.verification_gates.iter().any(|gate| {
            self.completion_gate_is_enforced(gate)
                && gate.kind == VerificationGateKind::BrowserVisualObservation
                && gate.passed != Some(true)
        }) {
            actions.push(
                "serve or render the artifact and call browser_evidence_capture after the last file or process mutation",
            );
        }
        if self.verification_gates.iter().any(|gate| {
            self.completion_gate_is_enforced(gate)
                && gate.kind == VerificationGateKind::BrowserSessionObservation
                && gate.passed != Some(true)
        }) {
            actions.push(
                "call browser_session for the requested navigation or interaction and obtain its fresh screenshot-bearing observation",
            );
        }
        if self.verification_gates.iter().any(|gate| {
            self.completion_gate_is_enforced(gate)
                && gate.kind == VerificationGateKind::DesktopObservation
                && gate.passed != Some(true)
        }) {
            actions.push(
                "call computer_observe now; every successful computer_control requires another fresh computer_observe before completion",
            );
        }
        if actions.is_empty() {
            actions.push(
                "run the required checks, record exact passed or failed outcomes, and use an independent reviewer when required",
            );
        }
        format!(
            "Resolve the enforced completion contract with concrete tool results: {}. A pending, claimed, or skipped check is not success.",
            actions.join("; ")
        )
    }

    pub fn requires_runtime_write_isolation(&self) -> bool {
        self.verification_gates
            .iter()
            .any(|gate| gate.required && gate.kind == VerificationGateKind::WriteIsolation)
    }

    /// Reconciles planner output with the stronger runtime contract of an
    /// unattended isolated patch. This removes delegation-only nodes that the
    /// sandboxed schedule cannot execute and makes worktree promotion plus a
    /// controller-owned independent patch review authoritative even when the
    /// planner underpredicts mutation.
    pub fn configure_for_scheduled_isolated_patch(&mut self) {
        let removed_reconnaissance = self
            .nodes
            .iter()
            .filter(|node| node.phase == "reconnaissance")
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        self.nodes.retain(|node| node.phase != "reconnaissance");
        for node in &mut self.nodes {
            node.dependencies
                .retain(|dependency| !removed_reconnaissance.contains(dependency));
            node.allowed_tools.retain(|tool| {
                !matches!(
                    tool.as_str(),
                    "spawn_subagent" | "spawn_subagent_batch" | "judge_subagent_results"
                )
            });
        }
        ensure_required_gate(
            &mut self.verification_gates,
            "write-isolation",
            VerificationGateKind::WriteIsolation,
            "Scheduled isolated patches require controller-owned worktree promotion.",
        );
        ensure_required_gate(
            &mut self.verification_gates,
            "independent-review",
            VerificationGateKind::IndependentReview,
            "Scheduled isolated patches require the controller's non-delegating Git patch review.",
        );
        self.completion_contract.require_all_nodes_succeeded = true;
        self.completion_contract.require_verification_gates = true;
        self.refresh_checkpoint();
    }

    /// Returns true only when a controller review can be the final read of the
    /// patch before promotion. Excluding the review and promotion gates avoids
    /// circularity while preventing later repair mutations from invalidating a
    /// review that ran too early.
    pub fn ready_for_runtime_independent_review(&self) -> bool {
        let nodes_pass = !self.completion_contract.require_all_nodes_succeeded
            || self
                .nodes
                .iter()
                .all(|node| node.status == WorkflowNodeStatus::Succeeded);
        let prerequisite_gates_pass = self
            .verification_gates
            .iter()
            .filter(|gate| {
                gate.required
                    && !matches!(
                        gate.kind,
                        VerificationGateKind::IndependentReview
                            | VerificationGateKind::WriteIsolation
                    )
            })
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
        nodes_pass && prerequisite_gates_pass && evidence_pass
    }

    /// Plan Mode produces an approval handoff and never executes the compiled
    /// mutation workflow. Keep the IR as read-only planning context without
    /// requiring execution nodes, process isolation, or release gates.
    pub fn configure_for_plan_mode(&mut self) {
        for node in &mut self.nodes {
            node.phase = "planning".to_string();
            node.isolation = WorkflowIsolation::SharedReadOnly;
            node.write_scope.clear();
            node.allowed_tools.retain(|tool| {
                !tool_may_mutate_workspace(tool)
                    && !matches!(
                        tool.as_str(),
                        "spawn_subagent"
                            | "spawn_subagent_batch"
                            | "judge_subagent_results"
                            | "record_verification"
                    )
            });
        }
        self.verification_gates.clear();
        self.completion_contract = WorkflowCompletionContract {
            require_all_nodes_succeeded: false,
            require_verification_gates: false,
            require_evidence_ledger: false,
            require_interaction_gates: false,
        };
        self.refresh_checkpoint();
    }

    /// Bind execution gates to tooling detected by the controller before the
    /// model can mutate the project. Unsupported categories remain visible as
    /// controller-validated not-applicable gates and cannot deadlock a run.
    pub fn configure_project_verification_support(&mut self, support: ProjectVerificationSupport) {
        for gate in &mut self.verification_gates {
            let supported = match gate.kind {
                VerificationGateKind::Tests => Some(support.tests),
                VerificationGateKind::Lint => Some(support.lint),
                VerificationGateKind::Typecheck => Some(support.typecheck),
                VerificationGateKind::Build => Some(support.build),
                _ => None,
            };
            let Some(supported) = supported else {
                continue;
            };
            gate.required = supported;
            gate.passed = (!supported).then_some(true);
            gate.detail = Some(if supported {
                "Controller detected applicable project tooling; a successful runtime command is required."
                    .to_string()
            } else {
                "Controller marked this gate not applicable because no matching project tooling was detected before execution."
                    .to_string()
            });
        }
        self.refresh_checkpoint();
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

    pub fn record_runtime_independent_review(&mut self, passed: bool, detail: impl Into<String>) {
        self.record_gate("independent-review", passed, detail);
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

pub(crate) fn is_verified_browser_visual_observation(
    tool_name: &str,
    artifacts: Option<&serde_json::Value>,
) -> bool {
    let Some(artifacts) = artifacts else {
        return false;
    };
    let nested_kind = artifacts
        .pointer("/artifacts/kind")
        .or_else(|| artifacts.get("kind"))
        .and_then(serde_json::Value::as_str);
    match tool_name {
        "browser_evidence_capture" => {
            nested_kind == Some("browserEvidenceCapture")
                && (artifacts
                    .pointer("/artifacts/visual/screenshotAttached")
                    .or_else(|| artifacts.pointer("/visual/screenshotAttached"))
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                    || has_nonempty_string_at(
                        artifacts,
                        &[
                            "/data/screenshotHash",
                            "/data/screenshot/hash",
                            "/artifacts/capture/screenshotHash",
                        ],
                    ))
        }
        "browser_session" => {
            nested_kind == Some("browserObservation")
                && has_nonempty_string_at(
                    artifacts,
                    &[
                        "/data/screenshotHash",
                        "/data/screenshot/contentHash",
                        "/artifacts/observation/screenshotHash",
                        "/artifacts/observation/screenshot/contentHash",
                    ],
                )
        }
        _ => false,
    }
}

pub(crate) fn is_verified_desktop_observation(
    tool_arguments: Option<&str>,
    artifacts: Option<&serde_json::Value>,
) -> bool {
    let Some(artifacts) = artifacts else {
        return false;
    };
    let action = normalized_tool_action(tool_arguments);
    let nested_kind = artifacts
        .pointer("/artifacts/kind")
        .or_else(|| artifacts.get("kind"))
        .and_then(serde_json::Value::as_str);
    let receipt_kind = artifacts
        .pointer("/data/kind")
        .and_then(serde_json::Value::as_str);
    matches!(
        action.as_deref(),
        Some("capture_window" | "wait_for_change")
    ) && (nested_kind == Some("computerObservation")
        || receipt_kind == Some("computerObservationReceipt"))
        && has_nonempty_string_at(
            artifacts,
            &[
                "/data/screenshotHash",
                "/data/observation/screenshotHash",
                "/screenshotHash",
                "/observation/screenshotHash",
            ],
        )
}

fn normalized_tool_action(tool_arguments: Option<&str>) -> Option<String> {
    tool_arguments
        .and_then(|arguments| serde_json::from_str::<serde_json::Value>(arguments).ok())
        .and_then(|arguments| {
            arguments
                .get("action")
                .and_then(serde_json::Value::as_str)
                .map(|action| action.trim().to_ascii_lowercase())
        })
}

fn browser_session_action_invalidates_observation(tool_arguments: Option<&str>) -> bool {
    match normalized_tool_action(tool_arguments).as_deref() {
        // Inventory calls do not change the shared page, and observe is the
        // only action authorized to establish a fresh pixel completion gate.
        Some("list_sessions" | "list_tabs" | "observe") | None => false,
        // Every other successful browser_session action either changes the
        // session/tab/page or advances visible interaction state. Treat future
        // actions fail-closed so a newly added mutation cannot reuse old pixels.
        Some(_) => true,
    }
}

pub(crate) fn tool_result_requires_desktop_observation(
    tool_name: &str,
    is_error: bool,
    artifacts: Option<&serde_json::Value>,
) -> bool {
    if tool_name != "computer_control" {
        return false;
    }
    if !is_error {
        return true;
    }
    if tool_result_effect_may_have_occurred(artifacts) {
        return true;
    }
    matches!(
        artifacts
            .and_then(|artifacts| artifacts.get("code"))
            .and_then(serde_json::Value::as_str),
        Some("computer_action_uncertain" | "computer_action_timeout_uncertain")
    )
}

pub(crate) fn tool_result_effect_may_have_occurred(artifacts: Option<&serde_json::Value>) -> bool {
    artifacts.is_some_and(|artifacts| {
        match artifacts
            .get("sideEffect")
            .and_then(serde_json::Value::as_str)
        {
            Some("may_have_occurred") => true,
            Some("not_started") => false,
            _ => {
                // Compatibility only for persisted artifacts produced before
                // sideEffect became the authoritative typed field.
                artifacts
                    .get("effectMayHaveOccurred")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
            }
        }
    })
}

fn has_nonempty_string_at(value: &serde_json::Value, pointers: &[&str]) -> bool {
    pointers.iter().any(|pointer| {
        value
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|candidate| !candidate.trim().is_empty())
    })
}

fn ensure_required_gate(
    gates: &mut Vec<VerificationGate>,
    id: &str,
    kind: VerificationGateKind,
    detail: &str,
) {
    if let Some(gate) = gates.iter_mut().find(|gate| gate.kind == kind) {
        gate.id = id.to_string();
        gate.required = true;
        gate.passed = None;
        gate.detail = Some(detail.to_string());
        return;
    }
    gates.push(VerificationGate {
        id: id.to_string(),
        kind,
        required: true,
        passed: None,
        detail: Some(detail.to_string()),
    });
}

pub fn detect_project_verification_support(roots: &[PathBuf]) -> ProjectVerificationSupport {
    let mut support = ProjectVerificationSupport::default();
    for root in roots.iter().filter(|root| root.is_dir()) {
        if root.join("Cargo.toml").is_file() {
            support.tests = true;
            support.lint = true;
            support.typecheck = true;
            support.build = true;
        }

        if let Ok(raw) = std::fs::read_to_string(root.join("package.json")) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) {
                let scripts = manifest
                    .get("scripts")
                    .and_then(serde_json::Value::as_object);
                let has_script = |name: &str| {
                    scripts.is_some_and(|scripts| {
                        scripts.iter().any(|(script_name, command)| {
                            (script_name == name || script_name.starts_with(&format!("{name}:")))
                                && command.as_str().is_some_and(|command| {
                                    !(command.trim().is_empty()
                                        || name == "test"
                                            && command
                                                .to_ascii_lowercase()
                                                .contains("no test specified"))
                                })
                        })
                    })
                };
                support.tests |= has_script("test");
                support.lint |= has_script("lint");
                support.typecheck |= has_script("typecheck") || has_script("type-check");
                support.build |=
                    has_script("build") || has_script("compile") || has_script("package");
            }
        }
        support.typecheck |= root.join("tsconfig.json").is_file();
        support.lint |= has_any(
            root,
            &[
                "eslint.config.js",
                "eslint.config.mjs",
                "eslint.config.cjs",
                ".eslintrc",
                ".eslintrc.json",
                ".eslintrc.js",
                ".eslintrc.cjs",
            ],
        );

        if root.join("go.mod").is_file() {
            support.tests = true;
            support.build = true;
        }
        support.build |= has_any(root, &["Makefile", "makefile", "CMakeLists.txt"]);

        let pyproject = std::fs::read_to_string(root.join("pyproject.toml"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        support.tests |= root.join("tests").is_dir()
            || has_any(root, &["pytest.ini", "tox.ini"])
            || pyproject.contains("[tool.pytest");
        support.lint |=
            has_any(root, &["ruff.toml", ".ruff.toml"]) || pyproject.contains("[tool.ruff");
        support.typecheck |=
            has_any(root, &["mypy.ini", "pyrightconfig.json"]) || pyproject.contains("[tool.mypy");
    }
    support
}

fn has_any(root: &Path, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| root.join(candidate).is_file())
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
    if plan
        .interaction_requirements
        .requires_visual_observation_after_mutation()
        && !plan.interaction_requirements.browser_observation
    {
        verification_gates.push(VerificationGate {
            id: "browser-visual-observation".to_string(),
            kind: VerificationGateKind::BrowserVisualObservation,
            required: true,
            passed: None,
            detail: Some(
                "A rendered visual observation is required after the last mutation.".to_string(),
            ),
        });
    }
    if plan.interaction_requirements.browser_observation {
        verification_gates.push(VerificationGate {
            id: "browser-session-observation".to_string(),
            kind: VerificationGateKind::BrowserSessionObservation,
            required: true,
            passed: None,
            detail: Some(
                "A real screenshot-bearing browser_session observation is required for the requested browser operation."
                    .to_string(),
            ),
        });
    }
    if plan.interaction_requirements.requires_desktop_observation() {
        verification_gates.push(VerificationGate {
            id: "desktop-observation".to_string(),
            kind: VerificationGateKind::DesktopObservation,
            required: true,
            passed: None,
            detail: Some(
                "computer_observe is required before control and again after the last successful control."
                    .to_string(),
            ),
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
            // Balanced is the default interactive profile. Its task plan is
            // an observable controller projection, not a mandatory checklist
            // that can reject an otherwise valid answer or coerce update_plan
            // calls. Explicit deep/custom/ultra and Nexus runs retain strong
            // node completion gates; scheduled isolation strengthens them
            // again in configure_for_scheduled_isolated_patch().
            require_all_nodes_succeeded: nexus_enabled
                || profile.profile != OrchestrationProfile::Balanced,
            require_verification_gates: nexus_enabled || profile.require_independent_verifier,
            require_evidence_ledger: plan.evidence_policy.mode == EvidenceMode::Required,
            require_interaction_gates: plan.interaction_requirements.requires_completion_gate(),
        },
    };
    workflow.refresh_checkpoint();
    workflow.validate()?;
    Ok(workflow)
}

/// Compile the heavyweight workflow only when a turn has an execution
/// contract that can enforce it. Balanced direct answers intentionally return
/// `None`; callers must not synthesize a placeholder workflow artifact or scan
/// project manifests for that lane.
pub fn compile_turn_workflow_ir(
    plan: &AgentTaskPlan,
    profile: &ResolvedOrchestrationProfile,
    nexus_enabled: bool,
    requires_workspace_isolation: bool,
) -> Result<Option<WorkflowIr>, String> {
    let requires_workflow = nexus_enabled
        || profile.profile != OrchestrationProfile::Balanced
        || requires_workspace_isolation
        || plan.interaction_requirements.requires_completion_gate();
    if !requires_workflow {
        return Ok(None);
    }

    let mut workflow = compile_workflow_ir(plan, profile, nexus_enabled)?;
    if requires_workspace_isolation {
        workflow.configure_for_scheduled_isolated_patch();
    }
    Ok(Some(workflow))
}

pub(crate) fn ensure_runtime_desktop_observation_gate(
    workflow: &mut Option<WorkflowIr>,
    plan: &AgentTaskPlan,
    profile: &ResolvedOrchestrationProfile,
    nexus_enabled: bool,
) -> Result<(), String> {
    if workflow.is_none() {
        *workflow = Some(compile_workflow_ir(plan, profile, nexus_enabled)?);
    }
    workflow
        .as_mut()
        .expect("runtime desktop workflow was initialized")
        .require_fresh_desktop_observation(
            "Runtime computer_control requires a fresh computer_observe screenshot before completion.",
        );
    Ok(())
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
    use crate::tool_visibility_policy::{
        resolve_turn_capability_requirements, ToolVisibilityInput,
    };

    fn plan() -> AgentTaskPlan {
        build_task_plan(TaskPlanningInput::for_route(
            "Research and implement the change, then test it",
            "CodebaseOperation",
            false,
            0,
        ))
    }

    fn interaction_plan(query: &str) -> AgentTaskPlan {
        let requirements = resolve_turn_capability_requirements(ToolVisibilityInput {
            query,
            system_prompt: "",
            has_sources: false,
        });
        build_task_plan(TaskPlanningInput::for_requirements(
            query,
            &requirements,
            false,
            0,
        ))
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

    fn balanced_profile() -> ResolvedOrchestrationProfile {
        resolve_orchestration_profile(OrchestrationProfileInput {
            profile: OrchestrationProfile::Balanced,
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
        let python_test = serde_json::json!({
            "kind": "commandExecution",
            "execution": {
                "program": "python",
                "args": ["-m", "pytest", "-v"],
                "exitCode": 0,
                "timedOut": false
            }
        });
        workflow.observe_tool_result(
            "test-python",
            "run_shell",
            false,
            Some(&python_test),
            "command passed",
        );
        assert_eq!(
            workflow
                .verification_gates
                .iter()
                .find(|gate| gate.kind == VerificationGateKind::Tests)
                .and_then(|gate| gate.passed),
            Some(true),
            "python -m pytest -v must satisfy the executed tests gate"
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
        let simple = build_task_plan(TaskPlanningInput::for_route(
            "Rename one file",
            "FileOperation",
            false,
            0,
        ));
        let workflow = compile_workflow_ir(&simple, &profile(), true).unwrap();
        assert_eq!(workflow.ready_node_ids().len(), 1);
        assert!(workflow
            .reconnaissance_batch_arguments(&simple.objective)
            .is_none());
    }

    #[test]
    fn balanced_task_plan_is_advisory_unless_an_evidence_gate_applies() {
        let file_plan = build_task_plan(TaskPlanningInput::for_route(
            "Create the requested file",
            "FileOperation",
            false,
            0,
        ));
        let workflow = compile_workflow_ir(&file_plan, &balanced_profile(), false).unwrap();

        assert!(!workflow.completion_contract.require_all_nodes_succeeded);
        assert!(!workflow.completion_contract.require_verification_gates);
        assert!(workflow.completion_allowed());
        assert!(workflow
            .nodes
            .iter()
            .any(|node| { node.status != WorkflowNodeStatus::Succeeded }));
    }

    #[test]
    fn balanced_direct_turn_skips_workflow_while_interaction_contracts_compile_one() {
        let direct = build_task_plan(TaskPlanningInput::for_route(
            "What is the capital of France?",
            "DirectResponse",
            false,
            0,
        ));
        assert!(
            compile_turn_workflow_ir(&direct, &balanced_profile(), false, false)
                .expect("workflow policy")
                .is_none(),
            "ordinary Balanced answers must not carry an unused workflow artifact"
        );

        for query in [
            "帮我用html写一个黑洞演示图",
            "打开浏览器访问 example.com 并点击 More information",
            "Capture this app window, click Save, then verify it",
        ] {
            let plan = interaction_plan(query);
            let workflow = compile_turn_workflow_ir(&plan, &balanced_profile(), false, false)
                .expect("workflow policy")
                .expect("interaction completion contract must compile a workflow gate");
            assert!(workflow.completion_contract.require_interaction_gates);
        }
    }

    #[test]
    fn late_computer_control_dynamically_establishes_a_desktop_completion_gate() {
        let direct = build_task_plan(TaskPlanningInput::for_route(
            "在微信里点击发送按钮",
            "DirectResponse",
            false,
            0,
        ));
        let profile = balanced_profile();
        let mut workflow =
            compile_turn_workflow_ir(&direct, &profile, false, false).expect("workflow policy");
        assert!(workflow.is_none());

        ensure_runtime_desktop_observation_gate(&mut workflow, &direct, &profile, false)
            .expect("late desktop gate");
        let workflow = workflow.as_mut().expect("runtime workflow");
        workflow.observe_tool_result(
            "control",
            "computer_control",
            false,
            Some(&serde_json::json!({
                "data": {
                    "kind": "computerControlReceipt",
                    "effect": "unverifiable"
                }
            })),
            "Input was delivered but no screenshot was available.",
        );
        assert!(workflow.completion_contract.require_interaction_gates);
        assert!(!workflow.completion_allowed());

        workflow.observe_tool_result_with_arguments(
            "observe",
            "computer_observe",
            Some(r#"{"action":"capture_window"}"#),
            false,
            Some(&serde_json::json!({
                "data": {
                    "kind": "computerObservationReceipt",
                    "screenshotHash": "fresh-desktop-shot"
                }
            })),
            "Fresh window capture.",
        );
        assert!(workflow.completion_allowed());
    }

    #[test]
    fn only_uncertain_computer_control_errors_require_a_fresh_observation() {
        for code in [
            "computer_action_uncertain",
            "computer_action_timeout_uncertain",
        ] {
            let artifacts = serde_json::json!({
                "kind": "toolContractError",
                "code": code
            });
            assert!(tool_result_requires_desktop_observation(
                "computer_control",
                true,
                Some(&artifacts),
            ));
        }

        for artifacts in [
            serde_json::json!({
                "kind": "toolContractError",
                "code": "invalid_computer_action",
                "effectMayHaveOccurred": true,
            }),
            serde_json::json!({
                "kind": "toolContractError",
                "code": "invalid_computer_action",
                "sideEffect": "may_have_occurred",
            }),
        ] {
            assert!(tool_result_requires_desktop_observation(
                "computer_control",
                true,
                Some(&artifacts),
            ));
        }

        assert!(
            !tool_result_effect_may_have_occurred(Some(&serde_json::json!({
                "sideEffect": "not_started",
                "effectMayHaveOccurred": true,
            }))),
            "the typed sideEffect field must override a conflicting legacy boolean"
        );

        for code in [
            "computer_observation_stale",
            "computer_action_refused",
            "computer_user_takeover",
            "invalid_computer_action",
        ] {
            let artifacts = serde_json::json!({
                "kind": "toolContractError",
                "code": code
            });
            assert!(
                !tool_result_requires_desktop_observation(
                    "computer_control",
                    true,
                    Some(&artifacts),
                ),
                "{code} did not cross an uncertain commit boundary"
            );
        }
    }

    #[test]
    fn skipped_visual_check_cannot_complete_html_artifact_after_mutation() {
        let plan = interaction_plan("帮我用html写一个黑洞演示图");
        let mut workflow =
            compile_workflow_ir(&plan, &balanced_profile(), false).expect("visual workflow");

        workflow.observe_tool_result("write", "create_file", false, None, "created index.html");
        workflow.observe_tool_result(
            "claimed",
            "record_verification",
            false,
            Some(&serde_json::json!({
                "kind": "verification",
                "overallStatus": "passed",
                "checks": [{ "name": "Rendered visual check", "status": "skipped" }]
            })),
            "Verification recorded: passed. 0 passed, 0 failed, 0 pending, 1 skipped.",
        );
        assert!(!workflow.completion_allowed());

        workflow.observe_tool_result(
            "list",
            "browser_session",
            false,
            Some(&serde_json::json!({
                "artifacts": { "kind": "browserSessionList" },
                "data": { "sessions": [] }
            })),
            "Listed browser sessions.",
        );
        assert!(!workflow.completion_allowed());
        workflow.observe_tool_result(
            "capture",
            "browser_evidence_capture",
            false,
            Some(&serde_json::json!({
                "artifacts": {
                    "kind": "browserEvidenceCapture",
                    "visual": { "screenshotAttached": true }
                }
            })),
            "Rendered screenshot captured.",
        );
        assert!(workflow.completion_allowed());

        workflow.observe_tool_result("edit", "edit_file", false, None, "updated canvas.js");
        assert!(
            !workflow.completion_allowed(),
            "a later mutation must invalidate the older visual observation"
        );
    }

    #[test]
    fn explicit_browser_operation_requires_screenshot_bearing_session_observation() {
        let plan = interaction_plan("打开浏览器访问 example.com 并点击 More information");
        let mut workflow =
            compile_workflow_ir(&plan, &balanced_profile(), false).expect("browser workflow");

        workflow.observe_tool_result(
            "list",
            "browser_session",
            false,
            Some(&serde_json::json!({
                "artifacts": { "kind": "browserSessionList" },
                "data": { "sessions": [] }
            })),
            "Listed browser sessions.",
        );
        assert!(!workflow.completion_allowed());
        workflow.observe_tool_result(
            "other-capture",
            "browser_evidence_capture",
            false,
            Some(&serde_json::json!({
                "artifacts": {
                    "kind": "browserEvidenceCapture",
                    "visual": { "screenshotAttached": true }
                }
            })),
            "Captured a separate browser surface.",
        );
        assert!(!workflow.completion_allowed());
        let session_observation = serde_json::json!({
            "artifacts": {
                "kind": "browserObservation",
                "observation": { "screenshotHash": "session-shot" }
            },
            "data": { "screenshotHash": "session-shot" }
        });
        workflow.observe_tool_result_with_arguments(
            "session-click",
            "browser_session",
            Some(r#"{"action":"click"}"#),
            false,
            Some(&session_observation),
            "Clicked in the browser session.",
        );
        assert!(
            !workflow.completion_allowed(),
            "only an explicit browser observe may satisfy the session gate"
        );
        workflow.observe_tool_result_with_arguments(
            "session-observe",
            "browser_session",
            Some(r#"{"action":"observe"}"#),
            false,
            Some(&session_observation),
            "Observed the interacted browser tab.",
        );
        assert!(workflow.completion_allowed());
    }

    #[test]
    fn successful_browser_state_changes_invalidate_the_previous_session_observation() {
        let session_observation = serde_json::json!({
            "artifacts": {
                "kind": "browserObservation",
                "observation": { "screenshotHash": "fresh-session-shot" }
            },
            "data": { "screenshotHash": "fresh-session-shot" }
        });

        for action in [
            "create_session",
            "open_tab",
            "activate_tab",
            "close_tab",
            "close_session",
            "navigate",
            "go_back",
            "go_forward",
            "reload",
            "click",
            "double_click",
            "drag",
            "type",
            "select",
            "press",
            "scroll",
            "move",
            "hover",
            "wait_for",
        ] {
            let plan =
                interaction_plan("Open the browser, visit https://example.com, and click More");
            let mut workflow =
                compile_workflow_ir(&plan, &balanced_profile(), false).expect("browser workflow");

            workflow.observe_tool_result_with_arguments(
                "observe-before",
                "browser_session",
                Some(r#"{"action":"observe"}"#),
                false,
                Some(&session_observation),
                "Observed the browser tab.",
            );
            assert!(workflow.completion_allowed(), "setup failed for {action}");

            workflow.observe_tool_result_with_arguments(
                "browser-action",
                "browser_session",
                Some(&format!(r#"{{"action":"{action}"}}"#)),
                false,
                Some(&session_observation),
                "Browser action completed.",
            );
            assert!(
                !workflow.completion_allowed(),
                "successful browser action `{action}` must invalidate the previous observation even when its result embeds a screenshot"
            );

            workflow.observe_tool_result_with_arguments(
                "observe-without-pixels",
                "browser_session",
                Some(r#"{"action":"observe"}"#),
                false,
                Some(&serde_json::json!({
                    "artifacts": { "kind": "browserObservation" },
                    "data": {}
                })),
                "Observed without a screenshot.",
            );
            assert!(
                !workflow.completion_allowed(),
                "a screenshot-free observe must not repair `{action}`"
            );

            workflow.observe_tool_result_with_arguments(
                "observe-after",
                "browser_session",
                Some(r#"{"action":"observe"}"#),
                false,
                Some(&session_observation),
                "Observed the changed browser tab.",
            );
            assert!(
                workflow.completion_allowed(),
                "a fresh screenshot-bearing observe must repair `{action}`"
            );
        }
    }

    #[test]
    fn browser_inventory_reads_preserve_a_fresh_session_observation() {
        let plan = interaction_plan("Open the browser, visit https://example.com, and click More");
        let mut workflow =
            compile_workflow_ir(&plan, &balanced_profile(), false).expect("browser workflow");
        let session_observation = serde_json::json!({
            "artifacts": { "kind": "browserObservation" },
            "data": { "screenshotHash": "fresh-session-shot" }
        });
        workflow.observe_tool_result_with_arguments(
            "observe",
            "browser_session",
            Some(r#"{"action":"observe"}"#),
            false,
            Some(&session_observation),
            "Observed the browser tab.",
        );
        assert!(workflow.completion_allowed());

        for action in ["list_sessions", "list_tabs"] {
            workflow.observe_tool_result_with_arguments(
                "inventory",
                "browser_session",
                Some(&format!(r#"{{"action":"{action}"}}"#)),
                false,
                None,
                "Listed browser state.",
            );
            assert!(
                workflow.completion_allowed(),
                "read-only browser action `{action}` must not invalidate a fresh observation"
            );
        }
    }

    #[test]
    fn desktop_control_requires_fresh_computer_observation_before_completion() {
        let plan = interaction_plan(
            "Capture this app window, click the Save button with the mouse, then verify it",
        );
        let mut workflow =
            compile_workflow_ir(&plan, &balanced_profile(), false).expect("desktop workflow");

        assert!(!workflow.completion_allowed());
        workflow.observe_tool_result_with_arguments(
            "list-before",
            "computer_observe",
            Some(r#"{"action":"list_windows"}"#),
            false,
            Some(&serde_json::json!({
                "artifacts": { "kind": "computerObservation" },
                "data": {
                    "windows": [],
                    "screenshotHash": "forged-or-stale-list-thumbnail"
                }
            })),
            "Listed windows.",
        );
        assert!(!workflow.completion_allowed());
        let captured_window = serde_json::json!({
            "artifacts": { "kind": "computerObservation" },
            "data": { "screenshotHash": "capture-hash" }
        });
        workflow.observe_tool_result_with_arguments(
            "observe-before",
            "computer_observe",
            Some(r#"{"action":"capture_window"}"#),
            false,
            Some(&captured_window),
            "Window captured.",
        );
        assert!(workflow.completion_allowed());
        workflow.observe_tool_result("control", "computer_control", false, None, "Save clicked.");
        assert!(!workflow.completion_allowed());
        workflow.observe_tool_result_with_arguments(
            "observe-after",
            "computer_observe",
            Some(r#"{"action":"capture_window"}"#),
            false,
            Some(&captured_window),
            "Fresh window capture shows the saved state.",
        );
        assert!(workflow.completion_allowed());
    }

    #[test]
    fn explicit_nexus_keeps_strong_task_completion_gates() {
        let workflow = compile_workflow_ir(&plan(), &balanced_profile(), true).unwrap();

        assert!(workflow.completion_contract.require_all_nodes_succeeded);
        assert!(!workflow.completion_allowed());
    }

    #[test]
    fn plan_mode_removes_execution_isolation_and_release_gates() {
        let mut workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        assert!(workflow.requires_runtime_write_isolation());

        workflow.configure_for_plan_mode();

        assert!(!workflow.requires_runtime_write_isolation());
        assert!(workflow.verification_gates.is_empty());
        assert!(!workflow.completion_contract.require_all_nodes_succeeded);
        assert!(!workflow.completion_contract.require_verification_gates);
        assert!(!workflow.completion_contract.require_evidence_ledger);
        assert!(workflow.nodes.iter().all(|node| {
            node.phase == "planning"
                && node.isolation == WorkflowIsolation::SharedReadOnly
                && node
                    .allowed_tools
                    .iter()
                    .all(|tool| !tool_may_mutate_workspace(tool))
        }));
    }

    #[test]
    fn scheduled_isolated_patch_overrides_underpredicted_write_and_delegation_plan() {
        let mut underpredicted = plan();
        for step in &mut underpredicted.steps {
            step.required_tools = vec!["read_file".to_string()];
        }
        let mut workflow = compile_workflow_ir(&underpredicted, &profile(), true).unwrap();
        assert!(!workflow.requires_runtime_write_isolation());
        assert!(workflow
            .nodes
            .iter()
            .any(|node| node.phase == "reconnaissance"));

        workflow.configure_for_scheduled_isolated_patch();

        assert!(workflow.requires_runtime_write_isolation());
        assert!(workflow
            .nodes
            .iter()
            .all(|node| node.phase != "reconnaissance"));
        assert!(workflow
            .nodes
            .iter()
            .all(|node| node.allowed_tools.iter().all(|tool| !matches!(
                tool.as_str(),
                "spawn_subagent" | "spawn_subagent_batch" | "judge_subagent_results"
            ))));
        let independent = workflow
            .verification_gates
            .iter()
            .find(|gate| gate.kind == VerificationGateKind::IndependentReview)
            .unwrap();
        assert!(independent.required);
        assert_eq!(independent.passed, None);

        workflow.record_runtime_independent_review(
            true,
            "Controller independently checked the isolated Git patch.",
        );
        assert_eq!(
            workflow
                .verification_gates
                .iter()
                .find(|gate| gate.kind == VerificationGateKind::IndependentReview)
                .and_then(|gate| gate.passed),
            Some(true)
        );
    }

    #[test]
    fn controller_requires_only_project_supported_verification_gates() {
        let python = tempfile::tempdir().unwrap();
        std::fs::create_dir(python.path().join("tests")).unwrap();
        std::fs::write(
            python.path().join("pyproject.toml"),
            "[project]\nname = \"small-python-package\"\n",
        )
        .unwrap();
        let support = detect_project_verification_support(&[python.path().to_path_buf()]);
        assert_eq!(
            support,
            ProjectVerificationSupport {
                tests: true,
                lint: false,
                typecheck: false,
                build: false,
            }
        );

        let mut workflow = compile_workflow_ir(&plan(), &profile(), true).unwrap();
        workflow.configure_project_verification_support(support);
        for gate in workflow.verification_gates.iter().filter(|gate| {
            matches!(
                gate.kind,
                VerificationGateKind::Tests
                    | VerificationGateKind::Lint
                    | VerificationGateKind::Typecheck
                    | VerificationGateKind::Build
            )
        }) {
            if gate.kind == VerificationGateKind::Tests {
                assert!(gate.required);
                assert_eq!(gate.passed, None);
            } else {
                assert!(!gate.required);
                assert_eq!(gate.passed, Some(true));
                assert!(gate
                    .detail
                    .as_deref()
                    .is_some_and(|detail| detail.contains("not applicable")));
            }
        }

        let rust = tempfile::tempdir().unwrap();
        std::fs::write(
            rust.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        assert_eq!(
            detect_project_verification_support(&[rust.path().to_path_buf()]),
            ProjectVerificationSupport {
                tests: true,
                lint: true,
                typecheck: true,
                build: true,
            }
        );
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
        let file_plan = build_task_plan(TaskPlanningInput::for_route(
            "Investigate multiple file constraints, implement the requested edits, and verify every output across the project",
            "FileOperation",
            true,
            1,
        ));
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
