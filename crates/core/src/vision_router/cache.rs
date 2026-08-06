use std::collections::HashSet;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::conversation::{
    invalidate_context_projection, ImageAttachment, LLM_CONTEXT_CONTENT_ARTIFACT_KEY,
};
use crate::db::Database;
use crate::error::CoreError;

use super::{observation_prompt_text, VisionObservationV1, VISION_OBSERVATION_SCHEMA_VERSION};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionObservationCacheEntry {
    pub observation: VisionObservationV1,
    pub created_at_epoch: i64,
    pub expires_at_epoch: i64,
    pub last_accessed_at_epoch: i64,
}

impl Database {
    pub fn get_vision_observation_cache(
        &self,
        attachment_hash: &str,
        profile_hash: &str,
        now_epoch: i64,
    ) -> Result<Option<VisionObservationCacheEntry>, CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM vision_observation_cache WHERE expires_at_epoch <= ?1",
            [now_epoch],
        )?;
        let row: Option<(String, i64, i64, i64)> = conn
            .query_row(
                "SELECT observation_json, created_at_epoch, expires_at_epoch,
                        last_accessed_at_epoch
                 FROM vision_observation_cache
                 WHERE attachment_hash = ?1 AND profile_hash = ?2",
                params![attachment_hash, profile_hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((observation_json, created_at_epoch, expires_at_epoch, last_accessed_at_epoch)) =
            row
        else {
            return Ok(None);
        };
        if expires_at_epoch <= now_epoch {
            conn.execute(
                "DELETE FROM vision_observation_cache
                 WHERE attachment_hash = ?1 AND profile_hash = ?2",
                params![attachment_hash, profile_hash],
            )?;
            return Ok(None);
        }
        let observation = serde_json::from_str::<VisionObservationV1>(&observation_json)
            .ok()
            .filter(|observation| {
                observation.validate().is_ok()
                    && observation.attachment_hash == attachment_hash
                    && observation.profile_hash == profile_hash
            });
        let Some(observation) = observation else {
            conn.execute(
                "DELETE FROM vision_observation_cache
                 WHERE attachment_hash = ?1 AND profile_hash = ?2",
                params![attachment_hash, profile_hash],
            )?;
            return Ok(None);
        };
        conn.execute(
            "UPDATE vision_observation_cache
             SET last_accessed_at_epoch = ?3
             WHERE attachment_hash = ?1 AND profile_hash = ?2",
            params![attachment_hash, profile_hash, now_epoch],
        )?;
        Ok(Some(VisionObservationCacheEntry {
            observation,
            created_at_epoch,
            expires_at_epoch,
            last_accessed_at_epoch: now_epoch.max(last_accessed_at_epoch),
        }))
    }

    pub fn save_vision_observation_cache(
        &self,
        observation: &VisionObservationV1,
        created_at_epoch: i64,
        expires_at_epoch: i64,
    ) -> Result<VisionObservationCacheEntry, CoreError> {
        observation.validate()?;
        if expires_at_epoch <= created_at_epoch {
            return Err(CoreError::InvalidInput(
                "Vision observation cache expiry must follow creation".to_string(),
            ));
        }
        let observation_json = serde_json::to_string(observation)?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO vision_observation_cache (
                 attachment_hash, profile_hash, schema_version, observation_json,
                 created_at_epoch, expires_at_epoch, last_accessed_at_epoch
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?5)
             ON CONFLICT(attachment_hash, profile_hash) DO UPDATE SET
                 schema_version = excluded.schema_version,
                 observation_json = excluded.observation_json,
                 created_at_epoch = excluded.created_at_epoch,
                 expires_at_epoch = excluded.expires_at_epoch,
                 last_accessed_at_epoch = excluded.last_accessed_at_epoch",
            params![
                &observation.attachment_hash,
                &observation.profile_hash,
                VISION_OBSERVATION_SCHEMA_VERSION,
                observation_json,
                created_at_epoch,
                expires_at_epoch,
            ],
        )?;
        Ok(VisionObservationCacheEntry {
            observation: observation.clone(),
            created_at_epoch,
            expires_at_epoch,
            last_accessed_at_epoch: created_at_epoch,
        })
    }

    pub fn delete_vision_observation_cache(
        &self,
        attachment_hash: &str,
        profile_hash: Option<&str>,
    ) -> Result<usize, CoreError> {
        let mut conn = self.conn();
        let transaction = conn.transaction()?;
        let removed = match profile_hash {
            Some(profile_hash) => transaction.execute(
                "DELETE FROM vision_observation_cache
                 WHERE attachment_hash = ?1 AND profile_hash = ?2",
                params![attachment_hash, profile_hash],
            )?,
            None => transaction.execute(
                "DELETE FROM vision_observation_cache WHERE attachment_hash = ?1",
                [attachment_hash],
            )?,
        };
        clear_persisted_observation_references(&transaction, Some(attachment_hash), profile_hash)?;
        transaction.commit()?;
        Ok(removed)
    }

    pub fn clear_vision_observation_cache(&self) -> Result<usize, CoreError> {
        let mut conn = self.conn();
        let transaction = conn.transaction()?;
        let removed = transaction.execute("DELETE FROM vision_observation_cache", [])?;
        clear_persisted_observation_references(&transaction, None, None)?;
        transaction.commit()?;
        Ok(removed)
    }

    pub fn purge_expired_vision_observation_cache(
        &self,
        now_epoch: i64,
    ) -> Result<usize, CoreError> {
        Ok(self.conn().execute(
            "DELETE FROM vision_observation_cache WHERE expires_at_epoch <= ?1",
            [now_epoch],
        )?)
    }
}

