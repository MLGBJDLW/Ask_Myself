//! Built-in multi-agent workflow catalog.
//!
//! These templates are pure product contracts: they define reusable fan-out
//! patterns without depending on desktop runtime state or model calls.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct WorkflowTaskDefinition {
    pub id: &'static str,
    pub role_id: &'static str,
    pub task: &'static str,
    pub expected_output: &'static str,
    pub deliverable_style: &'static str,
    pub acceptance_criteria: &'static [&'static str],
}

#[derive(Clone, Debug)]
pub struct WorkflowTemplateDefinition {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub prompt_template: &'static str,
    pub max_parallel: u32,
    pub tasks: &'static [WorkflowTaskDefinition],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCatalogTask {
    pub id: String,
    pub role_id: String,
    pub role_label: String,
    pub task: String,
    pub expected_output: String,
    pub deliverable_style: String,
    pub acceptance_criteria: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowCatalogTemplate {
    pub id: String,
    pub label: String,
    pub description: String,
    pub max_parallel: u32,
    pub prompt_template: String,
    pub tasks: Vec<WorkflowCatalogTask>,
}

const RESEARCH_VERIFY_TASKS: &[WorkflowTaskDefinition] = &[
    WorkflowTaskDefinition {
        id: "research",
        role_id: "researcher",
        task: "Gather the strongest evidence and summarize what is directly supported.",
        expected_output: "Evidence-backed findings with gaps called out.",
        deliverable_style: "research brief",
        acceptance_criteria: &[
            "Use retrieval or explicit provided context before concluding.",
            "Separate direct evidence from inference.",
        ],
    },
    WorkflowTaskDefinition {
        id: "verify",
        role_id: "verifier",
        task: "Verify the likely answer or plan against available evidence and identify unsupported claims.",
        expected_output: "Verification verdict with checks and risks.",
        deliverable_style: "verification report",
        acceptance_criteria: &[
            "Flag every unsupported or stale claim.",
            "State what evidence would be needed to raise confidence.",
        ],
    },
    WorkflowTaskDefinition {
        id: "critique",
        role_id: "critic",
        task: "Stress-test the findings for blind spots, contradictions, or operational risks.",
        expected_output: "Concise critique with remediation suggestions.",
        deliverable_style: "risk critique",
        acceptance_criteria: &[
            "Focus on risks that would change the supervisor's final answer.",
            "Do not repeat the researcher unless adding a distinct concern.",
        ],
    },
];

const DRAFT_REVIEW_TASKS: &[WorkflowTaskDefinition] = &[
    WorkflowTaskDefinition {
        id: "draft",
        role_id: "writer",
        task: "Create a concise first draft that satisfies the goal and notes assumptions.",
        expected_output: "Draft ready for supervisor editing.",
        deliverable_style: "draft",
        acceptance_criteria: &[
            "Use only supplied or retrieved facts.",
            "Mark assumptions explicitly.",
        ],
    },
    WorkflowTaskDefinition {
        id: "review",
        role_id: "critic",
        task: "Review the draft for clarity, omissions, and trust or UX risks.",
        expected_output: "Review notes and concrete improvements.",
        deliverable_style: "editorial critique",
        acceptance_criteria: &[
            "Prioritize issues over praise.",
            "Suggest specific edits the supervisor can apply.",
        ],
    },
    WorkflowTaskDefinition {
        id: "verify",
        role_id: "verifier",
        task: "Check that the draft's factual claims are supported.",
        expected_output: "Claim verification summary.",
        deliverable_style: "fact check",
        acceptance_criteria: &[
            "Identify claims that need citations or evidence.",
            "Do not rewrite the full draft.",
        ],
    },
];

const CONNECTOR_BACKGROUND_TASKS: &[WorkflowTaskDefinition] = &[
    WorkflowTaskDefinition {
        id: "connector-map",
        role_id: "connector",
        task: "Map connector or MCP options relevant to the goal, including setup and safety constraints.",
        expected_output: "Connector recommendation with risks.",
        deliverable_style: "connector brief",
        acceptance_criteria: &[
            "Mention credentials, process lifecycle, and timeout implications.",
            "Prefer disabled-by-default or approval-gated recommendations for high-risk tools.",
        ],
    },
    WorkflowTaskDefinition {
        id: "background-plan",
        role_id: "planner",
        task: "Design a background-task workflow for the goal, including triggers, cancellation, and user-visible status.",
        expected_output: "Background task plan with gates.",
        deliverable_style: "workflow plan",
        acceptance_criteria: &[
            "Include start, progress, completion, and failure states.",
            "Include cancellation and retry behavior.",
        ],
    },
    WorkflowTaskDefinition {
        id: "safety-check",
        role_id: "verifier",
        task: "Check the proposed connector/background workflow for security, privacy, and prompt-injection risks.",
        expected_output: "Safety verification summary.",
        deliverable_style: "safety check",
        acceptance_criteria: &[
            "Call out any broad tool access or exfiltration risk.",
            "Recommend the narrowest safe default.",
        ],
    },
];

const MEETING_SUMMARY_TASKS: &[WorkflowTaskDefinition] = &[
    WorkflowTaskDefinition {
        id: "extract",
        role_id: "researcher",
        task: "Extract explicit decisions, action items, owners, deadlines, open questions, and source-backed context from the provided meeting material.",
        expected_output: "Meeting facts separated into decisions, actions, open questions, and supporting context.",
        deliverable_style: "meeting extraction",
        acceptance_criteria: &[
            "Separate explicit decisions from inferred follow-ups.",
            "Preserve owner and due-date uncertainty instead of guessing.",
        ],
    },
    WorkflowTaskDefinition {
        id: "draft-summary",
        role_id: "writer",
        task: "Turn the extracted material into a clean meeting summary that a busy reader can scan quickly.",
        expected_output: "Polished meeting summary with decisions, actions, risks, and follow-ups.",
        deliverable_style: "meeting summary",
        acceptance_criteria: &[
            "Keep the summary concise and action-oriented.",
            "Call out missing owners, dates, or evidence.",
        ],
    },
    WorkflowTaskDefinition {
        id: "verify-actions",
        role_id: "verifier",
        task: "Verify that the meeting summary does not invent decisions, owners, deadlines, or commitments.",
        expected_output: "Verification notes for claims that are supported, uncertain, or unsupported.",
        deliverable_style: "meeting fact check",
        acceptance_criteria: &[
            "Flag any invented or weakly supported action item.",
            "State what source text supports each risky claim.",
        ],
    },
];

const DOCUMENT_COMPARE_TASKS: &[WorkflowTaskDefinition] = &[
    WorkflowTaskDefinition {
        id: "map-documents",
        role_id: "researcher",
        task: "Identify the purpose, scope, key entities, dates, claims, and assumptions in each document before comparing them.",
        expected_output: "Document map with comparable dimensions and notable source details.",
        deliverable_style: "document map",
        acceptance_criteria: &[
            "Read or retrieve the relevant document evidence before comparing.",
            "Keep each document's claims distinct until the comparison step.",
        ],
    },
    WorkflowTaskDefinition {
        id: "compare",
        role_id: "critic",
        task: "Compare the documents for overlap, contradictions, missing coverage, risk, and decision-relevant differences.",
        expected_output: "Comparison matrix with differences that matter for the user's goal.",
        deliverable_style: "comparison brief",
        acceptance_criteria: &[
            "Prioritize differences that change a decision or next action.",
            "Avoid superficial formatting differences unless they matter.",
        ],
    },
    WorkflowTaskDefinition {
        id: "verify",
        role_id: "verifier",
        task: "Check the comparison against the cited or retrieved document evidence.",
        expected_output: "Verification verdict with unsupported or uncertain comparisons called out.",
        deliverable_style: "comparison verification",
        acceptance_criteria: &[
            "Flag every comparison that lacks source support.",
            "State where additional reading is required.",
        ],
    },
];

const REPORT_BRIEF_TASKS: &[WorkflowTaskDefinition] = &[
    WorkflowTaskDefinition {
        id: "research",
        role_id: "researcher",
        task: "Gather the strongest local evidence and organize it into findings, examples, and unresolved gaps for the requested report.",
        expected_output: "Evidence pack for a report draft.",
        deliverable_style: "report research pack",
        acceptance_criteria: &[
            "Use local knowledge sources or provided context before drafting.",
            "Mark gaps that should not be silently filled.",
        ],
    },
    WorkflowTaskDefinition {
        id: "outline",
        role_id: "planner",
        task: "Design a report structure with sections, evidence placement, and verification gates.",
        expected_output: "Report outline with dependencies and checks.",
        deliverable_style: "report outline",
        acceptance_criteria: &[
            "Make the outline usable for a polished final answer.",
            "Include where citations or source details are needed.",
        ],
    },
    WorkflowTaskDefinition {
        id: "draft",
        role_id: "writer",
        task: "Create a clear report draft grounded in the evidence pack and outline.",
        expected_output: "Report draft with assumptions and source needs noted.",
        deliverable_style: "report draft",
        acceptance_criteria: &[
            "Do not invent facts to make the report feel complete.",
            "Keep unresolved questions visible.",
        ],
    },
];

pub const WORKFLOW_TEMPLATES: &[WorkflowTemplateDefinition] = &[
    WorkflowTemplateDefinition {
        id: "research_verify",
        label: "Research + Verify",
        description: "Parallel evidence gathering, verification, and critique.",
        prompt_template: "Run the Research + Verify workflow for this goal:\n\nGoal:\n\nScope or sources to use:\n\nReturn the final answer with evidence, risks, and open questions.",
        max_parallel: 3,
        tasks: RESEARCH_VERIFY_TASKS,
    },
    WorkflowTemplateDefinition {
        id: "draft_review",
        label: "Draft + Review",
        description: "Create a draft, critique it, and fact-check it.",
        prompt_template: "Run the Draft + Review workflow for this deliverable:\n\nDeliverable:\n\nMaterial to use:\n\nTone or format constraints:\n\nReturn the revised draft plus verification notes.",
        max_parallel: 3,
        tasks: DRAFT_REVIEW_TASKS,
    },
    WorkflowTemplateDefinition {
        id: "meeting_summary",
        label: "Meeting Summary",
        description: "Turn notes into decisions, actions, risks, and follow-ups.",
        prompt_template: "Run the Meeting Summary workflow for this material:\n\nMaterial:\n\nKnown attendees or owners:\n\nReturn decisions, action items, risks, open questions, and a concise summary.",
        max_parallel: 3,
        tasks: MEETING_SUMMARY_TASKS,
    },
    WorkflowTemplateDefinition {
        id: "document_compare",
        label: "Document Compare",
        description: "Compare local documents for overlap, contradictions, and decision-relevant differences.",
        prompt_template: "Run the Document Compare workflow for these documents or excerpts:\n\nDocuments:\n\nDecision or question to support:\n\nReturn a comparison table, key differences, risks, and evidence gaps.",
        max_parallel: 3,
        tasks: DOCUMENT_COMPARE_TASKS,
    },
    WorkflowTemplateDefinition {
        id: "report_brief",
        label: "Report Brief",
        description: "Gather evidence, design an outline, and draft a grounded report.",
        prompt_template: "Run the Report Brief workflow for this topic:\n\nTopic:\n\nAudience:\n\nSources or constraints:\n\nReturn a structured report draft with evidence notes and gaps.",
        max_parallel: 3,
        tasks: REPORT_BRIEF_TASKS,
    },
    WorkflowTemplateDefinition {
        id: "connector_background",
        label: "Connector + Background Task",
        description: "Assess connector setup and background-task lifecycle risks.",
        prompt_template: "Run the Connector + Background Task workflow for this local automation goal:\n\nGoal:\n\nConnector or folder/app involved:\n\nReturn the recommended setup, background lifecycle, cancellation behavior, and safety checks.",
        max_parallel: 3,
        tasks: CONNECTOR_BACKGROUND_TASKS,
    },
];

pub fn normalize_workflow_id(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

pub fn workflow_role_label(role_id: &str) -> &'static str {
    match normalize_workflow_id(role_id).as_str() {
        "researcher" => "Researcher",
        "verifier" => "Verifier",
        "critic" => "Critic",
        "planner" => "Planner",
        "writer" => "Writer",
        "connector" => "Connector Specialist",
        "desktop_operator" => "Desktop Operator",
        _ => "Worker",
    }
}

pub fn workflow_template_by_id(template_id: &str) -> Option<&'static WorkflowTemplateDefinition> {
    let normalized = normalize_workflow_id(template_id);
    WORKFLOW_TEMPLATES
        .iter()
        .find(|template| template.id == normalized)
}

