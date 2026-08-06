use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

use super::{VisionObservationV1, VISION_OBSERVATION_SCHEMA_VERSION};

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
        let conn = self.conn();
        match profile_hash {
            Some(profile_hash) => Ok(conn.execute(
                "DELETE FROM vision_observation_cache
                 WHERE attachment_hash = ?1 AND profile_hash = ?2",
                params![attachment_hash, profile_hash],
            )?),
            None => Ok(conn.execute(
                "DELETE FROM vision_observation_cache WHERE attachment_hash = ?1",
                [attachment_hash],
            )?),
        }
    }

    pub fn clear_vision_observation_cache(&self) -> Result<usize, CoreError> {
        Ok(self
            .conn()
            .execute("DELETE FROM vision_observation_cache", [])?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision_router::{
        VisionConfidenceKind, VisionIntent, VisionObservationSource, VisionObservationSourceKind,
        VisionPrivacyScope, VisionRoutePlan, VisionRouteTrace, VISION_CLASSIFIER_VERSION,
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
}
