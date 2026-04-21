//! Skills module — user-defined instruction snippets injected into the system prompt.
//!
//! Supports Anthropic Agent Skills standard format: built-in skills are bundled
//! as SKILL.md files with YAML frontmatter, while user-created skills live in
//! the database. Skills are selected per-query via keyword overlap against the
//! description and content.

use crate::db::Database;
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A skill (instruction snippet) — either a built-in (bundled SKILL.md) or a
/// user-created record in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    /// Concise trigger-match description (when to activate this skill).
    #[serde(default)]
    pub description: String,
    pub content: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    /// True when the skill originates from a bundled SKILL.md file. Built-in
    /// skills are read-only in the UI.
    #[serde(default)]
    pub builtin: bool,
}

/// Input for creating or updating a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSkillInput {
    /// `None` = create new, `Some` = update existing.
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub content: String,
    pub enabled: bool,
}

/// Parsed YAML frontmatter of a SKILL.md file.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Bundled built-in skills. Content is embedded at compile time via
/// `include_str!` so the binary is self-contained.
static BUILTIN_SKILLS: &[(&str, &str)] = &[
    (
        "visual-explanations",
        include_str!("../assets/skills/visual-explanations/SKILL.md"),
    ),
    (
        "office-document-design",
        include_str!("../assets/skills/office-document-design/SKILL.md"),
    ),
    (
        "evidence-first",
        include_str!("../assets/skills/evidence-first/SKILL.md"),
    ),
];

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

fn split_frontmatter(rest: &str) -> Result<(&str, &str), CoreError> {
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
    for (slug, content) in BUILTIN_SKILLS {
        match parse_skill_file(content) {
            Ok((fm, body)) => {
                out.push(Skill {
                    id: format!("builtin-{slug}"),
                    name: fm.name,
                    description: fm.description,
                    content: body,
                    enabled: true,
                    created_at: String::new(),
                    updated_at: String::new(),
                    builtin: true,
                });
            }
            Err(e) => {
                tracing::error!(skill = slug, error = %e, "Failed to parse bundled SKILL.md");
            }
        }
    }
    out
}

fn normalize_skill_input(input: &SaveSkillInput) -> Result<SaveSkillInput, CoreError> {
    let name = input
        .name
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let description = input.description.trim().to_string();
    let content = input.content.trim().to_string();

    if name.is_empty() {
        return Err(CoreError::InvalidInput("Skill name cannot be empty".into()));
    }

    if content.is_empty() {
        return Err(CoreError::InvalidInput(
            "Skill content cannot be empty".into(),
        ));
    }

    if description.len() > 2000 {
        return Err(CoreError::InvalidInput(
            "Skill description is too long (max 2000 chars)".into(),
        ));
    }

    Ok(SaveSkillInput {
        id: input.id.clone(),
        name,
        description,
        content,
        enabled: input.enabled,
    })
}

