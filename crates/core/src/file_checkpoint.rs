//! File mutation checkpoints for agent-managed disk writes.
//!
//! Conversation checkpoints archive chat context. File checkpoints snapshot
//! bytes immediately before a tool mutates a user file so the change can be
//! restored later.

use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;

#[derive(Debug, Clone)]
pub struct CreateFileCheckpointInput<'a> {
    pub conversation_id: Option<&'a str>,
    pub tool_call_id: &'a str,
    pub tool_name: &'a str,
    pub operation: &'a str,
    pub path: &'a str,
    pub absolute_path: &'a Path,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCheckpoint {
    pub id: String,
    pub conversation_id: Option<String>,
    pub tool_call_id: String,
    pub tool_name: String,
    pub operation: String,
    pub path: String,
    pub absolute_path: String,
    pub existed_before: bool,
    pub bytes_before: u64,
    pub hash_before: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCheckpointRestore {
    pub checkpoint: FileCheckpoint,
    pub action: String,
    pub bytes_written: u64,
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, CoreError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn validate_restore_target_in_sources(db: &Database, target: &Path) -> Result<(), CoreError> {
    let sources = db.list_sources()?;
    if sources.is_empty() {
        return Err(CoreError::InvalidInput(
            "Cannot restore file checkpoint because no source directories are registered."
                .to_string(),
        ));
    }

    let resolved = if target.exists() {
        std::fs::canonicalize(target)?
    } else {
        let parent = target.parent().ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid checkpoint path: {}", target.display()))
        })?;
        let canonical_parent = std::fs::canonicalize(parent)?;
        let file_name = target.file_name().ok_or_else(|| {
            CoreError::InvalidInput(format!("Invalid checkpoint path: {}", target.display()))
        })?;
        canonical_parent.join(file_name)
    };

    let in_scope = sources.iter().any(|source| {
        std::fs::canonicalize(Path::new(&source.root_path))
            .map(|root| resolved.starts_with(root))
            .unwrap_or(false)
    });
    if !in_scope {
        return Err(CoreError::InvalidInput(format!(
            "Cannot restore '{}': path is no longer inside a registered source directory.",
            target.display()
        )));
    }
    Ok(())
}

fn map_checkpoint_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileCheckpoint> {
    Ok(FileCheckpoint {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        tool_call_id: row.get(2)?,
        tool_name: row.get(3)?,
        operation: row.get(4)?,
        path: row.get(5)?,
        absolute_path: row.get(6)?,
        existed_before: row.get::<_, i64>(7)? != 0,
        bytes_before: row.get::<_, i64>(8)? as u64,
        hash_before: row.get(9)?,
        created_at: row.get(10)?,
    })
}

