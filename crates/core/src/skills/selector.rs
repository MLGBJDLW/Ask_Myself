use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::db::Database;
use crate::error::CoreError;
use strsim::jaro_winkler;

use super::model::Skill;
use super::registry::load_builtin_skills;

/// Tokenize a text into lowercase alphanumeric word tokens (length ≥ 2).
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_string())
        .collect()
}

pub(crate) fn normalize_text(text: &str) -> String {
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn token_aliases(token: &str) -> &'static [&'static str] {
    match token {
        "debug" | "diagnose" | "diagnostics" | "troubleshoot" | "failure" | "error" | "broken"
        | "regression" => &[
            "diagnose",
            "debug",
            "troubleshoot",
            "failure",
            "error",
            "regression",
        ],
        "agent" | "agents" | "claude" | "deepseek" | "eval" | "evaluation" | "benchmark"
        | "accuracy" | "hitrate" | "hit" | "routing" | "router" => &[
            "agent",
            "routing",
            "router",
            "tool",
            "tools",
            "evaluation",
            "benchmark",
            "quality",
            "diagnose",
        ],
        "frontend" | "ui" | "ux" | "interface" | "layout" | "style" | "styles" | "css"
        | "visual" | "color" | "colour" | "button" | "theme" => &[
            "frontend",
            "ui",
            "ux",
            "interface",
            "layout",
            "style",
            "visual",
            "design",
            "color",
        ],
        "refactor" | "refactoring" | "architecture" | "architectural" | "cleanup"
        | "maintainability" => &[
            "refactor",
            "refactoring",
            "architecture",
            "cleanup",
            "maintainability",
            "code",
        ],
        "tdd" | "test" | "tests" | "testing" | "coverage" => {
            &["tdd", "test", "tests", "testing", "coverage"]
        }
        "diagram" | "diagramming" | "draw" | "flow" | "flowchart" | "visualize" | "visualise"
        | "mermaid" | "workflow" => &["diagram", "flowchart", "visual", "workflow", "mermaid"],
        "slide" | "slides" | "deck" | "presentation" | "ppt" | "pptx" => {
            &["slides", "deck", "presentation", "pptx"]
        }
        "report" | "doc" | "docx" | "document" => &["report", "document", "docx"],
        "sheet" | "sheets" | "spreadsheet" | "workbook" | "xlsx" | "excel" => {
            &["spreadsheet", "workbook", "xlsx", "excel"]
        }
        "cite" | "citation" | "citations" | "source" | "sources" | "evidence" => {
            &["cite", "citation", "source", "evidence"]
        }
        _ if token.contains("诊断")
            || token.contains("排查")
            || token.contains("调试")
            || token.contains("报错")
            || token.contains("失败")
            || token.contains("不工作")
            || token.contains("回归") =>
        {
            &[
                "diagnose",
                "debug",
                "troubleshoot",
                "failure",
                "error",
                "regression",
            ]
        }
        _ if token.contains("命中")
            || token.contains("准确率")
            || token.contains("召回率")
            || token.contains("评测")
            || token.contains("基准")
            || token.contains("路由")
            || token.contains("工具选择")
            || token.contains("claude")
            || token.contains("deepseek")
            || token.contains("智能体") =>
        {
            &[
                "agent",
                "routing",
                "router",
                "tool",
                "tools",
                "evaluation",
                "benchmark",
                "quality",
                "diagnose",
            ]
        }
        _ if token.contains("界面")
            || token.contains("前端")
            || token.contains("样式")
            || token.contains("视觉")
            || token.contains("布局")
            || token.contains("颜色")
            || token.contains("按钮")
            || token.contains("主题")
            || token.contains("颜表情") =>
        {
            &[
                "frontend",
                "ui",
                "ux",
                "interface",
                "layout",
                "style",
                "visual",
                "design",
                "color",
            ]
        }
        _ if token.contains("重构")
            || token.contains("架构")
            || token.contains("代码细节")
            || token.contains("清理")
            || token.contains("维护性") =>
        {
            &[
                "refactor",
                "refactoring",
                "architecture",
                "cleanup",
                "maintainability",
                "code",
            ]
        }
        _ if token.contains("测试")
            || token.contains("覆盖率")
            || token.contains("测试驱动")
            || token.contains("先写测试") =>
        {
            &["tdd", "test", "tests", "testing", "coverage"]
        }
        _ if token.contains("小说")
            || token.contains("网文")
            || token.contains("故事")
            || token.contains("章节")
            || token.contains("角色")
            || token.contains("剧情")
            || token.contains("伏笔") =>
        {
            &[
                "fiction",
                "novel",
                "story",
                "chapter",
                "character",
                "plot",
                "chinese",
            ]
        }
        _ => &[],
    }
}

