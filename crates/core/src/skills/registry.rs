use crate::error::CoreError;

use super::model::{Skill, SkillFrontmatter, SkillResourceEncoding, SkillResourceFile};
use super::storage::{
    resource_bundle_metadata, resource_kind_from_relative_path, substitute_skill_dir,
};

const EMPTY_BUILTIN_RESOURCES: &[BuiltinSkillResource] = &[];

pub(crate) struct BuiltinSkillBundle {
    pub(crate) slug: &'static str,
    pub(crate) skill_md: &'static str,
    pub(crate) resources: &'static [BuiltinSkillResource],
}

pub(crate) struct BuiltinSkillResource {
    pub(crate) path: &'static str,
    pub(crate) content: &'static str,
}

/// Bundled built-in skills. Content is embedded at compile time via
/// `include_str!` so the binary is self-contained.
static BUILTIN_SKILLS: &[BuiltinSkillBundle] = &[
    BuiltinSkillBundle {
        slug: "visual-explanations",
        skill_md: include_str!("../../assets/skills/visual-explanations/SKILL.md"),
        resources: EMPTY_BUILTIN_RESOURCES,
    },
    BuiltinSkillBundle {
        slug: "office-document-design",
        skill_md: include_str!("../../assets/skills/office-document-design/SKILL.md"),
        resources: &[
            BuiltinSkillResource {
                path: "scripts/outline-blueprint.md",
                content: include_str!(
                    "../../assets/skills/office-document-design/scripts/outline-blueprint.md"
                ),
            },
            BuiltinSkillResource {
                path: "assets/theme-presets.json",
                content: include_str!(
                    "../../assets/skills/office-document-design/assets/theme-presets.json"
                ),
            },
        ],
    },
    BuiltinSkillBundle {
        slug: "docx-document-design",
        skill_md: include_str!("../../assets/skills/docx-document-design/SKILL.md"),
        resources: &[
            BuiltinSkillResource {
                path: "references/docx-playbook.md",
                content: include_str!(
                    "../../assets/skills/docx-document-design/references/docx-playbook.md"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/docx_audit.py",
                content: include_str!(
                    "../../assets/skills/docx-document-design/scripts/docx_audit.py"
                ),
            },
        ],
    },
    BuiltinSkillBundle {
        slug: "pptx-presentation-design",
        skill_md: include_str!("../../assets/skills/pptx-presentation-design/SKILL.md"),
        resources: &[
            BuiltinSkillResource {
                path: "references/pptx-playbook.md",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/references/pptx-playbook.md"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_audit.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_audit.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_renderer.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_renderer.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_quality_gate.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_quality_gate.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_template_profile.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_template_profile.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_template_bind.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_template_bind.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_visual_qa.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_visual_qa.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_style_profile.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_style_profile.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_deck_planner.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_deck_planner.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_rewrite_plan.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_rewrite_plan.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_semantic_rewriter.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_semantic_rewriter.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_asset_pack.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_asset_pack.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_regression_suite.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_regression_suite.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/pptx_delivery_pack.py",
                content: include_str!(
                    "../../assets/skills/pptx-presentation-design/scripts/pptx_delivery_pack.py"
                ),
            },
        ],
    },
    BuiltinSkillBundle {
        slug: "xlsx-workbook-design",
        skill_md: include_str!("../../assets/skills/xlsx-workbook-design/SKILL.md"),
        resources: &[
            BuiltinSkillResource {
                path: "references/xlsx-playbook.md",
                content: include_str!(
                    "../../assets/skills/xlsx-workbook-design/references/xlsx-playbook.md"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/xlsx_audit.py",
                content: include_str!(
                    "../../assets/skills/xlsx-workbook-design/scripts/xlsx_audit.py"
                ),
            },
        ],
    },
    BuiltinSkillBundle {
        slug: "evidence-first",
        skill_md: include_str!("../../assets/skills/evidence-first/SKILL.md"),
        resources: EMPTY_BUILTIN_RESOURCES,
    },
    BuiltinSkillBundle {
        slug: "doc-script-editor",
        skill_md: include_str!("../../assets/skills/doc-script-editor/SKILL.md"),
        resources: &[
            BuiltinSkillResource {
                path: "scripts/edit_doc.py",
                content: include_str!("../../assets/skills/doc-script-editor/scripts/edit_doc.py"),
            },
            BuiltinSkillResource {
                path: "scripts/requirements.txt",
                content: include_str!(
                    "../../assets/skills/doc-script-editor/scripts/requirements.txt"
                ),
            },
        ],
    },
    BuiltinSkillBundle {
        slug: "skill-creator",
        skill_md: include_str!("../../assets/skills/skill-creator/SKILL.md"),
        resources: &[
            BuiltinSkillResource {
                path: "LICENSE.txt",
                content: include_str!("../../assets/skills/skill-creator/LICENSE.txt"),
            },
            BuiltinSkillResource {
                path: "agents/analyzer.md",
                content: include_str!("../../assets/skills/skill-creator/agents/analyzer.md"),
            },
            BuiltinSkillResource {
                path: "agents/comparator.md",
                content: include_str!("../../assets/skills/skill-creator/agents/comparator.md"),
            },
            BuiltinSkillResource {
                path: "agents/grader.md",
                content: include_str!("../../assets/skills/skill-creator/agents/grader.md"),
            },
            BuiltinSkillResource {
                path: "assets/eval_review.html",
                content: include_str!("../../assets/skills/skill-creator/assets/eval_review.html"),
            },
            BuiltinSkillResource {
                path: "eval-viewer/generate_review.py",
                content: include_str!(
                    "../../assets/skills/skill-creator/eval-viewer/generate_review.py"
                ),
            },
            BuiltinSkillResource {
                path: "eval-viewer/viewer.html",
                content: include_str!("../../assets/skills/skill-creator/eval-viewer/viewer.html"),
            },
            BuiltinSkillResource {
                path: "references/schemas.md",
                content: include_str!("../../assets/skills/skill-creator/references/schemas.md"),
            },
            BuiltinSkillResource {
                path: "scripts/__init__.py",
                content: include_str!("../../assets/skills/skill-creator/scripts/__init__.py"),
            },
            BuiltinSkillResource {
                path: "scripts/aggregate_benchmark.py",
                content: include_str!(
                    "../../assets/skills/skill-creator/scripts/aggregate_benchmark.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/generate_report.py",
                content: include_str!(
                    "../../assets/skills/skill-creator/scripts/generate_report.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/improve_description.py",
                content: include_str!(
                    "../../assets/skills/skill-creator/scripts/improve_description.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/package_skill.py",
                content: include_str!("../../assets/skills/skill-creator/scripts/package_skill.py"),
            },
            BuiltinSkillResource {
                path: "scripts/quick_validate.py",
                content: include_str!(
                    "../../assets/skills/skill-creator/scripts/quick_validate.py"
                ),
            },
            BuiltinSkillResource {
                path: "scripts/run_eval.py",
                content: include_str!("../../assets/skills/skill-creator/scripts/run_eval.py"),
            },
            BuiltinSkillResource {
                path: "scripts/run_loop.py",
                content: include_str!("../../assets/skills/skill-creator/scripts/run_loop.py"),
            },
            BuiltinSkillResource {
                path: "scripts/utils.py",
                content: include_str!("../../assets/skills/skill-creator/scripts/utils.py"),
            },
        ],
    },
];

pub(crate) fn builtin_skill_bundles() -> &'static [BuiltinSkillBundle] {
    BUILTIN_SKILLS
}

/// Parse a SKILL.md file (YAML frontmatter + markdown body).
///
/// The frontmatter must be delimited by `---` on its own line at the start
/// of the file, and closed by another `---` line.
pub fn parse_skill_file(content: &str) -> Result<(SkillFrontmatter, String), CoreError> {
    let trimmed = content.trim_start_matches('\u{feff}');
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            CoreError::InvalidInput("SKILL.md must start with YAML frontmatter (---)".into())
        })?;

    let (front_matter_text, body) = split_frontmatter(rest)?;

    let fm: SkillFrontmatter = serde_yaml::from_str(front_matter_text)
        .map_err(|e| CoreError::InvalidInput(format!("Invalid SKILL.md YAML frontmatter: {e}")))?;

    if fm.name.trim().is_empty() {
        return Err(CoreError::InvalidInput(
            "SKILL.md frontmatter must include a non-empty `name`".into(),
        ));
    }

    Ok((fm, body.trim().to_string()))
}

pub(crate) fn split_frontmatter(rest: &str) -> Result<(&str, &str), CoreError> {
    let mut cursor = 0;
    for line in rest.split_inclusive('\n') {
        let stripped = line.trim_end_matches(['\n', '\r']);
        if stripped == "---" {
            let fm = &rest[..cursor];
            let body_start = cursor + line.len();
            let body = &rest[body_start..];
            return Ok((fm, body));
        }
        cursor += line.len();
    }
    Err(CoreError::InvalidInput(
        "SKILL.md frontmatter is not closed with `---`".into(),
    ))
}

/// Load all built-in skills bundled with the binary.
pub fn load_builtin_skills() -> Vec<Skill> {
    let mut out = Vec::with_capacity(BUILTIN_SKILLS.len());
    for bundle in BUILTIN_SKILLS {
        match parse_skill_file(bundle.skill_md) {
            Ok((fm, body)) => {
                let body = substitute_skill_dir(body, bundle.slug);
                let resource_bundle = bundle
                    .resources
                    .iter()
                    .map(|resource| SkillResourceFile {
                        path: resource.path.to_string(),
                        kind: resource_kind_from_relative_path(resource.path),
                        encoding: SkillResourceEncoding::Utf8,
                        content: resource.content.to_string(),
                    })
                    .collect::<Vec<_>>();
                out.push(Skill {
                    id: format!("builtin-{}", bundle.slug),
                    name: fm.name,
                    description: fm.description,
                    content: body,
                    enabled: true,
                    created_at: String::new(),
                    updated_at: String::new(),
                    builtin: true,
                    resources: resource_bundle_metadata(&resource_bundle),
                    resource_bundle,
                });
            }
            Err(e) => {
                tracing::error!(skill = bundle.slug, error = %e, "Failed to parse bundled SKILL.md");
            }
        }
    }
    out
}
