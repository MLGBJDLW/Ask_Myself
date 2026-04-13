//! Skills module — user-defined instruction snippets injected into the system prompt.

use crate::db::Database;
use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A user-defined skill (instruction snippet).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub content: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Input for creating or updating a skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSkillInput {
    /// `None` = create new, `Some` = update existing.
    pub id: Option<String>,
    pub name: String,
    pub content: String,
    pub enabled: bool,
}

fn normalize_skill_input(input: &SaveSkillInput) -> Result<SaveSkillInput, CoreError> {
    let name = input
        .name
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let content = input.content.trim().to_string();

    if name.is_empty() {
        return Err(CoreError::InvalidInput("Skill name cannot be empty".into()));
    }

    if content.is_empty() {
        return Err(CoreError::InvalidInput(
            "Skill content cannot be empty".into(),
        ));
    }

    Ok(SaveSkillInput {
        id: input.id.clone(),
        name,
        content,
        enabled: input.enabled,
    })
}

impl Database {
    /// List all skills, newest first.
    pub fn list_skills(&self) -> Result<Vec<Skill>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, content, enabled, created_at, updated_at
             FROM skills
             ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                content: row.get(2)?,
                enabled: row.get::<_, i32>(3)? != 0,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Create or update a skill.
    pub fn save_skill(&self, input: &SaveSkillInput) -> Result<Skill, CoreError> {
        let input = normalize_skill_input(input)?;
        let conn = self.conn();
        let id = match &input.id {
            Some(existing_id) => {
                conn.execute(
                    "UPDATE skills
                     SET name = ?2, content = ?3, enabled = ?4, updated_at = datetime('now')
                     WHERE id = ?1",
                    rusqlite::params![
                        existing_id,
                        &input.name,
                        &input.content,
                        input.enabled as i32
                    ],
                )?;
                existing_id.clone()
            }
            None => {
                let new_id = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO skills (id, name, content, enabled)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![&new_id, &input.name, &input.content, input.enabled as i32],
                )?;
                new_id
            }
        };
        drop(conn);
        self.get_skill(&id)
    }

    /// Delete a skill by ID.
    pub fn delete_skill(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM skills WHERE id = ?1", rusqlite::params![id])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Skill {id}")));
        }
        Ok(())
    }

    /// Toggle a skill's enabled state.
    pub fn toggle_skill(&self, id: &str, enabled: bool) -> Result<(), CoreError> {
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

    /// Get only enabled skills.
    pub fn get_enabled_skills(&self) -> Result<Vec<Skill>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, content, enabled, created_at, updated_at
             FROM skills
             WHERE enabled = 1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Skill {
                id: row.get(0)?,
                name: row.get(1)?,
                content: row.get(2)?,
                enabled: true,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
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
            "SELECT id, name, content, enabled, created_at, updated_at
             FROM skills
             WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(Skill {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    content: row.get(2)?,
                    enabled: row.get::<_, i32>(3)? != 0,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .map_err(|_| CoreError::NotFound(format!("Skill {id}")))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_crud() {
        let db = Database::open_memory().unwrap();

        // Clear seeded default skills so we start fresh.
        db.conn().execute("DELETE FROM skills", []).unwrap();
        assert!(db.list_skills().unwrap().is_empty());

        // Create a skill.
        let skill = db
            .save_skill(&SaveSkillInput {
                id: None,
                name: "Test Skill".into(),
                content: "Do something useful".into(),
                enabled: true,
            })
            .unwrap();
        assert_eq!(skill.name, "Test Skill");
        assert!(skill.enabled);

        // List returns it.
        let all = db.list_skills().unwrap();
        assert_eq!(all.len(), 1);

        // Update it.
        let updated = db
            .save_skill(&SaveSkillInput {
                id: Some(skill.id.clone()),
                name: "Updated Skill".into(),
                content: "Updated content".into(),
                enabled: false,
            })
            .unwrap();
        assert_eq!(updated.name, "Updated Skill");
        assert!(!updated.enabled);

        // Toggle.
        db.toggle_skill(&skill.id, true).unwrap();
        let enabled = db.get_enabled_skills().unwrap();
        assert_eq!(enabled.len(), 1);

        // Delete.
        db.delete_skill(&skill.id).unwrap();
        assert!(db.list_skills().unwrap().is_empty());
    }

    #[test]
    fn test_get_enabled_skills_filters() {
        let db = Database::open_memory().unwrap();

        // Clear seeded default skills so counts are deterministic.
        db.conn().execute("DELETE FROM skills", []).unwrap();

        db.save_skill(&SaveSkillInput {
            id: None,
            name: "Enabled".into(),
            content: "content".into(),
            enabled: true,
        })
        .unwrap();
        db.save_skill(&SaveSkillInput {
            id: None,
            name: "Disabled".into(),
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
            content: "Be brief.".into(),
            enabled: true,
            created_at: String::new(),
            updated_at: String::new(),
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
                content: "content".into(),
                enabled: true,
            })
            .is_err());
        assert!(db
            .save_skill(&SaveSkillInput {
                id: None,
                name: "Name".into(),
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
}