pub(crate) fn enrich_tokens(tokens: &[String]) -> Vec<String> {
    let mut expanded = BTreeSet::new();
    for token in tokens {
        expanded.insert(token.clone());
        for alias in token_aliases(token) {
            expanded.insert((*alias).to_string());
        }
    }
    expanded.into_iter().collect()
}

fn skill_surface_text(skill: &Skill) -> String {
    let mut parts = vec![skill.name.clone(), skill.description.clone()];
    parts.push(skill.content.chars().take(500).collect());
    for resource in &skill.resources {
        parts.push(resource.path.replace(['/', '-', '_', '.'], " "));
    }
    normalize_text(&parts.join(" "))
}

fn lexical_score(token_set: &HashSet<String>, query_tokens: &[String], weight: f32) -> f32 {
    query_tokens
        .iter()
        .filter(|token| token_set.contains(*token))
        .count() as f32
        * weight
}

/// Score a skill against a query using lexical overlap plus fuzzy intent
/// matching so semantic variants like "slide deck" still activate the PPTX
/// skill without exact keyword matches.
pub(crate) fn score_skill(skill: &Skill, query_tokens: &[String], query_normalized: &str) -> f32 {
    if query_tokens.is_empty() {
        return 0.0;
    }

    let desc_tokens: HashSet<String> = enrich_tokens(&tokenize(&skill.description))
        .into_iter()
        .collect();
    let content_head: String = skill.content.chars().take(600).collect();
    let content_tokens: HashSet<String> = enrich_tokens(&tokenize(&content_head))
        .into_iter()
        .collect();
    let name_tokens: HashSet<String> = enrich_tokens(&tokenize(&skill.name)).into_iter().collect();
    let resource_tokens: HashSet<String> = skill
        .resources
        .iter()
        .flat_map(|resource| tokenize(&resource.path.replace(['/', '-', '_', '.'], " ")))
        .collect();

    let lexical = lexical_score(&desc_tokens, query_tokens, 2.4)
        + lexical_score(&name_tokens, query_tokens, 2.0)
        + lexical_score(&resource_tokens, query_tokens, 1.5)
        + lexical_score(&content_tokens, query_tokens, 1.0);

    let surface = skill_surface_text(skill);
    let phrase = query_tokens
        .iter()
        .filter(|token| surface.contains(token.as_str()))
        .count() as f32
        * 0.35;
    let fuzzy_similarity = jaro_winkler(&normalize_text(&skill.name), query_normalized)
        .max(jaro_winkler(
            &normalize_text(&skill.description),
            query_normalized,
        ))
        .max(jaro_winkler(&surface, query_normalized)) as f32;
    let fuzzy = if fuzzy_similarity >= 0.82 {
        fuzzy_similarity * 3.0
    } else {
        0.0
    };

    (lexical + phrase + fuzzy) / query_tokens.len() as f32
}

fn fallback_skill_order(a: &Skill, b: &Skill) -> std::cmp::Ordering {
    a.builtin
        .cmp(&b.builtin)
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.name.cmp(&b.name))
}

fn skill_slug(skill: &Skill) -> String {
    skill
        .id
        .strip_prefix("builtin-")
        .unwrap_or(skill.id.as_str())
        .to_string()
}

