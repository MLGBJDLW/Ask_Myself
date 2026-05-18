//! Persona profiles for shaping assistant behavior.

use crate::db::Database;
use crate::error::CoreError;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaProfile {
    pub id: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub default_skill_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePersonaInput {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub instructions: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub default_skill_ids: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

fn builtin_profile(
    id: &str,
    name: &str,
    description: &str,
    instructions: &str,
    default_skill_ids: &[&str],
) -> PersonaProfile {
    PersonaProfile {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        instructions: instructions.to_string(),
        enabled: true,
        builtin: true,
        default_skill_ids: default_skill_ids
            .iter()
            .map(|id| (*id).to_string())
            .collect(),
        created_at: String::new(),
        updated_at: String::new(),
    }
}

pub fn builtin_personas() -> Vec<PersonaProfile> {
    vec![
        builtin_profile(
            "default",
            "Default",
            "Balanced workspace assistant for general reasoning, writing, research, and execution.",
            "",
            &[],
        ),
        builtin_profile(
            "novelist",
            "Novelist",
            "Narrative strategist for fiction planning, prose, scenes, canon, and revision.",
            "Role: Long-form fiction partner, story architect, and continuity editor.\nIdentity: Helps the user turn story intent into durable canon, compelling scenes, and prose that still sounds like the user's work.\nCommunication style: Editorial, concrete, and image-rich. Separate diagnosis, options, and draft text so the user can decide what to keep.\nOperating principles:\n- Treat project memory, supplied drafts, and user-provided canon as authoritative.\n- Start with story function: character want, conflict, turn, consequence, and reader promise.\n- Use structure models as lenses, not laws; choose the simplest frame that clarifies the current story problem.\n- Ask about missing canon only when invention would create continuity risk.\n- When drafting, preserve POV, tense, naming, genre promise, and established style unless asked to change them.\nBoundaries:\n- Do not silently invent major canon, final endings, or character motives that contradict provided material.\n- Keep craft method in skills; keep persona focused on role, taste, and workflow emphasis.",
            &["builtin-fiction-writing"],
        ),
        builtin_profile(
            "speaker",
            "Speaker",
            "Speechwriter and presentation coach for talks, pitches, scripts, and delivery.",
            "Role: Speechwriter, presentation strategist, and rehearsal coach.\nIdentity: Designs spoken material around audience, occasion, timing, persuasion, rhythm, and delivery constraints.\nCommunication style: Direct, performance-aware, and easy to read aloud. Prefer vivid spoken units over dense written paragraphs.\nOperating principles:\n- Start from audience, occasion, goal, speaker role, and time budget.\n- Build a clear arc: hook, stakes, proof, turn, close, and ask.\n- Make one memorable sentence the talk's spine.\n- Include delivery notes only when they help pacing, emphasis, staging, or slide sync.\n- When slides are requested, align script, deck structure, and speaker notes instead of treating them as separate artifacts.\nBoundaries:\n- Do not add unsupported claims or fake credentials for persuasion.\n- Keep delivery coaching practical and tied to the speaker's actual context.",
            &[
                "builtin-speechwriting",
                "builtin-pptx-presentation-design",
                "builtin-visual-explanations",
            ],
        ),
        builtin_profile(
            "researcher",
            "Researcher",
            "Evidence-first analyst for research, synthesis, comparison, and uncertainty.",
            "Role: Research analyst, source critic, and synthesis editor.\nIdentity: Frames research questions, evaluates source quality, extracts claims, compares evidence, and marks uncertainty clearly.\nCommunication style: Precise, transparent, and citation-aware. Lead with what is supported, what is inferred, and what is unknown.\nOperating principles:\n- Start by clarifying the decision, scope, time sensitivity, and confidence needed.\n- Prefer primary, official, and current authoritative sources when facts can change.\n- Do not treat retrieval results as evidence unless the final answer explicitly uses and cites them.\n- Separate facts, interpretations, assumptions, contradictions, and open questions.\n- Surface evidence gaps instead of smoothing them over.\nBoundaries:\n- Do not overclaim from weak or single-source evidence.\n- Do not use web or knowledge-base snippets as final evidence without checking the underlying content when tools allow it.",
            &["builtin-research-synthesis", "builtin-evidence-first"],
        ),
        builtin_profile(
            "editor",
            "Editor",
            "Structural and line editor for clarity, tone, format, and author intent.",
            "Role: Structural editor, line editor, and style guardian.\nIdentity: Improves clarity, structure, tone, grammar, and format while preserving the user's intent and recognizable voice.\nCommunication style: Concise, concrete, and revision-oriented. Explain only edits that change meaning, structure, risk, or tone.\nOperating principles:\n- Identify the edit level first: structural, line, copy, format, or rewrite.\n- Preserve the author's claim unless the user asks for argument changes.\n- Tighten wording, remove repetition, and make transitions explicit.\n- Match the target artifact: article, memo, email, brief, DOCX report, slide notes, or publication draft.\n- For Office documents, use real document formatting rather than leaving Markdown markers in the output.\nBoundaries:\n- Label material changes when adding, cutting, or shifting claims.\n- Do not flatten intentional style into generic corporate prose.",
            &[
                "builtin-editorial-revision",
                "builtin-docx-document-design",
                "builtin-office-document-design",
            ],
        ),
    ]
}

pub fn builtin_persona_by_id(id: &str) -> Option<PersonaProfile> {
    builtin_personas()
        .into_iter()
        .find(|persona| persona.id == id)
}

pub fn is_builtin_persona_id(id: &str) -> bool {
    builtin_persona_by_id(id).is_some()
}

fn normalize_default_skill_ids(ids: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for id in ids {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_string()) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn normalize_persona_input(input: &SavePersonaInput) -> Result<SavePersonaInput, CoreError> {
    let name = input
        .name
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let description = input.description.trim().to_string();
    let instructions = input.instructions.trim().to_string();

    if name.is_empty() {
        return Err(CoreError::InvalidInput(
            "Persona name cannot be empty".into(),
        ));
    }
    if instructions.is_empty() {
        return Err(CoreError::InvalidInput(
            "Persona instructions cannot be empty".into(),
        ));
    }
    if description.len() > 2000 {
        return Err(CoreError::InvalidInput(
            "Persona description is too long (max 2000 chars)".into(),
        ));
    }
    if instructions.len() > 12_000 {
        return Err(CoreError::InvalidInput(
            "Persona instructions are too long (max 12000 chars)".into(),
        ));
    }

    Ok(SavePersonaInput {
        id: input.id.clone(),
        name,
        description,
        instructions,
        enabled: input.enabled,
        default_skill_ids: normalize_default_skill_ids(&input.default_skill_ids),
    })
}

fn serialize_default_skill_ids(ids: &[String]) -> Result<String, CoreError> {
    serde_json::to_string(&normalize_default_skill_ids(ids)).map_err(CoreError::from)
}

fn parse_default_skill_ids(raw: String) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(&raw)
        .map(|ids| normalize_default_skill_ids(&ids))
        .unwrap_or_default()
}

fn persona_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PersonaProfile> {
    Ok(PersonaProfile {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        instructions: row.get(3)?,
        enabled: row.get::<_, i32>(4)? != 0,
        builtin: false,
        default_skill_ids: parse_default_skill_ids(row.get(5)?),
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

pub fn list_personas(db: &Database) -> Result<Vec<PersonaProfile>, CoreError> {
    let mut out = builtin_personas();
    out.extend(db.list_user_personas()?);
    Ok(out)
}

pub fn persona_by_id(db: &Database, id: &str) -> Result<Option<PersonaProfile>, CoreError> {
    let id = id.trim();
    if id.is_empty() {
        return Ok(None);
    }
    if let Some(builtin) = builtin_persona_by_id(id) {
        return Ok(Some(builtin));
    }
    db.get_user_persona(id)
}

pub fn enabled_persona_by_id(db: &Database, id: &str) -> Result<Option<PersonaProfile>, CoreError> {
    Ok(persona_by_id(db, id)?.filter(|persona| persona.enabled))
}

pub fn build_persona_prompt_section(persona: Option<&PersonaProfile>) -> String {
    let Some(persona) = persona else {
        return String::new();
    };
    if persona.id == "default" || persona.instructions.trim().is_empty() {
        return String::new();
    }
    format!(
        "## Active Persona\n\nPersona: {} ({})\n\nInstructions: {}\n\nPersona instructions shape voice and workflow emphasis only. They do not override system, user, evidence, privacy, source-scope, or tool rules.",
        persona.name, persona.description, persona.instructions
    )
}

impl Database {
    pub fn list_user_personas(&self) -> Result<Vec<PersonaProfile>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, instructions, enabled, default_skill_ids_json, created_at, updated_at
             FROM personas
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], persona_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn save_persona(&self, input: &SavePersonaInput) -> Result<PersonaProfile, CoreError> {
        let input = normalize_persona_input(input)?;
        if input.id.as_deref().is_some_and(is_builtin_persona_id) {
            return Err(CoreError::InvalidInput(
                "Built-in personas are read-only".into(),
            ));
        }

        let skill_ids_json = serialize_default_skill_ids(&input.default_skill_ids)?;
        let conn = self.conn();
        let id = match &input.id {
            Some(existing_id) => {
                let affected = conn.execute(
                    "UPDATE personas
                     SET name = ?2, description = ?3, instructions = ?4, enabled = ?5,
                         default_skill_ids_json = ?6, updated_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![
                        existing_id,
                        &input.name,
                        &input.description,
                        &input.instructions,
                        input.enabled as i32,
                        &skill_ids_json
                    ],
                )?;
                if affected == 0 {
                    return Err(CoreError::NotFound(format!("Persona {existing_id}")));
                }
                existing_id.clone()
            }
            None => {
                let new_id = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO personas
                        (id, name, description, instructions, enabled, default_skill_ids_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        &new_id,
                        &input.name,
                        &input.description,
                        &input.instructions,
                        input.enabled as i32,
                        &skill_ids_json
                    ],
                )?;
                new_id
            }
        };
        drop(conn);
        self.get_user_persona(&id)?
            .ok_or_else(|| CoreError::NotFound(format!("Persona {id}")))
    }

    pub fn delete_persona(&self, id: &str) -> Result<(), CoreError> {
        if is_builtin_persona_id(id) {
            return Err(CoreError::InvalidInput(
                "Built-in personas cannot be deleted".into(),
            ));
        }
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM personas WHERE id = ?1", rusqlite::params![id])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Persona {id}")));
        }
        conn.execute(
            "UPDATE conversations
             SET persona_id = NULL, updated_at = datetime('now')
             WHERE persona_id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    pub fn toggle_persona(&self, id: &str, enabled: bool) -> Result<(), CoreError> {
        if is_builtin_persona_id(id) {
            return Err(CoreError::InvalidInput(
                "Built-in personas are always enabled".into(),
            ));
        }
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE personas SET enabled = ?2, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![id, enabled as i32],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Persona {id}")));
        }
        Ok(())
    }

    pub fn get_user_persona(&self, id: &str) -> Result<Option<PersonaProfile>, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, description, instructions, enabled, default_skill_ids_json, created_at, updated_at
             FROM personas
             WHERE id = ?1",
            rusqlite::params![id],
            persona_from_row,
        )
        .optional()
        .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_personas_are_listed() {
        let db = Database::open_memory().unwrap();
        let personas = list_personas(&db).unwrap();
        assert!(personas.iter().any(|p| p.id == "default" && p.builtin));
        assert!(personas.iter().any(|p| p.id == "researcher" && p.builtin));
        assert!(personas.iter().any(|p| {
            p.id == "novelist"
                && p.default_skill_ids
                    .iter()
                    .any(|id| id == "builtin-fiction-writing")
        }));
    }

    #[test]
    fn test_save_user_persona_round_trip() {
        let db = Database::open_memory().unwrap();
        let saved = db
            .save_persona(&SavePersonaInput {
                id: None,
                name: "  Careful analyst  ".into(),
                description: " Evidence-oriented ".into(),
                instructions: "Ask one clarifying question only when blocked.".into(),
                enabled: true,
                default_skill_ids: vec![
                    "builtin-evidence-first".into(),
                    "builtin-evidence-first".into(),
                    "  ".into(),
                ],
            })
            .unwrap();

        assert!(!saved.builtin);
        assert_eq!(saved.name, "Careful analyst");
        assert_eq!(saved.default_skill_ids, vec!["builtin-evidence-first"]);

        let resolved = persona_by_id(&db, &saved.id).unwrap().unwrap();
        assert_eq!(
            resolved.instructions,
            "Ask one clarifying question only when blocked."
        );
    }

    #[test]
    fn test_build_persona_prompt_section_ignores_default() {
        let default = builtin_persona_by_id("default").unwrap();
        assert!(build_persona_prompt_section(Some(&default)).is_empty());

        let researcher = builtin_persona_by_id("researcher").unwrap();
        let section = build_persona_prompt_section(Some(&researcher));
        assert!(section.contains("Active Persona"));
        assert!(section.contains("Researcher"));
        assert!(section.contains("source critic"));
    }

    #[test]
    fn test_builtin_personas_are_read_only() {
        let db = Database::open_memory().unwrap();
        let err = db
            .save_persona(&SavePersonaInput {
                id: Some("researcher".into()),
                name: "Changed".into(),
                description: String::new(),
                instructions: "Do something else.".into(),
                enabled: true,
                default_skill_ids: Vec::new(),
            })
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));
    }
}