pub fn workflow_template_id_values() -> Vec<&'static str> {
    WORKFLOW_TEMPLATES
        .iter()
        .map(|template| template.id)
        .collect()
}

pub fn workflow_catalog() -> Vec<WorkflowCatalogTemplate> {
    WORKFLOW_TEMPLATES
        .iter()
        .map(|template| WorkflowCatalogTemplate {
            id: template.id.to_string(),
            label: template.label.to_string(),
            description: template.description.to_string(),
            max_parallel: template.max_parallel,
            prompt_template: template.prompt_template.to_string(),
            tasks: template
                .tasks
                .iter()
                .map(|task| WorkflowCatalogTask {
                    id: task.id.to_string(),
                    role_id: task.role_id.to_string(),
                    role_label: workflow_role_label(task.role_id).to_string(),
                    task: task.task.to_string(),
                    expected_output: task.expected_output.to_string(),
                    deliverable_style: task.deliverable_style.to_string(),
                    acceptance_criteria: task
                        .acceptance_criteria
                        .iter()
                        .map(|criterion| (*criterion).to_string())
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn workflow_catalog_exposes_product_ready_templates() {
        let catalog = workflow_catalog();
        let ids: BTreeSet<_> = catalog
            .iter()
            .map(|template| template.id.as_str())
            .collect();

        assert!(ids.contains("research_verify"));
        assert!(ids.contains("draft_review"));
        assert!(ids.contains("meeting_summary"));
        assert!(ids.contains("document_compare"));
        assert!(ids.contains("report_brief"));
        assert!(ids.contains("connector_background"));

        for template in &catalog {
            assert!(!template.label.trim().is_empty());
            assert!(!template.description.trim().is_empty());
            assert!(!template.prompt_template.trim().is_empty());
            assert!(!template.tasks.is_empty());
            assert!(template.max_parallel >= 1);

            for task in &template.tasks {
                assert!(!task.role_label.trim().is_empty());
                assert_ne!(task.role_label, "Worker");
                assert!(!task.expected_output.trim().is_empty());
                assert!(!task.deliverable_style.trim().is_empty());
                assert!(!task.acceptance_criteria.is_empty());
            }
        }
    }

    #[test]
    fn workflow_template_lookup_normalizes_common_user_input() {
        assert_eq!(
            workflow_template_by_id("Research Verify").map(|template| template.id),
            Some("research_verify")
        );
        assert_eq!(
            workflow_template_by_id("draft-review").map(|template| template.id),
            Some("draft_review")
        );
    }
}
