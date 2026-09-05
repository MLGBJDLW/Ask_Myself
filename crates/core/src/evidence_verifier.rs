//! Deterministic evidence audit for final agent answers.
//!
//! This is intentionally lightweight. It does not claim semantic proof; it
//! verifies the execution contract around evidence use so trace review and the
//! UI can spot unsupported answers.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::citations::extract_citations;
use crate::intelligence::{AgentTaskPlan, EvidenceMode};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeVerificationSignals {
    pub required: bool,
    pub reasons: Vec<String>,
    pub verification_artifact_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceSignals {
    pub successful_evidence_tool_calls: usize,
    pub verification_tool_recorded: bool,
    pub runtime_verification: RuntimeVerificationSignals,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Pending,
    Passed,
    Failed,
    Skipped,
}

impl VerificationStatus {
    fn as_str(&self) -> &'static str {
        match self {
            VerificationStatus::Pending => "pending",
            VerificationStatus::Passed => "passed",
            VerificationStatus::Failed => "failed",
            VerificationStatus::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VerificationCheck {
    pub name: String,
    pub status: VerificationStatus,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceAudit {
    pub kind: String,
    pub summary: String,
    pub overall_status: String,
    pub checks: Vec<VerificationCheck>,
    pub counts: EvidenceAuditCounts,
    pub runtime_gate: RuntimeVerificationSignals,
    pub citation_count: usize,
    pub explicit_insufficiency: bool,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceAuditCounts {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub skipped: usize,
}

impl EvidenceAudit {
    pub fn to_artifact(&self) -> serde_json::Value {
        let checks = self
            .checks
            .iter()
            .map(|check| {
                serde_json::json!({
                    "name": check.name,
                    "status": check.status.as_str(),
                    "details": check.details,
                })
            })
            .collect::<Vec<_>>();

        serde_json::json!({
            "kind": "verification",
            "summary": self.summary,
            "overallStatus": self.overall_status,
            "checks": checks,
            "counts": {
                "total": self.counts.total,
                "passed": self.counts.passed,
                "failed": self.counts.failed,
                "pending": self.counts.pending,
                "skipped": self.counts.skipped,
            },
            "runtimeGate": {
                "required": self.runtime_gate.required,
                "reasons": self.runtime_gate.reasons,
                "verificationArtifactStatus": self.runtime_gate.verification_artifact_status,
            },
            "citationCount": self.citation_count,
            "explicitInsufficiency": self.explicit_insufficiency,
            "updatedAt": self.updated_at,
        })
    }
}

pub fn audit_final_answer(
    plan: &AgentTaskPlan,
    answer_text: &str,
    signals: EvidenceSignals,
) -> EvidenceAudit {
    let citations = extract_citations(answer_text);
    let citation_count = citations.len();
    let explicit_insufficiency = answer_states_insufficient_evidence(answer_text);
    let mut checks = Vec::new();

    let evidence_required = matches!(plan.evidence_policy.mode, EvidenceMode::Required);
    let evidence_preferred = matches!(plan.evidence_policy.mode, EvidenceMode::Prefer);
    let min_sources = plan.evidence_policy.min_sources as usize;

    checks.push(check(
        "Evidence requirement",
        if evidence_required {
            if citation_count >= min_sources.max(1) || explicit_insufficiency {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            }
        } else if evidence_preferred {
            if citation_count > 0 || signals.successful_evidence_tool_calls > 0 {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Skipped
            }
        } else {
            VerificationStatus::Skipped
        },
        Some(if evidence_required {
            format!(
                "Requires at least {} cited source(s); found {} citation(s).",
                min_sources.max(1),
                citation_count
            )
        } else {
            "Evidence is not mandatory for this route.".to_string()
        }),
    ));

    checks.push(check(
        "Citation coverage",
        if plan.evidence_policy.require_citations {
            if citation_count > 0 || explicit_insufficiency {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            }
        } else {
            VerificationStatus::Skipped
        },
        Some(format!(
            "Final answer contains {citation_count} citation marker(s)."
        )),
    ));

    checks.push(check(
        "Evidence tool usage",
        if evidence_required || evidence_preferred {
            if signals.successful_evidence_tool_calls > 0 || explicit_insufficiency {
                VerificationStatus::Passed
            } else if evidence_required {
                VerificationStatus::Failed
            } else {
                VerificationStatus::Pending
            }
        } else {
            VerificationStatus::Skipped
        },
        Some(format!(
            "{} successful evidence-oriented tool call(s) observed.",
            signals.successful_evidence_tool_calls
        )),
    ));

    checks.push(check(
        "Verification record",
        if plan.evidence_policy.require_verification {
            if signals.verification_tool_recorded {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Pending
            }
        } else {
            VerificationStatus::Skipped
        },
        Some(if signals.verification_tool_recorded {
            "The agent recorded an explicit verification artifact.".to_string()
        } else {
            "No explicit record_verification tool call was observed.".to_string()
        }),
    ));

    let runtime_gate = signals.runtime_verification.clone();
    checks.push(check(
        "Runtime completion gate",
        if runtime_gate.required {
            match runtime_gate.verification_artifact_status.as_deref() {
                Some("passed") => VerificationStatus::Passed,
                Some("failed") => VerificationStatus::Failed,
                _ => VerificationStatus::Pending,
            }
        } else {
            VerificationStatus::Skipped
        },
        Some(runtime_gate_details(&runtime_gate)),
    ));

    checks.push(check(
        "Contradiction check",
        if plan.evidence_policy.contradiction_check {
            if signals.verification_tool_recorded || explicit_insufficiency {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Pending
            }
        } else {
            VerificationStatus::Skipped
        },
        Some("Contradiction handling is required for evidence-heavy routes.".to_string()),
    ));

    let counts = count_checks(&checks);
    let overall_status = overall_status(&counts).to_string();
    let summary = match overall_status.as_str() {
        "passed" => "Evidence audit passed.".to_string(),
        "failed" => "Evidence audit found unsupported answer risks.".to_string(),
        "partial" => "Evidence audit is partially complete.".to_string(),
        _ => "Evidence audit is pending.".to_string(),
    };

    EvidenceAudit {
        kind: "verification".to_string(),
        summary,
        overall_status,
        checks,
        counts,
        runtime_gate,
        citation_count,
        explicit_insufficiency,
        updated_at: Utc::now().to_rfc3339(),
    }
}

fn runtime_gate_details(gate: &RuntimeVerificationSignals) -> String {
    if !gate.required {
        return "No runtime-sensitive mutation requiring explicit verification was observed."
            .to_string();
    }

    let reason = if gate.reasons.is_empty() {
        "runtime-sensitive mutation".to_string()
    } else {
        gate.reasons.join("; ")
    };
    match gate.verification_artifact_status.as_deref() {
        Some(status) => {
            format!("Observed {reason}. Latest explicit verification artifact status: {status}.")
        }
        None => {
            format!("Observed {reason}. No explicit record_verification artifact was observed.")
        }
    }
}

fn check(name: &str, status: VerificationStatus, details: Option<String>) -> VerificationCheck {
    VerificationCheck {
        name: name.to_string(),
        status,
        details,
    }
}

fn count_checks(checks: &[VerificationCheck]) -> EvidenceAuditCounts {
    let mut counts = EvidenceAuditCounts {
        total: checks.len(),
        passed: 0,
        failed: 0,
        pending: 0,
        skipped: 0,
    };
    for check in checks {
        match check.status {
            VerificationStatus::Passed => counts.passed += 1,
            VerificationStatus::Failed => counts.failed += 1,
            VerificationStatus::Pending => counts.pending += 1,
            VerificationStatus::Skipped => counts.skipped += 1,
        }
    }
    counts
}

fn overall_status(counts: &EvidenceAuditCounts) -> &'static str {
    if counts.failed > 0 {
        "failed"
    } else if counts.pending > 0 && counts.passed > 0 {
        "partial"
    } else if counts.pending > 0 {
        "pending"
    } else if counts.passed > 0 || (counts.skipped > 0 && counts.skipped == counts.total) {
        "passed"
    } else {
        "pending"
    }
}

fn answer_states_insufficient_evidence(answer_text: &str) -> bool {
    let lower = answer_text.to_lowercase();
    [
        "insufficient evidence",
        "not enough evidence",
        "no evidence",
        "could not find",
        "cannot verify",
        "unable to verify",
        "没有足够证据",
        "证据不足",
        "没有找到",
        "无法确认",
        "无法验证",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::{build_task_plan, TaskPlanningInput};

    fn knowledge_plan() -> AgentTaskPlan {
        build_task_plan(TaskPlanningInput::for_route(
            "What changed in the retry notes?",
            "KnowledgeRetrieval",
            true,
            2,
        ))
    }

    #[test]
    fn required_evidence_fails_without_citations_or_insufficiency() {
        let audit = audit_final_answer(
            &knowledge_plan(),
            "The retry guard changed because the timeout moved.",
            EvidenceSignals {
                successful_evidence_tool_calls: 1,
                verification_tool_recorded: false,
                ..EvidenceSignals::default()
            },
        );

        assert_eq!(audit.overall_status, "failed");
        assert!(audit
            .checks
            .iter()
            .any(|check| check.name == "Citation coverage"
                && check.status == VerificationStatus::Failed));
    }

    #[test]
    fn required_evidence_passes_with_citations_and_verification() {
        let audit = audit_final_answer(
            &knowledge_plan(),
            "The retry guard changed because the timeout moved. [cite:chunk-1] [cite:chunk-2]",
            EvidenceSignals {
                successful_evidence_tool_calls: 2,
                verification_tool_recorded: true,
                ..EvidenceSignals::default()
            },
        );

        assert_eq!(audit.overall_status, "passed");
        assert_eq!(audit.citation_count, 2);
        assert_eq!(audit.to_artifact()["kind"], "verification");
    }

    #[test]
    fn explicit_insufficiency_is_not_treated_as_hallucination() {
        let audit = audit_final_answer(
            &knowledge_plan(),
            "I could not find enough evidence in the linked sources to confirm that.",
            EvidenceSignals {
                successful_evidence_tool_calls: 0,
                verification_tool_recorded: false,
                ..EvidenceSignals::default()
            },
        );

        assert_ne!(audit.overall_status, "failed");
        assert!(audit.explicit_insufficiency);
    }

    #[test]
    fn runtime_gate_blocks_fully_verified_mutation_without_artifact() {
        let mut plan = build_task_plan(TaskPlanningInput::for_route(
            "Update the parser implementation.",
            "CodebaseOperation",
            true,
            1,
        ));
        plan.evidence_policy.require_verification = true;

        let audit = audit_final_answer(
            &plan,
            "Updated the parser and ran the focused tests.",
            EvidenceSignals {
                successful_evidence_tool_calls: 1,
                verification_tool_recorded: false,
                runtime_verification: RuntimeVerificationSignals {
                    required: true,
                    reasons: vec!["edit_file modified source files".to_string()],
                    verification_artifact_status: None,
                },
            },
        );

        assert_ne!(audit.overall_status, "passed");
        let gate = audit
            .checks
            .iter()
            .find(|check| check.name == "Runtime completion gate")
            .expect("runtime gate check");
        assert_eq!(gate.status, VerificationStatus::Pending);
        assert_eq!(audit.to_artifact()["runtimeGate"]["required"], true);
    }

    #[test]
    fn runtime_gate_passes_when_explicit_verification_artifact_passed() {
        let audit = audit_final_answer(
            &knowledge_plan(),
            "The retry guard changed because the timeout moved. [cite:chunk-1]",
            EvidenceSignals {
                successful_evidence_tool_calls: 1,
                verification_tool_recorded: true,
                runtime_verification: RuntimeVerificationSignals {
                    required: true,
                    reasons: vec!["multi_edit modified source files".to_string()],
                    verification_artifact_status: Some("passed".to_string()),
                },
            },
        );

        let gate = audit
            .checks
            .iter()
            .find(|check| check.name == "Runtime completion gate")
            .expect("runtime gate check");
        assert_eq!(gate.status, VerificationStatus::Passed);
    }
}