fn clear_persisted_observation_references(
    conn: &rusqlite::Connection,
    attachment_hash: Option<&str>,
    profile_hash: Option<&str>,
) -> Result<(), CoreError> {
    let mut statement = conn.prepare(
        "SELECT id, conversation_id, content, artifacts_json, image_attachments_json
         FROM messages WHERE image_attachments_json IS NOT NULL",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let mut invalidated = HashSet::new();
    for (message_id, conversation_id, content, artifacts_json, attachments_json) in rows {
        let mut attachments = match serde_json::from_str::<Vec<ImageAttachment>>(&attachments_json)
        {
            Ok(attachments) => attachments,
            Err(_) => continue,
        };
        let mut removed_prompts = Vec::new();
        let mut changed = false;
        for attachment in &mut attachments {
            let matches_hash = attachment_hash.is_none_or(|expected| {
                attachment
                    .attachment_hash
                    .as_deref()
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
            });
            let matches_profile = profile_hash.is_none_or(|expected| {
                attachment
                    .vision_analysis
                    .as_ref()
                    .and_then(|analysis| analysis.profile_hash.as_deref())
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
            });
            if !matches_hash || !matches_profile {
                continue;
            }
            if let Some(observation) = attachment
                .vision_analysis
                .as_ref()
                .and_then(|analysis| analysis.observation.as_ref())
            {
                if let Ok(prompt) = observation_prompt_text(&attachment.original_name, observation)
                {
                    removed_prompts.push(prompt);
                }
            }
            if attachment.vision_analysis.take().is_some() {
                changed = true;
            }
        }
        if !changed {
            continue;
        }

        let mut artifacts = artifacts_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(map) = artifacts.as_object_mut() {
            let mut llm_context = map
                .get(LLM_CONTEXT_CONTENT_ARTIFACT_KEY)
                .and_then(serde_json::Value::as_str)
                .unwrap_or(&content)
                .to_string();
            for prompt in removed_prompts {
                llm_context = llm_context.replace(&prompt, "");
            }
            let llm_context = llm_context
                .split("\n\n")
                .filter(|fragment| !fragment.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            map.insert(
                LLM_CONTEXT_CONTENT_ARTIFACT_KEY.to_string(),
                serde_json::Value::String(if llm_context.trim().is_empty() {
                    content.clone()
                } else {
                    llm_context
                }),
            );
        }
        conn.execute(
            "UPDATE messages SET artifacts_json = ?2, image_attachments_json = ?3 WHERE id = ?1",
            params![
                message_id,
                serde_json::to_string(&artifacts)?,
                serde_json::to_string(&attachments)?,
            ],
        )?;
        invalidated.insert(conversation_id);
    }
    for conversation_id in invalidated {
        invalidate_context_projection(conn, &conversation_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::{ConversationMessage, CreateConversationInput};
    use crate::llm::Role;
    use crate::vision_router::{
        VisionAttachmentAnalysis, VisionAttachmentStatus, VisionConfidenceKind, VisionIntent,
        VisionObservationSource, VisionObservationSourceKind, VisionPrivacyScope, VisionRoutePlan,
        VisionRouteTrace, VISION_CLASSIFIER_VERSION,
    };

    fn observation() -> VisionObservationV1 {
        VisionObservationV1 {
            schema_version: VISION_OBSERVATION_SCHEMA_VERSION,
            attachment_id: "attachment-1".to_string(),
            attachment_hash: "a".repeat(64),
            profile_hash: "b".repeat(64),
            intent: VisionIntent::DenseText,
            summary: None,
            ocr_text: Some("hello".to_string()),
            regions: Vec::new(),
            tables: Vec::new(),
            entities: Vec::new(),
            chart_data: Vec::new(),
            confidence: Some(0.8),
            confidence_kind: Some(VisionConfidenceKind::OcrRecognitionMean),
            sources: vec![VisionObservationSource {
                kind: VisionObservationSourceKind::LocalOcr,
                provider_id: None,
                model_id: None,
                target_id: None,
                target_revision: None,
                fallback_index: None,
                local: true,
            }],
            fallback_used: false,
            fallback_reason: None,
            privacy_scope: VisionPrivacyScope::Local,
            route: VisionRouteTrace {
                classifier_version: VISION_CLASSIFIER_VERSION,
                intent: VisionIntent::DenseText,
                plan: VisionRoutePlan::OcrOnly,
                classification_confidence: 1.0,
                reason_codes: Vec::new(),
                attempts: Vec::new(),
            },
        }
    }

    #[test]
    fn cache_expires_and_deletes_by_attachment() {
        let db = Database::open_memory().unwrap();
        let observation = observation();
        db.save_vision_observation_cache(&observation, 10, 20)
            .unwrap();
        assert!(db
            .get_vision_observation_cache(
                &observation.attachment_hash,
                &observation.profile_hash,
                19,
            )
            .unwrap()
            .is_some());
        assert!(db
            .get_vision_observation_cache(
                &observation.attachment_hash,
                &observation.profile_hash,
                20,
            )
            .unwrap()
            .is_none());
        assert_eq!(
            db.delete_vision_observation_cache(&observation.attachment_hash, None)
                .unwrap(),
            0
        );
    }

    #[test]
    fn cache_delete_also_removes_durable_message_observation_and_replay_text() {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "open_ai".to_string(),
                model: "gpt-test".to_string(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let observation = observation();
        let prompt = observation_prompt_text("scan.png", &observation).unwrap();
        db.add_message(&ConversationMessage {
            id: "message-vision".to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "read this".to_string(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: Some(serde_json::json!({
                (LLM_CONTEXT_CONTENT_ARTIFACT_KEY): format!("read this\n\n{prompt}"),
            })),
            token_count: 0,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: Some(vec![ImageAttachment {
                base64_data: "image".to_string(),
                media_type: "image/png".to_string(),
                original_name: "scan.png".to_string(),
                attachment_id: Some(observation.attachment_id.clone()),
                attachment_hash: Some(observation.attachment_hash.clone()),
                vision_analysis: Some(VisionAttachmentAnalysis {
                    status: VisionAttachmentStatus::Observed,
                    profile_hash: Some(observation.profile_hash.clone()),
                    observation: Some(observation.clone()),
                    reason_code: None,
                }),
            }]),
        })
        .unwrap();
        db.save_vision_observation_cache(&observation, 10, 20)
            .unwrap();

        assert_eq!(
            db.delete_vision_observation_cache(
                &observation.attachment_hash,
                Some(&observation.profile_hash),
            )
            .unwrap(),
            1
        );
        let message = db.get_messages(&conversation.id).unwrap().remove(0);
        let attachment = &message.image_attachments.unwrap()[0];
        assert!(attachment.vision_analysis.is_none());
        let llm_context = message
            .artifacts
            .as_ref()
            .and_then(|value| value.get(LLM_CONTEXT_CONTENT_ARTIFACT_KEY))
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert_eq!(llm_context, "read this");
    }
}
