//! Source management — CRUD operations for content sources.

use std::path::Path;

use globset::Glob;
use rusqlite::params;

use crate::db::Database;
use crate::error::CoreError;
use crate::models::Source;

/// Input for creating a new source.
pub struct CreateSourceInput {
    pub root_path: String,
    pub include_globs: Vec<String>,
    pub exclude_globs: Vec<String>,
    pub watch_enabled: bool,
}

/// Input for updating an existing source.
pub struct UpdateSourceInput {
    pub include_globs: Option<Vec<String>>,
    pub exclude_globs: Option<Vec<String>>,
    pub watch_enabled: Option<bool>,
}

/// Map a rusqlite row to a `Source`.
fn source_from_row(row: &rusqlite::Row) -> Result<Source, rusqlite::Error> {
    let include_json: String = row.get(3)?;
    let exclude_json: String = row.get(4)?;
    let watch_int: i32 = row.get(5)?;

    let include_globs: Vec<String> = serde_json::from_str(&include_json).unwrap_or_default();
    let exclude_globs: Vec<String> = serde_json::from_str(&exclude_json).unwrap_or_default();

    Ok(Source {
        id: row.get(0)?,
        kind: row.get(1)?,
        root_path: row.get(2)?,
        include_globs,
        exclude_globs,
        watch_enabled: watch_int != 0,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

/// Validate that every pattern in `globs` is a legal glob.
fn validate_globs(globs: &[String]) -> Result<(), CoreError> {
    for pattern in globs {
        Glob::new(pattern).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid glob pattern '{pattern}': {e}"))
        })?;
    }
    Ok(())
}

impl Database {
    /// Add a new source folder.
    ///
    /// Validates that the path exists, is a directory, is not already
    /// registered, and that all glob patterns are valid.
    pub fn add_source(&self, input: CreateSourceInput) -> Result<Source, CoreError> {
        let path = Path::new(&input.root_path);
        if !path.exists() {
            return Err(CoreError::InvalidInput(format!(
                "Path does not exist: {}",
                input.root_path
            )));
        }
        if !path.is_dir() {
            return Err(CoreError::InvalidInput(format!(
                "Path is not a directory: {}",
                input.root_path
            )));
        }

        validate_globs(&input.include_globs)?;
        validate_globs(&input.exclude_globs)?;

        if self.source_exists_for_path(&input.root_path)? {
            return Err(CoreError::InvalidInput(format!(
                "Source already registered for path: {}",
                input.root_path
            )));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let include_json = serde_json::to_string(&input.include_globs)?;
        let exclude_json = serde_json::to_string(&input.exclude_globs)?;
        let watch_int: i32 = if input.watch_enabled { 1 } else { 0 };

        let conn = self.conn();
        conn.execute(
            "INSERT INTO sources (id, kind, root_path, include_globs, exclude_globs, watch_enabled)
             VALUES (?1, 'local_folder', ?2, ?3, ?4, ?5)",
            params![
                &id,
                &input.root_path,
                &include_json,
                &exclude_json,
                watch_int
            ],
        )?;
        drop(conn);

        self.get_source(&id)
    }

    /// List all sources.
    pub fn list_sources(&self) -> Result<Vec<Source>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, kind, root_path, include_globs, exclude_globs, watch_enabled, created_at, updated_at
             FROM sources ORDER BY created_at",
        )?;
        let rows = stmt
            .query_map([], source_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Get a single source by ID.
    pub fn get_source(&self, id: &str) -> Result<Source, CoreError> {
        let conn = self.conn();
        conn.query_row(
            "SELECT id, kind, root_path, include_globs, exclude_globs, watch_enabled, created_at, updated_at
             FROM sources WHERE id = ?1",
            params![id],
            source_from_row,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::NotFound(format!("Source not found: {id}"))
            }
            other => CoreError::Database(other),
        })
    }

    /// Update source configuration.
    pub fn update_source(&self, id: &str, input: UpdateSourceInput) -> Result<Source, CoreError> {
        // Ensure the source exists first.
        let existing = self.get_source(id)?;

        let include_globs = input.include_globs.unwrap_or(existing.include_globs);
        let exclude_globs = input.exclude_globs.unwrap_or(existing.exclude_globs);
        let watch_enabled = input.watch_enabled.unwrap_or(existing.watch_enabled);

        validate_globs(&include_globs)?;
        validate_globs(&exclude_globs)?;

        let include_json = serde_json::to_string(&include_globs)?;
        let exclude_json = serde_json::to_string(&exclude_globs)?;
        let watch_int: i32 = if watch_enabled { 1 } else { 0 };

        let conn = self.conn();
        conn.execute(
            "UPDATE sources
             SET include_globs = ?1, exclude_globs = ?2, watch_enabled = ?3, updated_at = datetime('now')
             WHERE id = ?4",
            params![&include_json, &exclude_json, watch_int, id],
        )?;

        drop(conn);
        self.get_source(id)
    }

    /// Delete a source and all its documents/chunks (cascade handled by FK).
    pub fn delete_source(&self, id: &str) -> Result<(), CoreError> {
        // Verify source exists.
        let _ = self.get_source(id)?;

        let conn = self.conn();
        conn.execute("DELETE FROM sources WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Check if a path is already registered as a source.
    pub fn source_exists_for_path(&self, path: &str) -> Result<bool, CoreError> {
        let conn = self.conn();
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sources WHERE root_path = ?1)",
            params![path],
            |row| row.get(0),
        )?;
        Ok(exists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("failed to create temp dir")
    }

    fn test_db() -> Database {
        Database::open_memory().expect("failed to open in-memory db")
    }

    #[test]
    fn test_add_source_success() {
        let dir = create_test_dir();
        let db = test_db();
        let input = CreateSourceInput {
            root_path: dir.path().to_string_lossy().to_string(),
            include_globs: vec!["**/*.md".to_string()],
            exclude_globs: vec!["**/node_modules/**".to_string()],
            watch_enabled: true,
        };

        let source = db.add_source(input).expect("add_source should succeed");
        assert!(!source.id.is_empty());
        assert_eq!(source.kind, "local_folder");
        assert_eq!(source.root_path, dir.path().to_string_lossy().to_string());
        assert_eq!(source.include_globs, vec!["**/*.md"]);
        assert_eq!(source.exclude_globs, vec!["**/node_modules/**"]);
        assert!(source.watch_enabled);
    }

    #[test]
    fn test_add_source_invalid_path() {
        let db = test_db();
        let input = CreateSourceInput {
            root_path: "/nonexistent/path/that/does/not/exist".to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: true,
        };

        let err = db.add_source(input).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidInput(_)),
            "Expected InvalidInput, got: {err:?}"
        );
    }

    #[test]
    fn test_add_source_not_a_directory() {
        let dir = create_test_dir();
        let file_path = dir.path().join("file.txt");
        std::fs::write(&file_path, "hello").expect("create file");
        let db = test_db();
        let input = CreateSourceInput {
            root_path: file_path.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: true,
        };

        let err = db.add_source(input).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidInput(_)),
            "Expected InvalidInput, got: {err:?}"
        );
    }

    #[test]
    fn test_add_source_duplicate_path() {
        let dir = create_test_dir();
        let db = test_db();
        let make_input = || CreateSourceInput {
            root_path: dir.path().to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: true,
        };

        db.add_source(make_input())
            .expect("first add should succeed");
        let err = db.add_source(make_input()).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidInput(_)),
            "Expected InvalidInput for duplicate, got: {err:?}"
        );
    }

    #[test]
    fn test_add_source_invalid_glob() {
        let dir = create_test_dir();
        let db = test_db();
        let input = CreateSourceInput {
            root_path: dir.path().to_string_lossy().to_string(),
            include_globs: vec!["[invalid".to_string()],
            exclude_globs: vec![],
            watch_enabled: true,
        };

        let err = db.add_source(input).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidInput(_)),
            "Expected InvalidInput for bad glob, got: {err:?}"
        );
    }

    #[test]
    fn test_list_sources_empty() {
        let db = test_db();
        let sources = db.list_sources().expect("list_sources should succeed");
        assert!(sources.is_empty());
    }

    #[test]
    fn test_list_sources_multiple() {
        let dir1 = create_test_dir();
        let dir2 = create_test_dir();
        let db = test_db();

        db.add_source(CreateSourceInput {
            root_path: dir1.path().to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: true,
        })
        .unwrap();

        db.add_source(CreateSourceInput {
            root_path: dir2.path().to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .unwrap();

        let sources = db.list_sources().unwrap();
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn test_get_source() {
        let dir = create_test_dir();
        let db = test_db();
        let created = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec!["*.md".to_string()],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .unwrap();

        let fetched = db.get_source(&created.id).unwrap();
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.root_path, created.root_path);
        assert_eq!(fetched.include_globs, vec!["*.md"]);
    }

    #[test]
    fn test_get_source_not_found() {
        let db = test_db();
        let err = db.get_source("nonexistent-id").unwrap_err();
        assert!(
            matches!(err, CoreError::NotFound(_)),
            "Expected NotFound, got: {err:?}"
        );
    }

    #[test]
    fn test_update_source() {
        let dir = create_test_dir();
        let db = test_db();
        let created = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec!["*.md".to_string()],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .unwrap();

        let updated = db
            .update_source(
                &created.id,
                UpdateSourceInput {
                    include_globs: Some(vec!["*.txt".to_string(), "*.log".to_string()]),
                    exclude_globs: None,
                    watch_enabled: Some(false),
                },
            )
            .unwrap();

        assert_eq!(updated.id, created.id);
        assert_eq!(updated.include_globs, vec!["*.txt", "*.log"]);
        assert!(!updated.watch_enabled);
        // exclude_globs unchanged
        assert!(updated.exclude_globs.is_empty());
    }

    #[test]
    fn test_delete_source() {
        let dir = create_test_dir();
        let db = test_db();
        let created = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec![],
                exclude_globs: vec![],
                watch_enabled: true,
            })
            .unwrap();

        db.delete_source(&created.id)
            .expect("delete should succeed");

        let err = db.get_source(&created.id).unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[test]
    fn test_source_exists_for_path() {
        let dir = create_test_dir();
        let db = test_db();
        let path_str = dir.path().to_string_lossy().to_string();

        assert!(!db.source_exists_for_path(&path_str).unwrap());

        db.add_source(CreateSourceInput {
            root_path: path_str.clone(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: true,
        })
        .unwrap();

        assert!(db.source_exists_for_path(&path_str).unwrap());
    }
}