impl Database {
    /// List all user skills, newest first. Built-in skills are NOT included.
    pub fn list_skills(&self) -> Result<Vec<Skill>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, content, enabled, created_at, updated_at
             FROM skills
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                content: row.get(3)?,
                enabled: row.get::<_, i32>(4)? != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                builtin: false,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Create or update a user skill.
    pub fn save_skill(&self, input: &SaveSkillInput) -> Result<Skill, CoreError> {
        let input = normalize_skill_input(input)?;
        if input
            .id
            .as_deref()
            .is_some_and(|id| id.starts_with("builtin-"))
        {
            return Err(CoreError::InvalidInput(
                "Built-in skills are read-only".into(),
            ));
        }

        let conn = self.conn();
        let id = match &input.id {
            Some(existing_id) => {
                conn.execute(
                    "UPDATE skills
                     SET name = ?2, description = ?3, content = ?4, enabled = ?5,
                         updated_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![
                        existing_id,
                        &input.name,
                        &input.description,
                        &input.content,
                        input.enabled as i32
                    ],
                )?;
                existing_id.clone()
            }
            None => {
                let new_id = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO skills (id, name, description, content, enabled)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        &new_id,
                        &input.name,
                        &input.description,
                        &input.content,
                        input.enabled as i32
                    ],
                )?;
                new_id
            }
        };
        drop(conn);
        self.get_skill(&id)
    }

    /// Delete a user skill by ID.
    pub fn delete_skill(&self, id: &str) -> Result<(), CoreError> {
        if id.starts_with("builtin-") {
            return Err(CoreError::InvalidInput(
                "Built-in skills cannot be deleted".into(),
            ));
        }
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM skills WHERE id = ?1", rusqlite::params![id])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Skill {id}")));
        }
        Ok(())
    }

    /// Toggle a user skill's enabled state.
    pub fn toggle_skill(&self, id: &str, enabled: bool) -> Result<(), CoreError> {
        if id.starts_with("builtin-") {
            return Err(CoreError::InvalidInput(
                "Built-in skills cannot be toggled via this API (always on)".into(),
            ));
        }
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE skills SET enabled = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, enabled as i32],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Skill {id}")));
        }
        Ok(())
    }

    /// Get only enabled user skills (built-ins are NOT included here — combine
    /// with `load_builtin_skills()` for the full active set).
    pub fn get_enabled_skills(&self) -> Result<Vec<Skill>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, content, enabled, created_at, updated_at
             FROM skills
             WHERE enabled = 1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                content: row.get(3)?,
                enabled: true,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                builtin: false,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn get_skill(&self, id: &str) -> Result<Skill, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, description, content, enabled, created_at, updated_at
             FROM skills
             WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(Skill {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    content: row.get(3)?,
                    enabled: row.get::<_, i32>(4)? != 0,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    builtin: false,
                })
            },
        )
        .map_err(|_| CoreError::NotFound(format!("Skill {id}")))
    }
}

/// Tokenize a text into lowercase alphanumeric word tokens (length ≥ 2).
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2)
        .map(|w| w.to_string())
        .collect()
}

/// Score a skill against a query-token set.
///
/// Description matches are weighted 2×, name 1.5×, content (first 300 chars) 1×.
fn score_skill(skill: &Skill, query_tokens: &[String]) -> f32 {
    if query_tokens.is_empty() {
        return 0.0;
    }

    let desc_tokens: Vec<String> = tokenize(&skill.description);
    let content_head: String = skill.content.chars().take(300).collect();
    let content_tokens: Vec<String> = tokenize(&content_head);
    let name_tokens: Vec<String> = tokenize(&skill.name);

    let mut score: f32 = 0.0;
    for q in query_tokens {
        if desc_tokens.iter().any(|t| t == q) {
            score += 2.0;
        } else if name_tokens.iter().any(|t| t == q) {
            score += 1.5;
        } else if content_tokens.iter().any(|t| t == q) {
            score += 1.0;
        }
    }
    score / query_tokens.len() as f32
}

/// Return the skills active for a given user query.
///
/// Combines built-in (bundled) skills with enabled user skills from the DB,
/// then ranks by keyword overlap against the query. Falls back to returning
/// ALL skills (capped at `max_skills`) when the query is empty/short or when
/// no skill matches — preserving always-on behaviour for non-task prompts.
pub fn get_active_skills_for_query(
    db: &Database,
    query: &str,
    max_skills: usize,
) -> Result<Vec<Skill>, CoreError> {
    let mut all: Vec<Skill> = load_builtin_skills();
    all.extend(db.get_enabled_skills()?);

    if all.is_empty() {
        return Ok(all);
    }

    let query_tokens = tokenize(query);

    if query_tokens.len() < 2 {
        all.truncate(max_skills);
        return Ok(all);
    }

    let mut scored: Vec<(f32, Skill)> = all
        .into_iter()
        .map(|s| (score_skill(&s, &query_tokens), s))
        .collect();

    if scored.iter().all(|(s, _)| *s == 0.0) {
        let mut out: Vec<Skill> = scored.into_iter().map(|(_, s)| s).collect();
        out.truncate(max_skills);
        return Ok(out);
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored
        .into_iter()
        .filter(|(s, _)| *s > 0.0)
        .take(max_skills)
        .map(|(_, s)| s)
        .collect())
}