fn normalize_selector(value: &str) -> String {
    value
        .trim()
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '$'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn selector_matches_skill(skill: &Skill, selector: &str) -> bool {
    let selector = selector.trim();
    if selector.is_empty() {
        return false;
    }
    let selector = selector.strip_prefix('$').unwrap_or(selector);
    let selector = selector.strip_prefix('@').unwrap_or(selector);
    let selector_normalized = normalize_selector(selector);
    let slug = skill_slug(skill);
    selector_normalized == normalize_selector(&skill.id)
        || selector_normalized == normalize_selector(&slug)
        || selector_normalized == normalize_selector(&skill.name)
        || selector_normalized == normalize_selector(&skill.interface.display_name)
}

fn is_skill_pinned(skill: &Skill, pinned_skill_ids: &[String]) -> bool {
    pinned_skill_ids
        .iter()
        .any(|id| selector_matches_skill(skill, id))
}

fn is_skill_explicitly_requested(skill: &Skill, query: &str) -> bool {
    let query_lower = query.to_lowercase();
    let slug = skill_slug(skill).to_lowercase();
    let name = skill.name.to_lowercase();
    let display = skill.interface.display_name.to_lowercase();

    query_lower.contains(&format!("${slug}"))
        || query_lower.contains(&format!("@{slug}"))
        || (!name.trim().is_empty() && query_lower.contains(&name))
        || (!display.trim().is_empty() && query_lower.contains(&display))
}

pub fn select_skills_from_pool(skills: Vec<Skill>, query: &str, max_skills: usize) -> Vec<Skill> {
    if skills.is_empty() || max_skills == 0 {
        return Vec::new();
    }

    let skills = skills
        .into_iter()
        .filter(|skill| {
            skill.policy.allow_implicit_invocation || is_skill_explicitly_requested(skill, query)
        })
        .collect::<Vec<_>>();
    if skills.is_empty() {
        return Vec::new();
    }

    let query_normalized = normalize_text(query);
    let query_tokens = enrich_tokens(&tokenize(&query_normalized));

    let mut scored: Vec<(f32, Skill)> = skills
        .into_iter()
        .map(|skill| {
            let explicit = is_skill_explicitly_requested(&skill, query);
            let score = if explicit {
                f32::MAX
            } else if query_tokens.len() >= 2 {
                score_skill(&skill, &query_tokens, &query_normalized)
            } else {
                0.0
            };
            (score, skill)
        })
        .collect();

    let top_score = scored
        .iter()
        .filter(|(score, _)| *score < f32::MAX)
        .map(|(score, _)| *score)
        .fold(0.0_f32, f32::max);
    let has_explicit = scored.iter().any(|(score, _)| *score == f32::MAX);
    if !has_explicit && (query_tokens.len() < 2 || top_score <= 0.05) {
        return Vec::new();
    }

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.builtin.cmp(&b.1.builtin))
            .then_with(|| a.1.name.cmp(&b.1.name))
    });

    let cutoff = (top_score * 0.55).max(0.18);
    scored
        .into_iter()
        .filter(|(score, _)| *score == f32::MAX || *score >= cutoff)
        .take(max_skills)
        .map(|(_, skill)| skill)
        .collect()
}

pub fn select_skills_from_pool_with_pinned(
    skills: Vec<Skill>,
    query: &str,
    max_skills: usize,
    pinned_skill_ids: &[String],
) -> Vec<Skill> {
    if skills.is_empty() || max_skills == 0 {
        return Vec::new();
    }

    let mut by_id = BTreeMap::new();
    for skill in &skills {
        by_id.insert(skill.id.clone(), skill.clone());
    }

    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for id in pinned_skill_ids {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }

        let builtin_alias;
        let skill = if let Some(skill) = by_id.get(id) {
            Some(skill)
        } else if id.starts_with("builtin-") {
            None
        } else {
            builtin_alias = format!("builtin-{id}");
            by_id.get(&builtin_alias)
        };

        if let Some(skill) = skill {
            if seen.insert(skill.id.clone()) {
                out.push(skill.clone());
                if out.len() >= max_skills {
                    return out;
                }
            }
        }
    }

    let ranked = select_skills_from_pool(skills, query, max_skills);
    for skill in ranked {
        if seen.insert(skill.id.clone()) {
            out.push(skill);
            if out.len() >= max_skills {
                break;
            }
        }
    }
    out
}

pub fn select_available_skills_from_pool_with_pinned(
    skills: Vec<Skill>,
    query: &str,
    pinned_skill_ids: &[String],
) -> Vec<Skill> {
    if skills.is_empty() {
        return Vec::new();
    }

    let query_normalized = normalize_text(query);
    let query_tokens = enrich_tokens(&tokenize(&query_normalized));

    let mut ranked = skills
        .into_iter()
        .filter_map(|skill| {
            let pinned = is_skill_pinned(&skill, pinned_skill_ids);
            let explicit = is_skill_explicitly_requested(&skill, query);
            if !skill.policy.allow_implicit_invocation && !pinned && !explicit {
                return None;
            }
            let score = if query_tokens.len() >= 2 {
                score_skill(&skill, &query_tokens, &query_normalized)
            } else {
                0.0
            };
            Some((pinned, explicit, score, skill))
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| fallback_skill_order(&a.3, &b.3))
    });

    ranked.into_iter().map(|(_, _, _, skill)| skill).collect()
}

