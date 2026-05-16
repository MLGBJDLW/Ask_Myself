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
    let fuzzy = jaro_winkler(&normalize_text(&skill.name), query_normalized)
        .max(jaro_winkler(
            &normalize_text(&skill.description),
            query_normalized,
        ))
        .max(jaro_winkler(&surface, query_normalized)) as f32
        * 3.0;

    (lexical + phrase + fuzzy) / query_tokens.len() as f32
}

fn fallback_skill_order(a: &Skill, b: &Skill) -> std::cmp::Ordering {
    a.builtin
        .cmp(&b.builtin)
        .then_with(|| a.created_at.cmp(&b.created_at))
        .then_with(|| a.name.cmp(&b.name))
}

fn fallback_skills(mut skills: Vec<Skill>, max_skills: usize) -> Vec<Skill> {
    skills.sort_by(fallback_skill_order);
    skills.truncate(max_skills);
    skills
}

pub fn select_skills_from_pool(skills: Vec<Skill>, query: &str, max_skills: usize) -> Vec<Skill> {
    if skills.is_empty() || max_skills == 0 {
        return Vec::new();
    }

    let query_normalized = normalize_text(query);
    let query_tokens = enrich_tokens(&tokenize(&query_normalized));
    if query_tokens.len() < 2 {
        return fallback_skills(skills, max_skills);
    }

    let mut scored: Vec<(f32, Skill)> = skills
        .into_iter()
        .map(|skill| (score_skill(&skill, &query_tokens, &query_normalized), skill))
        .collect();

    let top_score = scored
        .iter()
        .map(|(score, _)| *score)
        .fold(0.0_f32, f32::max);
    if top_score <= 0.05 {
        return fallback_skills(
            scored.into_iter().map(|(_, skill)| skill).collect(),
            max_skills,
        );
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
        .filter(|(score, _)| *score >= cutoff)
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
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        if let Some(skill) = by_id.get(id) {
            out.push(skill.clone());
            if out.len() >= max_skills {
                return out;
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

/// Return the skills active for a given user query.
///
/// Combines built-in (bundled) skills with enabled user skills from the DB,
/// then ranks by keyword overlap against the query. Falls back to returning
/// user skills before built-ins (capped at `max_skills`) when the query is
/// empty/short or when no skill matches, so user-authored abilities do not get
/// silently starved by bundled defaults.
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
