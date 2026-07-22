//! Durable, conversation-scoped execution goals.

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationGoalStatus {
    Active,
    Blocked,
    Complete,
}

impl ConversationGoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Blocked => "blocked",
            Self::Complete => "complete",
        }
    }

    fn parse(value: &str) -> Result<Self, CoreError> {
        match value {
            "active" => Ok(Self::Active),
            "blocked" => Ok(Self::Blocked),
            "complete" => Ok(Self::Complete),
            other => Err(CoreError::InvalidInput(format!(
                "Unknown conversation goal status: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationGoal {
    pub conversation_id: String,
    pub id: String,
    pub objective: String,
    pub status: ConversationGoalStatus,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

fn read_goal(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationGoal> {
    let raw_status: String = row.get(3)?;
    let status = ConversationGoalStatus::parse(&raw_status).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(ConversationGoal {
        conversation_id: row.get(0)?,
        id: row.get(1)?,
        objective: row.get(2)?,
        status,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        completed_at: row.get(6)?,
    })
}

impl Database {
    pub fn set_conversation_goal(
        &self,
        conversation_id: &str,
        objective: &str,
    ) -> Result<ConversationGoal, CoreError> {
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(CoreError::InvalidInput(
                "Conversation goal objective cannot be empty".into(),
            ));
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.conn().execute(
            "INSERT INTO conversation_goals (
                conversation_id, id, objective, status, created_at, updated_at, completed_at
             ) VALUES (?1, ?2, ?3, 'active', ?4, ?4, NULL)
             ON CONFLICT(conversation_id) DO UPDATE SET
                id = excluded.id,
                objective = excluded.objective,
                status = 'active',
                created_at = excluded.created_at,
                updated_at = excluded.updated_at,
                completed_at = NULL",
            params![conversation_id, id, objective, now],
        )?;
        self.get_conversation_goal(conversation_id)?
            .ok_or_else(|| CoreError::Internal("Conversation goal was not persisted".into()))
    }

    pub fn get_conversation_goal(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationGoal>, CoreError> {
        self.conn()
            .query_row(
                "SELECT conversation_id, id, objective, status, created_at, updated_at, completed_at
                 FROM conversation_goals WHERE conversation_id = ?1",
                [conversation_id],
                read_goal,
            )
            .optional()
            .map_err(CoreError::from)
    }

    pub fn update_conversation_goal(
        &self,
        conversation_id: &str,
        status: ConversationGoalStatus,
        objective: Option<&str>,
    ) -> Result<ConversationGoal, CoreError> {
        let current = self
            .get_conversation_goal(conversation_id)?
            .ok_or_else(|| {
                CoreError::InvalidInput("This conversation does not have a goal".into())
            })?;
        let objective = objective.map(str::trim).filter(|value| !value.is_empty());
        let next_objective = objective.unwrap_or(&current.objective);
        let now = Utc::now().to_rfc3339();
        let completed_at = (status == ConversationGoalStatus::Complete).then_some(now.as_str());
        self.conn().execute(
            "UPDATE conversation_goals
             SET objective = ?2, status = ?3, updated_at = ?4, completed_at = ?5
             WHERE conversation_id = ?1",
            params![
                conversation_id,
                next_objective,
                status.as_str(),
                now,
                completed_at,
            ],
        )?;
        self.get_conversation_goal(conversation_id)?.ok_or_else(|| {
            CoreError::Internal("Conversation goal disappeared during update".into())
        })
    }

    pub fn clear_conversation_goal(&self, conversation_id: &str) -> Result<(), CoreError> {
        self.conn().execute(
            "DELETE FROM conversation_goals WHERE conversation_id = ?1",
            [conversation_id],
        )?;
        Ok(())
    }
}

pub fn build_conversation_goal_prompt_section(
    db: &Database,
    conversation_id: &str,
    autonomous_execution: bool,
) -> String {
    let Ok(Some(goal)) = db.get_conversation_goal(conversation_id) else {
        return String::new();
    };
    if goal.status == ConversationGoalStatus::Complete {
        return String::new();
    }

    if !autonomous_execution {
        return format!(
            "## Active Goal\nObjective: {}\nStatus: {}\n\nThis goal is durable conversation context. Plan Mode may analyze it, but must not execute work or change its lifecycle state.",
            goal.objective,
            goal.status.as_str(),
        );
    }

    format!(
        "## Active Goal\nObjective: {}\nStatus: {}\n\nThis is a durable execution goal, not a one-turn prompt. Continue taking concrete actions until the objective is actually achieved and verified. Do not stop after merely restating the goal, writing a plan, or reporting partial progress. Call `update_goal` with `complete` only after the objective is achieved; call it with `blocked` only when progress genuinely requires user input or an external state change. Otherwise keep the goal active and continue working.",
        goal.objective,
        goal.status.as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::CreateConversationInput;

    fn conversation_id(db: &Database) -> String {
        db.create_conversation(&CreateConversationInput {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .unwrap()
        .id
    }

    #[test]
    fn goal_lifecycle_is_scoped_and_durable() {
        let db = Database::open_memory().unwrap();
        let first = conversation_id(&db);
        let second = conversation_id(&db);

        let created = db
            .set_conversation_goal(&first, "Ship the feature")
            .unwrap();
        assert_eq!(created.status, ConversationGoalStatus::Active);
        assert_eq!(created.objective, "Ship the feature");
        assert!(db.get_conversation_goal(&second).unwrap().is_none());

        let blocked = db
            .update_conversation_goal(&first, ConversationGoalStatus::Blocked, None)
            .unwrap();
        assert_eq!(blocked.status, ConversationGoalStatus::Blocked);
        assert!(blocked.completed_at.is_none());

        let completed = db
            .update_conversation_goal(
                &first,
                ConversationGoalStatus::Complete,
                Some("Ship and verify the feature"),
            )
            .unwrap();
        assert_eq!(completed.status, ConversationGoalStatus::Complete);
        assert!(completed.completed_at.is_some());

        db.clear_conversation_goal(&first).unwrap();
        assert!(db.get_conversation_goal(&first).unwrap().is_none());
    }

    #[test]
    fn active_goal_prompt_requires_verified_completion() {
        let db = Database::open_memory().unwrap();
        let conversation_id = conversation_id(&db);
        db.set_conversation_goal(&conversation_id, "Finish the work")
            .unwrap();

        let prompt = build_conversation_goal_prompt_section(&db, &conversation_id, true);
        assert!(prompt.contains("durable execution goal"));
        assert!(prompt.contains("update_goal"));
        assert!(prompt.contains("actually achieved and verified"));
    }
}