/// Build a skills section string from a list of skills for injection into the system prompt.
/// Returns an empty string if no skills are provided.
pub fn build_skills_section(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut section = String::from("\n\n## Active Skills\n");
    for skill in skills {
        section.push_str(&format!("\n### {}\n{}\n", skill.name, skill.content));
    }
    section
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_crud() {
        let db = Database::open_memory().unwrap();
        db.conn().execute("DELETE FROM skills", []).unwrap();
        assert!(db.list_skills().unwrap().is_empty());

        let skill = db
            .save_skill(&SaveSkillInput {
                id: None,
                name: "Test Skill".into(),
                description: "Trigger for tests".into(),
                content: "Do something useful".into(),
                enabled: true,
            })
            .unwrap();
        assert_eq!(skill.name, "Test Skill");
        assert_eq!(skill.description, "Trigger for tests");
        assert!(skill.enabled);

        let all = db.list_skills().unwrap();
        assert_eq!(all.len(), 1);

        let updated = db
            .save_skill(&SaveSkillInput {
                id: Some(skill.id.clone()),
                name: "Updated Skill".into(),
                description: "Updated desc".into(),
                content: "Updated content".into(),
                enabled: false,
            })
            .unwrap();
        assert_eq!(updated.name, "Updated Skill");
        assert_eq!(updated.description, "Updated desc");
        assert!(!updated.enabled);

        db.toggle_skill(&skill.id, true).unwrap();
        let enabled = db.get_enabled_skills().unwrap();
        assert_eq!(enabled.len(), 1);

        db.delete_skill(&skill.id).unwrap();
        assert!(db.list_skills().unwrap().is_empty());
    }

    #[test]
    fn test_get_enabled_skills_filters() {
        let db = Database::open_memory().unwrap();
        db.conn().execute("DELETE FROM skills", []).unwrap();

        db.save_skill(&SaveSkillInput {
            id: None,
            name: "Enabled".into(),
            description: "".into(),
            content: "content".into(),
            enabled: true,
        })
        .unwrap();
        db.save_skill(&SaveSkillInput {
            id: None,
            name: "Disabled".into(),
            description: "".into(),
            content: "content".into(),
            enabled: false,
        })
        .unwrap();

        let enabled = db.get_enabled_skills().unwrap();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "Enabled");
    }

    #[test]
    fn test_build_skills_section_empty() {
        assert_eq!(build_skills_section(&[]), "");
    }

    #[test]
    fn test_build_skills_section_with_skills() {
        let skills = vec![Skill {
            id: "1".into(),
            name: "Concise".into(),
            description: "Be brief".into(),
            content: "Be brief.".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
        }];
        let section = build_skills_section(&skills);
        assert!(section.contains("## Active Skills"));
        assert!(section.contains("### Concise"));
        assert!(section.contains("Be brief."));
    }

    #[test]
    fn test_delete_nonexistent_skill() {
        let db = Database::open_memory().unwrap();
        let result = db.delete_skill("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_save_skill_rejects_blank_fields() {
        let db = Database::open_memory().unwrap();
        assert!(db
            .save_skill(&SaveSkillInput {
                id: None,
                name: "   ".into(),
                description: "".into(),
                content: "content".into(),
                enabled: true,
            })
            .is_err());
        assert!(db
            .save_skill(&SaveSkillInput {
                id: None,
                name: "Name".into(),
                description: "".into(),
                content: "   ".into(),
                enabled: true,
            })
            .is_err());
    }

    #[test]
    fn test_toggle_nonexistent_skill() {
        let db = Database::open_memory().unwrap();
        let result = db.toggle_skill("nonexistent", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_skill_file_basic() {
        let content =
            "---\nname: my-skill\ndescription: Test description\n---\n\n## Body\n\nSome content.\n";
        let (fm, body) = parse_skill_file(content).unwrap();
        assert_eq!(fm.name, "my-skill");
        assert_eq!(fm.description, "Test description");
        assert!(body.starts_with("## Body"));
        assert!(body.contains("Some content."));
    }

    #[test]
    fn test_parse_skill_file_missing_frontmatter() {
        assert!(parse_skill_file("# No frontmatter").is_err());
        assert!(parse_skill_file("---\nname: x\n# never closed").is_err());
    }

    #[test]
    fn test_load_builtin_skills() {
        let skills = load_builtin_skills();
        assert_eq!(skills.len(), 3, "three bundled SKILL.md files must parse");
        for s in &skills {
            assert!(s.builtin);
            assert!(!s.name.is_empty());
            assert!(!s.description.is_empty(), "description must be set");
            assert!(!s.content.is_empty());
            assert!(s.id.starts_with("builtin-"));
        }
        assert!(skills.iter().any(|s| s.id == "builtin-visual-explanations"));
        assert!(skills
            .iter()
            .any(|s| s.id == "builtin-office-document-design"));
        assert!(skills.iter().any(|s| s.id == "builtin-evidence-first"));
    }

    #[test]
    fn test_builtin_skills_reject_write_operations() {
        let db = Database::open_memory().unwrap();
        assert!(db.delete_skill("builtin-visual-explanations").is_err());
        assert!(db
            .toggle_skill("builtin-visual-explanations", false)
            .is_err());
        assert!(db
            .save_skill(&SaveSkillInput {
                id: Some("builtin-visual-explanations".into()),
                name: "x".into(),
                description: "".into(),
                content: "y".into(),
                enabled: true,
            })
            .is_err());
    }

    #[test]
    fn test_get_active_skills_short_query_returns_all() {
        let db = Database::open_memory().unwrap();
        db.conn().execute("DELETE FROM skills", []).unwrap();

        let active = get_active_skills_for_query(&db, "", 10).unwrap();
        assert_eq!(active.len(), 3);
    }

    #[test]
    fn test_get_active_skills_matches_description() {
        let db = Database::open_memory().unwrap();
        db.conn().execute("DELETE FROM skills", []).unwrap();

        let active = get_active_skills_for_query(
            &db,
            "can you draw me a flowchart of the login workflow?",
            5,
        )
        .unwrap();
        assert!(!active.is_empty());
        assert!(
            active.iter().any(|s| s.id == "builtin-visual-explanations"),
            "visual-explanations skill should match a flowchart query"
        );
    }

    #[test]
    fn test_get_active_skills_no_match_falls_back_all() {
        let db = Database::open_memory().unwrap();
        db.conn().execute("DELETE FROM skills", []).unwrap();

        let active = get_active_skills_for_query(&db, "zzzxxx qqqyyy wwwvvv", 10).unwrap();
        assert_eq!(active.len(), 3, "fallback: return all built-ins");
    }

    #[test]
    fn test_export_skill_to_md_roundtrip() {
        let skill = Skill {
            id: "user-1".into(),
            name: "Test Name".into(),
            description: "When to use it".into(),
            content: "## Rules\n\n1. Do X\n".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
            builtin: false,
        };
        let md = export_skill_to_md(&skill);
        let (fm, body) = parse_skill_file(&md).unwrap();
        assert_eq!(fm.name, "Test Name");
        assert_eq!(fm.description, "When to use it");
        assert!(body.contains("## Rules"));
        assert!(body.contains("Do X"));
    }
}