impl Database {
    pub fn create_file_checkpoint(
        &self,
        input: CreateFileCheckpointInput<'_>,
    ) -> Result<FileCheckpoint, CoreError> {
        let id = Uuid::new_v4().to_string();
        let absolute = normalize_absolute_path(input.absolute_path)?;

        if absolute.exists() && !absolute.is_file() {
            return Err(CoreError::InvalidInput(format!(
                "Cannot checkpoint non-file path: {}",
                absolute.display()
            )));
        }

        let (existed_before, content_before, bytes_before, hash_before) = if absolute.exists() {
            let bytes = std::fs::read(&absolute)?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            (true, Some(bytes.clone()), bytes.len() as u64, Some(hash))
        } else {
            (false, None, 0, None)
        };

        let conn = self.conn();
        conn.execute(
            "INSERT INTO file_checkpoints
             (id, conversation_id, tool_call_id, tool_name, operation, path, absolute_path,
              existed_before, content_before, bytes_before, hash_before)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                &id,
                input.conversation_id,
                input.tool_call_id,
                input.tool_name,
                input.operation,
                input.path,
                absolute.to_string_lossy().to_string(),
                if existed_before { 1_i64 } else { 0_i64 },
                &content_before,
                bytes_before as i64,
                &hash_before,
            ],
        )?;
        drop(conn);
        self.get_file_checkpoint(&id)
    }

    pub fn get_file_checkpoint(&self, checkpoint_id: &str) -> Result<FileCheckpoint, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, conversation_id, tool_call_id, tool_name, operation, path, absolute_path,
                    existed_before, bytes_before, hash_before, created_at
             FROM file_checkpoints
             WHERE id = ?1",
            rusqlite::params![checkpoint_id],
            map_checkpoint_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("File checkpoint {checkpoint_id}"))
            }
            other => CoreError::Database(other),
        })
    }

    pub fn list_file_checkpoints(
        &self,
        conversation_id: Option<&str>,
    ) -> Result<Vec<FileCheckpoint>, CoreError> {
        let conn = self.conn();
        let mut results = Vec::new();
        if let Some(conversation_id) = conversation_id {
            let mut stmt = conn.prepare(
                "SELECT id, conversation_id, tool_call_id, tool_name, operation, path, absolute_path,
                        existed_before, bytes_before, hash_before, created_at
                 FROM file_checkpoints
                 WHERE conversation_id = ?1
                 ORDER BY created_at DESC
                 LIMIT 100",
            )?;
            let rows = stmt.query_map(rusqlite::params![conversation_id], map_checkpoint_row)?;
            for row in rows {
                results.push(row?);
            }
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, conversation_id, tool_call_id, tool_name, operation, path, absolute_path,
                        existed_before, bytes_before, hash_before, created_at
                 FROM file_checkpoints
                 ORDER BY created_at DESC
                 LIMIT 100",
            )?;
            let rows = stmt.query_map([], map_checkpoint_row)?;
            for row in rows {
                results.push(row?);
            }
        }
        Ok(results)
    }

    pub fn restore_file_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<FileCheckpointRestore, CoreError> {
        let checkpoint = self.get_file_checkpoint(checkpoint_id)?;
        let target = PathBuf::from(&checkpoint.absolute_path);
        validate_restore_target_in_sources(self, &target)?;

        let conn = self.conn();
        let content_before: Option<Vec<u8>> = conn
            .query_row(
                "SELECT content_before FROM file_checkpoints WHERE id = ?1",
                rusqlite::params![checkpoint_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        drop(conn);

        if checkpoint.existed_before {
            let bytes = content_before.ok_or_else(|| {
                CoreError::Internal(format!(
                    "File checkpoint {checkpoint_id} is missing its stored bytes."
                ))
            })?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&target, &bytes)?;
            Ok(FileCheckpointRestore {
                checkpoint,
                action: "restored".to_string(),
                bytes_written: bytes.len() as u64,
            })
        } else {
            if target.exists() {
                if !target.is_file() {
                    return Err(CoreError::InvalidInput(format!(
                        "Cannot remove non-file path during checkpoint restore: {}",
                        target.display()
                    )));
                }
                std::fs::remove_file(&target)?;
            }
            Ok(FileCheckpointRestore {
                checkpoint,
                action: "deleted_created_file".to_string(),
                bytes_written: 0,
            })
        }
    }

    pub fn delete_file_checkpoint(&self, checkpoint_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM file_checkpoints WHERE id = ?1",
            rusqlite::params![checkpoint_id],
        )?;
        Ok(())
    }
}

pub fn checkpoint_artifact(
    checkpoint: &FileCheckpoint,
    bytes_after: Option<u64>,
) -> serde_json::Value {
    serde_json::json!({
        "kind": "fileCheckpoint",
        "checkpoint": checkpoint,
        "bytesAfter": bytes_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    fn db_with_source(root: &Path) -> Database {
        let db = Database::open_memory().expect("open memory db");
        db.add_source(CreateSourceInput {
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .expect("register source");
        db
    }

    #[test]
    fn restores_previous_bytes_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, b"before").unwrap();
        let db = db_with_source(dir.path());

        let checkpoint = db
            .create_file_checkpoint(CreateFileCheckpointInput {
                conversation_id: None,
                tool_call_id: "call-1",
                tool_name: "edit_file",
                operation: "str_replace",
                path: "notes.txt",
                absolute_path: &file,
            })
            .unwrap();

        std::fs::write(&file, b"after").unwrap();
        let restored = db.restore_file_checkpoint(&checkpoint.id).unwrap();

        assert_eq!(restored.action, "restored");
        assert_eq!(std::fs::read(&file).unwrap(), b"before");
        assert_eq!(restored.bytes_written, 6);
    }

    #[test]
    fn restoring_created_file_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new.md");
        let db = db_with_source(dir.path());

        let checkpoint = db
            .create_file_checkpoint(CreateFileCheckpointInput {
                conversation_id: None,
                tool_call_id: "call-1",
                tool_name: "create_file",
                operation: "create",
                path: "new.md",
                absolute_path: &file,
            })
            .unwrap();

        std::fs::write(&file, b"created").unwrap();
        let restored = db.restore_file_checkpoint(&checkpoint.id).unwrap();

        assert_eq!(restored.action, "deleted_created_file");
        assert!(!file.exists());
    }
}
