//! Project CRUD — groups conversations under named workspaces.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A project groups conversations and shares context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub color: String,
    pub system_prompt: String,
    pub source_scope: Option<Vec<String>>,
    pub archived: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Input for creating a new project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectInput {
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub system_prompt: Option<String>,
    pub source_scope: Option<Vec<String>>,
}

/// Input for updating an existing project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub system_prompt: Option<String>,
    pub source_scope: Option<Vec<String>>,
    pub archived: Option<bool>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn parse_source_scope(json: Option<String>) -> Option<Vec<String>> {
    json.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
}

fn serialize_source_scope(scope: Option<&[String]>) -> Result<Option<String>, CoreError> {
    match scope {
        Some(items) => Ok(Some(serde_json::to_string(items)?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// Create a new project. Returns the persisted row.
    pub fn create_project(&self, input: &CreateProjectInput) -> Result<Project, CoreError> {
        let id = new_id();
        let description = input.description.as_deref().unwrap_or("");
        let icon = input.icon.as_deref().unwrap_or("");
        let color = input.color.as_deref().unwrap_or("");
        let system_prompt = input.system_prompt.as_deref().unwrap_or("");
        let source_scope_json = serialize_source_scope(input.source_scope.as_deref())?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO projects (id, name, description, icon, color, system_prompt, source_scope_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![&id, &input.name, description, icon, color, system_prompt, &source_scope_json],
        )?;
        drop(conn);
        self.get_project(&id)
    }

    /// List all non-archived projects ordered by name.
    pub fn list_projects(&self) -> Result<Vec<Project>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, icon, color, system_prompt, source_scope_json, archived, created_at, updated_at
             FROM projects WHERE archived = 0 ORDER BY name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                icon: row.get(3)?,
                color: row.get(4)?,
                system_prompt: row.get(5)?,
                source_scope: parse_source_scope(row.get(6)?),
                archived: row.get::<_, i32>(7)? != 0,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get a single project by id.
    pub fn get_project(&self, id: &str) -> Result<Project, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, name, description, icon, color, system_prompt, source_scope_json, archived, created_at, updated_at
             FROM projects WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(Project {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    icon: row.get(3)?,
                    color: row.get(4)?,
                    system_prompt: row.get(5)?,
                    source_scope: parse_source_scope(row.get(6)?),
                    archived: row.get::<_, i32>(7)? != 0,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Project {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    /// Update a project. Only non-None fields are updated.
    pub fn update_project(
        &self,
        id: &str,
        input: &UpdateProjectInput,
    ) -> Result<Project, CoreError> {
        // Verify existence first.
        let _ = self.get_project(id)?;

        let conn = self.conn();
        if let Some(name) = &input.name {
            conn.execute(
                "UPDATE projects SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![name, id],
            )?;
        }
        if let Some(description) = &input.description {
            conn.execute(
                "UPDATE projects SET description = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![description, id],
            )?;
        }
        if let Some(icon) = &input.icon {
            conn.execute(
                "UPDATE projects SET icon = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![icon, id],
            )?;
        }
        if let Some(color) = &input.color {
            conn.execute(
                "UPDATE projects SET color = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![color, id],
            )?;
        }
        if let Some(system_prompt) = &input.system_prompt {
            conn.execute(
                "UPDATE projects SET system_prompt = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![system_prompt, id],
            )?;
        }
        if let Some(source_scope) = &input.source_scope {
            let json = serde_json::to_string(source_scope)?;
            conn.execute(
                "UPDATE projects SET source_scope_json = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![json, id],
            )?;
        }
        if let Some(archived) = input.archived {
            let val: i32 = if archived { 1 } else { 0 };
            conn.execute(
                "UPDATE projects SET archived = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![val, id],
            )?;
        }
        drop(conn);
        self.get_project(id)
    }

    /// Delete a project. Conversations' project_id is set to NULL via ON DELETE SET NULL.
    pub fn delete_project(&self, id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id])?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!("Project {id}")));
        }
        Ok(())
    }

    /// Move a conversation into a project.
    pub fn move_conversation_to_project(
        &self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<(), CoreError> {
        // Verify both exist.
        let _ = self.get_conversation(conversation_id)?;
        let _ = self.get_project(project_id)?;
        let conn = self.conn();
        conn.execute(
            "UPDATE conversations SET project_id = ?1, updated_at = datetime('now') WHERE id = ?2",
            rusqlite::params![project_id, conversation_id],
        )?;
        Ok(())
    }

    /// Remove a conversation from its project (set project_id to NULL).
    pub fn remove_conversation_from_project(&self, conversation_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        let affected = conn.execute(
            "UPDATE conversations SET project_id = NULL, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![conversation_id],
        )?;
        if affected == 0 {
            return Err(CoreError::NotFound(format!(
                "Conversation {conversation_id}"
            )));
        }
        Ok(())
    }

    /// List conversations filtered by project_id. If project_id is None, returns all.
    pub fn list_conversations_by_project(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<crate::conversation::Conversation>, CoreError> {
        match project_id {
            Some(pid) => {
                let conn = self.conn();
                let mut stmt = conn.prepare(
                    "SELECT id, title, provider, model, system_prompt, collection_context_json, project_id, title_is_auto, created_at, updated_at
                     FROM conversations WHERE project_id = ?1 ORDER BY updated_at DESC",
                )?;
                let rows = stmt.query_map(rusqlite::params![pid], |row| {
                    Ok(crate::conversation::Conversation {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        provider: row.get(2)?,
                        model: row.get(3)?,
                        system_prompt: row.get(4)?,
                        collection_context: crate::conversation::parse_collection_context(
                            row.get(5)?,
                        ),
                        project_id: row.get(6)?,
                        title_is_auto: row.get::<_, i64>(7)? != 0,
                        created_at: row.get(8)?,
                        updated_at: row.get(9)?,
                    })
                })?;
                let mut results = Vec::new();
                for row in rows {
                    results.push(row?);
                }
                Ok(results)
            }
            None => self.list_conversations(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_crud() {
        let db = Database::open_memory().unwrap();

        // Create
        let input = CreateProjectInput {
            name: "Test Project".into(),
            description: Some("A test".into()),
            icon: None,
            color: None,
            system_prompt: Some("You are helpful.".into()),
            source_scope: None,
        };
        let project = db.create_project(&input).unwrap();
        assert_eq!(project.name, "Test Project");
        assert_eq!(project.system_prompt, "You are helpful.");

        // List
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 1);

        // Update
        let update = UpdateProjectInput {
            name: Some("Renamed".into()),
            description: None,
            icon: None,
            color: None,
            system_prompt: None,
            source_scope: None,
            archived: None,
        };
        let updated = db.update_project(&project.id, &update).unwrap();
        assert_eq!(updated.name, "Renamed");

        // Delete
        db.delete_project(&project.id).unwrap();
        let projects = db.list_projects().unwrap();
        assert_eq!(projects.len(), 0);
    }

    #[test]
    fn test_move_conversation_to_project() {
        let db = Database::open_memory().unwrap();

        let project = db
            .create_project(&CreateProjectInput {
                name: "P1".into(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .unwrap();

        let conv = db
            .create_conversation(&crate::conversation::CreateConversationInput {
                provider: "test".into(),
                model: "gpt-4".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
            })
            .unwrap();

        // Move to project
        db.move_conversation_to_project(&conv.id, &project.id)
            .unwrap();
        let updated = db.get_conversation(&conv.id).unwrap();
        assert_eq!(updated.project_id.as_deref(), Some(project.id.as_str()));

        // List by project
        let filtered = db.list_conversations_by_project(Some(&project.id)).unwrap();
        assert_eq!(filtered.len(), 1);

        // Remove from project
        db.remove_conversation_from_project(&conv.id).unwrap();
        let updated = db.get_conversation(&conv.id).unwrap();
        assert!(updated.project_id.is_none());
    }

    #[test]
    fn test_delete_project_sets_null() {
        let db = Database::open_memory().unwrap();

        let project = db
            .create_project(&CreateProjectInput {
                name: "P1".into(),
                description: None,
                icon: None,
                color: None,
                system_prompt: None,
                source_scope: None,
            })
            .unwrap();

        let conv = db
            .create_conversation(&crate::conversation::CreateConversationInput {
                provider: "test".into(),
                model: "gpt-4".into(),
                system_prompt: None,
                collection_context: None,
                project_id: Some(project.id.clone()),
            })
            .unwrap();
        assert_eq!(conv.project_id.as_deref(), Some(project.id.as_str()));

        // Delete project → conversation's project_id should become NULL
        db.delete_project(&project.id).unwrap();
        let conv_after = db.get_conversation(&conv.id).unwrap();
        assert!(conv_after.project_id.is_none());
    }
}
