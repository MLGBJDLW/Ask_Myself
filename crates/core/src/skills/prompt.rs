use super::model::Skill;

const MAX_SKILL_SECTION_CHARS: usize = 8_000;
const MAX_SKILL_LINE_CHARS: usize = 420;

fn truncate_excerpt(text: &str, max_chars: usize) -> String {
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if compact.len() <= max_chars {
        return compact;
    }
    let mut cut = max_chars;
    while cut > 0 && !compact.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", compact[..cut].trim_end())
}

fn render_resource_paths(skill: &Skill) -> String {
    if skill.resources.is_empty() {
        return "none".to_string();
    }

    let mut paths = skill
        .resources
        .iter()
        .map(|resource| resource.path.as_str())
        .take(8)
        .collect::<Vec<_>>();
    let extra = skill.resources.len().saturating_sub(paths.len());
    let mut rendered = paths.join(", ");
    if extra > 0 {
        rendered.push_str(&format!(", +{extra} more"));
    }
    if rendered.is_empty() {
        paths.clear();
        "none".to_string()
    } else {
        rendered
    }
}

fn render_dependencies(skill: &Skill) -> String {
    if skill.dependencies.tools.is_empty() {
        return String::new();
    }
    let tools = skill
        .dependencies
        .tools
        .iter()
        .map(|tool| {
            if tool.kind.trim().is_empty() {
                tool.value.clone()
            } else {
                format!("{}:{}", tool.kind, tool.value)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("dependencies: {tools}\n")
}

/// Build a compact metadata-first skills section for system prompt injection.
///
/// The model sees what skills are available, but not their full instructions.
/// It must explicitly activate a skill through `manage_skill` before relying on
/// procedural details. This mirrors progressive disclosure while keeping Nexa's
/// existing database-backed skill lifecycle.
pub fn build_skills_section_for_query(skills: &[Skill], _query: &str) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut section = String::from(
        "\n\n## Available Skills\n\
         Skills are procedural capabilities available for this turn. This list is metadata only; \
         do not treat it as the full skill instructions.\n\
         - If a skill is relevant or the user explicitly names it, call `manage_skill` with \
         action `activate_skill` and the `skill_id` before following the skill.\n\
         - If an activated skill references bundled files, call `manage_skill` with action \
         `view_resource`, `skill_id`, and `resource_path` to inspect the exact resource.\n\
         - Respect each skill's policy; skills with implicit=false should only be used when \
         explicitly requested or pinned by persona.\n",
    );

    for skill in skills {
        let display_name = if skill.interface.display_name.trim().is_empty() {
            skill.name.as_str()
        } else {
            skill.interface.display_name.as_str()
        };
        let short_description = if skill.interface.short_description.trim().is_empty() {
            skill.description.as_str()
        } else {
            skill.interface.short_description.as_str()
        };
        let source = skill.source_path.as_deref().unwrap_or(if skill.builtin {
            "bundled"
        } else {
            "user-defined"
        });

        section.push_str(&format!("\n### {display_name}\n"));
        section.push_str(&format!("skill_id: {}\n", skill.id));
        section.push_str(&format!("source: {source}\n"));
        section.push_str(&format!(
            "short_description: {}\n",
            truncate_excerpt(short_description, MAX_SKILL_LINE_CHARS)
        ));
        if !skill.description.trim().is_empty() {
            section.push_str(&format!(
                "use_when: {}\n",
                truncate_excerpt(&skill.description, MAX_SKILL_LINE_CHARS)
            ));
        }
        if let Some(default_prompt) = skill.interface.default_prompt.as_deref() {
            if !default_prompt.trim().is_empty() {
                section.push_str(&format!(
                    "default_prompt: {}\n",
                    truncate_excerpt(default_prompt, MAX_SKILL_LINE_CHARS)
                ));
            }
        }
        section.push_str(&render_dependencies(skill));
        section.push_str(&format!(
            "policy: implicit={}\n",
            skill.policy.allow_implicit_invocation
        ));
        section.push_str(&format!("resources: {}\n", render_resource_paths(skill)));

        if section.len() >= MAX_SKILL_SECTION_CHARS {
            section = truncate_excerpt(&section, MAX_SKILL_SECTION_CHARS);
            section.push_str("\n...[skills metadata truncated]");
            break;
        }
    }

    section
}

/// Backwards-compatible wrapper used by tests and older callers.
pub fn build_skills_section(skills: &[Skill]) -> String {
    build_skills_section_for_query(skills, "")
}

/// Serialize a skill to standard SKILL.md text (YAML frontmatter + body).
pub fn export_skill_to_md(skill: &Skill) -> String {
    let name = escape_yaml_scalar(&skill.name);
    let description = escape_yaml_scalar(&skill.description);
    format!(
        "---\nname: {name}\ndescription: {description}\n---\n\n{}\n",
        skill.content.trim()
    )
}

fn escape_yaml_scalar(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quote = value.contains(':')
        || value.contains('#')
        || value.contains('\n')
        || value.contains('"')
        || value.starts_with(['-', '?', '|', '>', '!', '%', '@', '`', '*', '&']);
    if needs_quote {
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}