pub fn select_available_skills_from_pool(skills: Vec<Skill>, query: &str) -> Vec<Skill> {
    select_available_skills_from_pool_with_pinned(skills, query, &[])
}

/// Return the skills active for a given user query.
///
/// Combines built-in (bundled) skills with enabled user skills from the DB,
/// then ranks by keyword overlap against the query. If no skill clearly
/// matches, returns no skills; the model can still call `manage_skill
/// list_skills` when it needs to discover the full catalog.
pub fn get_active_skills_for_query(
    db: &Database,
    query: &str,
    max_skills: usize,
) -> Result<Vec<Skill>, CoreError> {
    let mut all: Vec<Skill> = load_builtin_skills();
    all.extend(db.get_enabled_skills()?);
    Ok(select_skills_from_pool(all, query, max_skills))
}

pub fn get_active_skills_for_query_with_pinned(
    db: &Database,
    query: &str,
    max_skills: usize,
    pinned_skill_ids: &[String],
) -> Result<Vec<Skill>, CoreError> {
    let mut all: Vec<Skill> = load_builtin_skills();
    all.extend(db.get_enabled_skills()?);
    Ok(select_skills_from_pool_with_pinned(
        all,
        query,
        max_skills,
        pinned_skill_ids,
    ))
}

/// Return all skills that may be shown in the prompt as metadata.
///
/// Unlike `get_active_skills_for_query`, this does not inject skill bodies and
/// does not cap to a small number. Prompt rendering owns the final budget. The
/// selector still sorts relevant and pinned skills first, and hides skills whose
/// policy opts out of implicit invocation unless the user explicitly names them
/// or a persona pins them.
pub fn get_available_skills_for_query(db: &Database, query: &str) -> Result<Vec<Skill>, CoreError> {
    let mut all: Vec<Skill> = load_builtin_skills();
    all.extend(db.get_enabled_skills()?);
    Ok(select_available_skills_from_pool(all, query))
}

pub fn get_available_skills_for_query_with_pinned(
    db: &Database,
    query: &str,
    pinned_skill_ids: &[String],
) -> Result<Vec<Skill>, CoreError> {
    let mut all: Vec<Skill> = load_builtin_skills();
    all.extend(db.get_enabled_skills()?);
    Ok(select_available_skills_from_pool_with_pinned(
        all,
        query,
        pinned_skill_ids,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{
        SkillDependencies, SkillInterfaceMetadata, SkillPolicy, SkillResourceFile,
        SkillResourceInfo,
    };

    fn test_skill(id: &str, name: &str, description: &str) -> Skill {
        Skill {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            content: format!("# {name}\n{description}"),
            enabled: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            builtin: true,
            interface: SkillInterfaceMetadata::default(),
            dependencies: SkillDependencies::default(),
            policy: SkillPolicy::default(),
            source_path: None,
            resources: Vec::<SkillResourceInfo>::new(),
            resource_bundle: Vec::<SkillResourceFile>::new(),
        }
    }

    #[test]
    fn chinese_agent_hit_rate_query_selects_agent_quality_skill() {
        let skills = vec![
            test_skill(
                "agent-quality",
                "agent-quality",
                "Diagnose agent routing, tool selection, evaluation benchmarks, and quality misses.",
            ),
            test_skill(
                "office-document-design",
                "office-document-design",
                "Create polished documents, slides, and spreadsheets.",
            ),
        ];

        let selected = select_skills_from_pool(
            skills,
            "使用 DeepSeek 的情况下命中效率只有 85%，参考 Claude 顶级 agents 看看路由和工具选择。",
            2,
        );

        assert_eq!(
            selected.first().map(|skill| skill.id.as_str()),
            Some("agent-quality")
        );
    }

    #[test]
    fn chinese_frontend_style_query_selects_frontend_skill() {
        let skills = vec![
            test_skill(
                "frontend-design",
                "frontend-design",
                "Improve frontend UI, visual design, layout, styling, color, and interaction polish.",
            ),
            test_skill(
                "research-synthesis",
                "research-synthesis",
                "Synthesize sources into evidence-backed research notes.",
            ),
        ];

        let selected = select_skills_from_pool(
            skills,
            "颜表情被同色的底盖住了，界面样式需要改成透明背景。",
            2,
        );

        assert_eq!(
            selected.first().map(|skill| skill.id.as_str()),
            Some("frontend-design")
        );
    }
}
