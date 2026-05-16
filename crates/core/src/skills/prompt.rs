use std::collections::BTreeMap;

use strsim::jaro_winkler;

use super::model::{Skill, SkillResourceEncoding, SkillResourceFile};
use super::selector::{enrich_tokens, normalize_text, score_skill, tokenize};

const MAX_SKILL_SECTION_CHARS: usize = 6_000;
const MAX_SKILL_BODY_EXCERPT_CHARS: usize = 1_400;
const MAX_SKILL_RESOURCE_EXCERPT_CHARS: usize = 700;

fn split_skill_sections(content: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let mut current_title = String::from("Overview");
    let mut current_body = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed
            .strip_prefix("## ")
            .or_else(|| trimmed.strip_prefix("### "))
        {
            if !current_body.is_empty() {
                sections.push((
                    current_title.clone(),
                    current_body.join("\n").trim().to_string(),
                ));
                current_body.clear();
            }
            current_title = title.trim().to_string();
        } else {
            current_body.push(line.to_string());
        }
    }
    if !current_body.is_empty() {
        sections.push((current_title, current_body.join("\n").trim().to_string()));
    }
    sections
}

fn truncate_excerpt(text: &str, max_chars: usize) -> String {
    let compact = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if compact.len() <= max_chars {
        return compact;
    }
    let mut cut = max_chars;
    while cut > 0 && !compact.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}...", compact[..cut].trim_end())
}

fn select_skill_section_excerpt(skill: &Skill, query: &str) -> String {
    let sections = split_skill_sections(&skill.content);
    if sections.is_empty() {
        return truncate_excerpt(&skill.content, MAX_SKILL_BODY_EXCERPT_CHARS);
    }

    let query_normalized = normalize_text(query);
    let query_tokens = enrich_tokens(&tokenize(&query_normalized));
    let mut selected = BTreeMap::new();
    for (index, (title, body)) in sections.iter().enumerate() {
        let title_lower = title.to_lowercase();
        if index == 0 || title_lower.contains("trigger") || title_lower.contains("rule") {
            selected.insert(
                index,
                format!("#### {title}\n{}", truncate_excerpt(body, 420)),
            );
        }
    }

    if query_tokens.len() >= 2 {
        let mut ranked: Vec<(f32, usize, String)> = sections
            .iter()
            .enumerate()
            .map(|(index, (title, body))| {
                let combined = format!("{title} {body}");
                (
                    score_skill(
                        &Skill {
                            id: String::new(),
                            name: title.clone(),
                            description: String::new(),
                            content: combined.clone(),
                            enabled: true,
                            created_at: String::new(),
                            updated_at: String::new(),
                            builtin: false,
                            resources: Vec::new(),
                            resource_bundle: Vec::new(),
                        },
                        &query_tokens,
                        &query_normalized,
                    ),
                    index,
                    format!("#### {title}\n{}", truncate_excerpt(body, 420)),
                )
            })
            .collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        for (score, index, excerpt) in ranked.into_iter().take(3) {
            if score > 0.1 {
                selected.entry(index).or_insert(excerpt);
            }
        }
    }

    let mut combined = selected.into_values().collect::<Vec<_>>().join("\n\n");
    if combined.len() > MAX_SKILL_BODY_EXCERPT_CHARS {
        combined = truncate_excerpt(&combined, MAX_SKILL_BODY_EXCERPT_CHARS);
    }
    combined
}

fn select_resource_excerpt(skill: &Skill, query: &str) -> String {
    let query_normalized = normalize_text(query);
    let query_tokens = enrich_tokens(&tokenize(&query_normalized));
    let mut ranked: Vec<(f32, &SkillResourceFile)> = skill
        .resource_bundle
        .iter()
        .filter(|resource| matches!(resource.encoding, SkillResourceEncoding::Utf8))
        .map(|resource| {
            let text = format!("{} {}", resource.path, resource.content);
            let score = if query_tokens.is_empty() {
                0.0
            } else {
                let surface = normalize_text(&text);
                let lexical = query_tokens
                    .iter()
                    .filter(|token| surface.contains(token.as_str()))
                    .count() as f32;
                let fuzzy = jaro_winkler(&surface, &query_normalized) as f32;
                lexical + fuzzy
            };
            let score = score
                + if resource.path.starts_with("references/") {
                    3.0
                } else {
                    0.0
                }
                + if resource.path.contains("playbook") {
                    2.0
                } else {
                    0.0
                };
            (score, resource)
        })
        .collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut rendered = Vec::new();
    for (index, (_score, resource)) in ranked.into_iter().enumerate() {
        if index >= 2 {
            break;
        }
        rendered.push(format!(
            "##### {}\n{}",
            resource.path,
            truncate_excerpt(&resource.content, MAX_SKILL_RESOURCE_EXCERPT_CHARS)
        ));
    }
    rendered.join("\n\n")
}

/// Build a compact skills section string from a list of skills for injection
/// into the system prompt. The renderer uses progressive disclosure so each
/// skill contributes a concise, query-aware excerpt instead of dumping the
/// entire SKILL.md every turn.
pub fn build_skills_section_for_query(skills: &[Skill], query: &str) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut section = String::from(
        "\n\n## Active Skills\nUse these skill excerpts as active procedural guidance. If an excerpt is insufficient, call manage_skill with action \"view_skill\" and the skill_id to inspect the full skill before relying on details that are not shown here.\n",
    );
    for skill in skills {
        let body_excerpt = select_skill_section_excerpt(skill, query);
        let resource_excerpt = select_resource_excerpt(skill, query);
        section.push_str(&format!("\n### {}\n", skill.name));
        if !skill.description.trim().is_empty() {
            section.push_str(&format!("Use when: {}\n", skill.description.trim()));
        }
        if !body_excerpt.is_empty() {
            section.push_str(&format!("\n{}\n", body_excerpt));
        }
        if !resource_excerpt.is_empty() {
            section.push_str("\n#### Bundled Resources\n");
            section.push_str(&resource_excerpt);
            section.push('\n');
        }
        if section.len() >= MAX_SKILL_SECTION_CHARS {
            section = truncate_excerpt(&section, MAX_SKILL_SECTION_CHARS);
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
