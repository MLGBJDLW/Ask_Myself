//! Ingestion module — discovers and imports files from sources.
//!
//! Walks a source's directory tree, applies include/exclude glob filters,
//! parses matching files, and upserts documents + chunks into the database.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use log::{debug, info, warn};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::embed::{create_embedder, Embedder, TfIdfEmbedder};
use crate::error::CoreError;
use crate::parse::{parse_file, ParsedChunk, ParsedDocument};
use crate::privacy::{self, PrivacyConfig};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Summary of an ingestion run for a single source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestResult {
    pub source_id: String,
    pub files_scanned: usize,
    pub files_added: usize,
    pub files_updated: usize,
    pub files_skipped: usize,
    pub files_failed: usize,
    pub errors: Vec<String>,
}

/// Summary of an embedding run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedResult {
    pub source_id: String,
    pub chunks_embedded: usize,
    pub chunks_skipped: usize,
    pub model: String,
}

/// Progress information emitted during scanning or embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub source_id: String,
    pub phase: String,
    pub current: usize,
    pub total: usize,
    pub current_file: Option<String>,
}

/// Result of ingesting a single file (for incremental watcher events).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestFileResult {
    /// File was new and inserted.
    Added,
    /// File content changed and was updated.
    Updated,
    /// File content unchanged — skipped.
    Unchanged,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan a source directory and ingest all matching files.
///
/// Walks the source's `root_path` recursively, applies include/exclude globs,
/// and for each matching file: parses it, checks for changes via content hash,
/// and inserts or updates the document and its chunks in the database.
pub fn scan_source(db: &Database, source_id: &str) -> Result<IngestResult, CoreError> {
    scan_source_inner(db, source_id, None, None)
}

/// Scan a source directory with an optional [`PrivacyConfig`].
///
/// When `privacy` is `Some`, its `exclude_patterns` are merged with the
/// source's own exclude globs and content redaction is applied to every
/// chunk before storage. When `None`, the stored config is loaded from the
/// database (falling back to defaults).
pub fn scan_source_with_privacy(
    db: &Database,
    source_id: &str,
    privacy: Option<&PrivacyConfig>,
) -> Result<IngestResult, CoreError> {
    scan_source_inner(db, source_id, privacy, None)
}

/// Scan a source directory with progress reporting.
///
/// Same as [`scan_source`] but calls `on_progress` at regular intervals
/// during the file-walk loop (approximately every 10 files).
pub fn scan_source_with_progress(
    db: &Database,
    source_id: &str,
    on_progress: impl Fn(ScanProgress),
) -> Result<IngestResult, CoreError> {
    scan_source_inner(db, source_id, None, Some(&on_progress))
}

/// Internal scan implementation shared by all public scan functions.
fn scan_source_inner(
    db: &Database,
    source_id: &str,
    privacy: Option<&PrivacyConfig>,
    on_progress: Option<&dyn Fn(ScanProgress)>,
) -> Result<IngestResult, CoreError> {
    let source = db.get_source(source_id)?;

    // Resolve privacy config: explicit > stored > default.
    let default_config;
    let privacy_cfg = match privacy {
        Some(cfg) => cfg,
        None => {
            default_config = db.load_privacy_config()?;
            &default_config
        }
    };

    let root = Path::new(&source.root_path);
    if !root.exists() {
        return Err(CoreError::InvalidInput(format!(
            "Source root path does not exist: {}",
            source.root_path
        )));
    }

    // Merge source excludes with privacy excludes.
    let mut all_excludes = source.exclude_globs.clone();
    all_excludes.extend(privacy_cfg.exclude_patterns.iter().cloned());

    let include_set = build_glob_set(&source.include_globs)?;
    let exclude_set = build_glob_set(&all_excludes)?;
    let has_includes = !source.include_globs.is_empty();

    let mut result = IngestResult {
        source_id: source_id.to_string(),
        files_scanned: 0,
        files_added: 0,
        files_updated: 0,
        files_skipped: 0,
        files_failed: 0,
        errors: Vec::new(),
    };

    // Collect all files recursively, sorted for deterministic order.
    let files = walk_directory(root)?;
    let total_files = files.len();

    // Emit initial progress.
    if let Some(cb) = &on_progress {
        cb(ScanProgress {
            source_id: source_id.to_string(),
            phase: "scanning".to_string(),
            current: 0,
            total: total_files,
            current_file: None,
        });
    }

    // Pre-fetch all existing document paths and hashes for this source
    // to avoid N individual database lookups during scanning.
    let existing_docs = db.get_document_paths_for_source(source_id)?;

    // Collect new and updated documents for batched database operations.
    let mut new_docs: Vec<ParsedDocument> = Vec::new();
    let mut update_docs: Vec<(String, ParsedDocument)> = Vec::new(); // (doc_id, parsed)

    // Progress throttle: emit every 10 files.
    let progress_interval: usize = 10;
    let mut files_processed: usize = 0;

    for file_path in &files {
        // Compute relative path for glob matching, normalised to forward slashes.
        let rel_str = file_path
            .strip_prefix(root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .replace('\\', "/");

        // Include filter: if globs are specified, file must match at least one.
        if has_includes && !include_set.is_match(&rel_str) {
            files_processed += 1;
            continue;
        }

        // Exclude filter: skip files matching any exclude pattern.
        if exclude_set.is_match(&rel_str) {
            files_processed += 1;
            continue;
        }

        result.files_scanned += 1;
        files_processed += 1;

        // Emit progress at regular intervals.
        if let Some(cb) = &on_progress {
            if files_processed % progress_interval == 0 || files_processed == total_files {
                cb(ScanProgress {
                    source_id: source_id.to_string(),
                    phase: "scanning".to_string(),
                    current: files_processed,
                    total: total_files,
                    current_file: Some(rel_str.clone()),
                });
            }
        }

        // Skip files exceeding the size limit to avoid excessive memory usage.
        const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB
        match std::fs::metadata(file_path) {
            Ok(meta) if meta.len() > MAX_FILE_SIZE => {
                warn!(
                    "Skipping large file ({}MB): {}",
                    meta.len() / 1024 / 1024,
                    file_path.display()
                );
                result.files_skipped += 1;
                continue;
            }
            _ => {} // proceed normally (missing metadata is handled by parse_file)
        }

        match classify_file(file_path, &existing_docs, privacy_cfg) {
            Ok(FileClassification::New(parsed)) => {
                new_docs.push(parsed);
                result.files_added += 1;
            }
            Ok(FileClassification::Changed(doc_id, parsed)) => {
                update_docs.push((doc_id, parsed));
                result.files_updated += 1;
            }
            Ok(FileClassification::Unchanged) => {
                result.files_skipped += 1;
            }
            Err(e) => {
                let msg = format!("{}: {}", file_path.display(), e);
                warn!("Failed to ingest file: {}", msg);
                result.errors.push(msg);
                result.files_failed += 1;
            }
        }
    }

    // Batch-insert all new documents in a single transaction.
    if !new_docs.is_empty() {
        debug!("Batch-inserting {} new documents", new_docs.len());
        batch_insert_documents(db, source_id, &new_docs)?;
    }

    // Batch-update all changed documents in a single transaction.
    if !update_docs.is_empty() {
        debug!("Batch-updating {} changed documents", update_docs.len());
        batch_update_documents(db, &update_docs)?;
    }

    // Emit final progress.
    if let Some(cb) = &on_progress {
        cb(ScanProgress {
            source_id: source_id.to_string(),
            phase: "scanning".to_string(),
            current: total_files,
            total: total_files,
            current_file: None,
        });
    }

    info!(
        "Scan complete for source {}: scanned={}, added={}, updated={}, skipped={}, failed={}",
        source_id,
        result.files_scanned,
        result.files_added,
        result.files_updated,
        result.files_skipped,
        result.files_failed
    );

    Ok(result)
}

/// Generate embeddings for all un-embedded chunks belonging to a source.
///
/// Reads the persisted [`EmbedderConfig`] to decide which embedder to use:
/// - `"tfidf"` — loads/builds TF-IDF from the corpus (original behaviour).
/// - `"local"` or `"api"` — delegates to [`create_embedder`].
pub fn embed_source(db: &Database, source_id: &str) -> Result<EmbedResult, CoreError> {
    embed_source_inner(db, source_id, None)
}

/// Generate embeddings with progress reporting.
///
/// Same as [`embed_source`] but calls `on_progress` after each batch of
/// chunks is embedded.
pub fn embed_source_with_progress(
    db: &Database,
    source_id: &str,
    on_progress: impl Fn(ScanProgress),
) -> Result<EmbedResult, CoreError> {
    embed_source_inner(db, source_id, Some(&on_progress))
}

/// Internal embed implementation shared by public embed functions.
fn embed_source_inner(
    db: &Database,
    source_id: &str,
    on_progress: Option<&dyn Fn(ScanProgress)>,
) -> Result<EmbedResult, CoreError> {
    let config = db.get_embedder_config()?;

    if config.provider == "tfidf" {
        return embed_source_tfidf(db, source_id, on_progress);
    }

    let embedder = create_embedder(&config)?;
    let model = embedder.model_name().to_string();

    let missing = db.get_chunks_without_embeddings(&model)?;
    let total_chunks = missing.len();

    // Emit initial progress.
    if let Some(cb) = &on_progress {
        cb(ScanProgress {
            source_id: source_id.to_string(),
            phase: "embedding".to_string(),
            current: 0,
            total: total_chunks,
            current_file: None,
        });
    }

    // Process embeddings in sub-batches for progress reporting.
    const EMBED_BATCH_SIZE: usize = 64;
    let mut all_batch: Vec<(String, String, Vec<f32>)> = Vec::with_capacity(total_chunks);
    let mut embedded_so_far: usize = 0;

    for chunk in missing.chunks(EMBED_BATCH_SIZE) {
        let texts: Vec<&str> = chunk.iter().map(|(_, c)| c.as_str()).collect();
        let vectors = embedder.embed_batch(&texts)?;
        for ((chunk_id, _), vector) in chunk.iter().zip(vectors.into_iter()) {
            all_batch.push((chunk_id.clone(), model.clone(), vector));
        }
        embedded_so_far += chunk.len();

        if let Some(cb) = &on_progress {
            cb(ScanProgress {
                source_id: source_id.to_string(),
                phase: "embedding".to_string(),
                current: embedded_so_far,
                total: total_chunks,
                current_file: None,
            });
        }
    }

    let embedded = all_batch.len();
    if !all_batch.is_empty() {
        db.batch_store_embeddings(&all_batch)?;
    }

    let total_source_chunks = db.get_chunks_for_source(source_id)?.len();
    let skipped = total_source_chunks.saturating_sub(embedded);

    info!(
        "Embedding complete for source {}: embedded={}, skipped={}, provider={}",
        source_id, embedded, skipped, config.provider
    );

    Ok(EmbedResult {
        source_id: source_id.to_string(),
        chunks_embedded: embedded,
        chunks_skipped: skipped,
        model,
    })
}

/// TF-IDF specific embedding path (original behaviour).
fn embed_source_tfidf(
    db: &Database,
    source_id: &str,
    on_progress: Option<&dyn Fn(ScanProgress)>,
) -> Result<EmbedResult, CoreError> {
    let model = "tfidf-v1";

    let embedder = match db.load_embedder_state(model)? {
        Some((vocab, idf)) => {
            info!("Loaded existing embedder state for model '{}'", model);
            TfIdfEmbedder::from_vocabulary(vocab, idf)
        }
        None => {
            info!("No saved embedder state — building from full corpus");
            let all_chunks = db.get_all_chunks()?;
            let corpus: Vec<&str> = all_chunks.iter().map(|(_, c)| c.as_str()).collect();
            let embedder = TfIdfEmbedder::build_from_corpus(&corpus);
            db.save_embedder_state(model, &embedder.vocabulary, &embedder.idf)?;
            embedder
        }
    };

    let missing = db.get_chunks_without_embeddings(model)?;
    let total_chunks = missing.len();

    if let Some(cb) = &on_progress {
        cb(ScanProgress {
            source_id: source_id.to_string(),
            phase: "embedding".to_string(),
            current: 0,
            total: total_chunks,
            current_file: None,
        });
    }

    let mut batch: Vec<(String, String, Vec<f32>)> = Vec::with_capacity(missing.len());
    for (i, (chunk_id, content)) in missing.iter().enumerate() {
        let vector = embedder.embed(content)?;
        batch.push((chunk_id.clone(), model.to_string(), vector));

        // Report progress every 50 chunks.
        if let Some(cb) = &on_progress {
            if (i + 1) % 50 == 0 || i + 1 == total_chunks {
                cb(ScanProgress {
                    source_id: source_id.to_string(),
                    phase: "embedding".to_string(),
                    current: i + 1,
                    total: total_chunks,
                    current_file: None,
                });
            }
        }
    }
    let embedded = batch.len();
    if !batch.is_empty() {
        db.batch_store_embeddings(&batch)?;
    }

    let total_source_chunks = db.get_chunks_for_source(source_id)?.len();
    let skipped = total_source_chunks.saturating_sub(embedded);

    info!(
        "Embedding complete for source {}: embedded={}, skipped={}",
        source_id, embedded, skipped
    );

    Ok(EmbedResult {
        source_id: source_id.to_string(),
        chunks_embedded: embedded,
        chunks_skipped: skipped,
        model: model.to_string(),
    })
}

/// Delete all embeddings, rebuild using the configured provider, and
/// re-embed every chunk in the database.
pub fn rebuild_embeddings(db: &Database) -> Result<EmbedResult, CoreError> {
    let config = db.get_embedder_config()?;

    if config.provider == "tfidf" {
        return rebuild_embeddings_tfidf(db);
    }

    let embedder = create_embedder(&config)?;
    let model = embedder.model_name().to_string();

    // 1. Delete all existing embeddings for this model.
    let deleted = db.delete_all_embeddings(&model)?;
    info!("Deleted {} existing embeddings", deleted);

    // 2. Embed every chunk.
    let all_chunks = db.get_all_chunks()?;
    let texts: Vec<&str> = all_chunks.iter().map(|(_, c)| c.as_str()).collect();
    let all_vectors = embedder.embed_batch(&texts)?;
    let batch: Vec<(String, String, Vec<f32>)> = all_chunks
        .iter()
        .zip(all_vectors.into_iter())
        .map(|((chunk_id, _), vector)| (chunk_id.clone(), model.clone(), vector))
        .collect();
    let embedded = batch.len();
    if !batch.is_empty() {
        db.batch_store_embeddings(&batch)?;
    }

    info!("Rebuild complete: {} chunks embedded (provider={})", embedded, config.provider);

    Ok(EmbedResult {
        source_id: "all".to_string(),
        chunks_embedded: embedded,
        chunks_skipped: 0,
        model,
    })
}

/// TF-IDF specific rebuild path (original behaviour).
fn rebuild_embeddings_tfidf(db: &Database) -> Result<EmbedResult, CoreError> {
    let model = "tfidf-v1";

    let deleted = db.delete_all_embeddings(model)?;
    info!("Deleted {} existing embeddings", deleted);

    let all_chunks = db.get_all_chunks()?;
    let corpus: Vec<&str> = all_chunks.iter().map(|(_, c)| c.as_str()).collect();
    let embedder = TfIdfEmbedder::build_from_corpus(&corpus);
    db.save_embedder_state(model, &embedder.vocabulary, &embedder.idf)?;

    let mut batch: Vec<(String, String, Vec<f32>)> = Vec::with_capacity(all_chunks.len());
    for (chunk_id, content) in &all_chunks {
        let vector = embedder.embed(content)?;
        batch.push((chunk_id.clone(), model.to_string(), vector));
    }
    let embedded = batch.len();
    if !batch.is_empty() {
        db.batch_store_embeddings(&batch)?;
    }

    info!("Rebuild complete: {} chunks embedded", embedded);

    Ok(EmbedResult {
        source_id: "all".to_string(),
        chunks_embedded: embedded,
        chunks_skipped: 0,
        model: model.to_string(),
    })
}

/// Insert multiple parsed documents in a single transaction for bulk operations.
///
/// Much faster than calling `insert_document` per file, as SQLite transactions
/// are expensive per-call. Returns the number of documents inserted.
pub fn batch_insert_documents(
    db: &Database,
    source_id: &str,
    parsed_docs: &[ParsedDocument],
) -> Result<usize, CoreError> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let mut count = 0usize;
    for parsed in parsed_docs {
        let doc_id = uuid::Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size,
                                    modified_at, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), ?7)",
            params![
                &doc_id,
                source_id,
                &parsed.file_path,
                &parsed.file_name,
                &parsed.mime_type,
                parsed.file_size,
                &parsed.content_hash,
            ],
        )?;
        insert_chunks(&tx, &doc_id, &parsed.chunks)?;
        count += 1;
    }
    tx.commit()?;
    Ok(count)
}

/// Update multiple existing documents in a single transaction for bulk operations.
///
/// Each entry is `(doc_id, parsed_document)`. Old chunks are deleted first
/// and new chunks are inserted, all within a single transaction.
pub fn batch_update_documents(
    db: &Database,
    updates: &[(String, ParsedDocument)],
) -> Result<usize, CoreError> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let mut count = 0usize;
    for (doc_id, parsed) in updates {
        // Delete old chunks — FTS triggers fire automatically.
        tx.execute(
            "DELETE FROM chunks WHERE document_id = ?1",
            params![doc_id],
        )?;

        // Update the document record.
        tx.execute(
            "UPDATE documents
             SET mime_type = ?1, file_size = ?2, modified_at = datetime('now'),
                 content_hash = ?3, indexed_at = datetime('now')
             WHERE id = ?4",
            params![
                &parsed.mime_type,
                parsed.file_size,
                &parsed.content_hash,
                doc_id,
            ],
        )?;

        insert_chunks(&tx, doc_id, &parsed.chunks)?;
        count += 1;
    }
    tx.commit()?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// Database methods for document CRUD
// ---------------------------------------------------------------------------

impl Database {
    /// Look up a document by its file path.
    ///
    /// Returns `(id, content_hash)` if a matching row exists, `None` otherwise.
    pub fn get_document_by_path(
        &self,
        file_path: &str,
    ) -> Result<Option<(String, String)>, CoreError> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, content_hash FROM documents WHERE path = ?1")?;
        let result = stmt.query_row(params![file_path], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        });
        match result {
            Ok(pair) => Ok(Some(pair)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CoreError::Database(e)),
        }
    }

    /// Pre-fetch all document paths and content hashes for a given source.
    ///
    /// Returns a `HashMap` from file path to `(document_id, content_hash)`,
    /// enabling O(1) lookups instead of N individual database queries.
    pub fn get_document_paths_for_source(
        &self,
        source_id: &str,
    ) -> Result<HashMap<String, (String, String)>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, path, content_hash FROM documents WHERE source_id = ?1",
        )?;
        let rows = stmt.query_map(params![source_id], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(0)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, id, hash) = row?;
            map.insert(path, (id, hash));
        }
        Ok(map)
    }

    /// Insert a new document and all its chunks within a single transaction.
    ///
    /// Returns the generated document ID.
    pub fn insert_document(
        &self,
        source_id: &str,
        parsed: &ParsedDocument,
    ) -> Result<String, CoreError> {
        let doc_id = uuid::Uuid::new_v4().to_string();

        let mut conn = self.conn();
        let tx = conn.transaction()?;

        tx.execute(
            "INSERT INTO documents (id, source_id, path, title, mime_type, file_size,
                                    modified_at, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), ?7)",
            params![
                &doc_id,
                source_id,
                &parsed.file_path,
                &parsed.file_name,
                &parsed.mime_type,
                parsed.file_size,
                &parsed.content_hash,
            ],
        )?;

        insert_chunks(&tx, &doc_id, &parsed.chunks)?;

        tx.commit()?;
        Ok(doc_id)
    }

    /// Update an existing document record and replace all its chunks.
    ///
    /// Old chunks are deleted first (FTS triggers handle cleanup),
    /// then the document row is updated and new chunks are inserted.
    pub fn update_document(
        &self,
        doc_id: &str,
        parsed: &ParsedDocument,
    ) -> Result<(), CoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;

        // Delete old chunks — FTS triggers fire automatically.
        tx.execute(
            "DELETE FROM chunks WHERE document_id = ?1",
            params![doc_id],
        )?;

        // Update the document record.
        tx.execute(
            "UPDATE documents
             SET mime_type = ?1, file_size = ?2, modified_at = datetime('now'),
                 content_hash = ?3, indexed_at = datetime('now')
             WHERE id = ?4",
            params![
                &parsed.mime_type,
                parsed.file_size,
                &parsed.content_hash,
                doc_id,
            ],
        )?;

        insert_chunks(&tx, doc_id, &parsed.chunks)?;

        tx.commit()?;
        Ok(())
    }

    /// Delete all documents (and their chunks via CASCADE) for a source.
    ///
    /// Returns the number of documents deleted.
    pub fn delete_documents_for_source(
        &self,
        source_id: &str,
    ) -> Result<usize, CoreError> {
        let conn = self.conn();
        let count = conn.execute(
            "DELETE FROM documents WHERE source_id = ?1",
            params![source_id],
        )?;
        Ok(count)
    }

    /// Delete a document (and its chunks via CASCADE) by file path.
    ///
    /// Returns `true` if a document was found and deleted, `false` if no
    /// document matched the given path.
    pub fn delete_document_by_path(&self, file_path: &str) -> Result<bool, CoreError> {
        let conn = self.conn();
        let deleted = conn.execute(
            "DELETE FROM documents WHERE path = ?1",
            params![file_path],
        )?;
        Ok(deleted > 0)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Classification of a file during scanning.
enum FileClassification {
    /// New file — not yet in the database.
    New(ParsedDocument),
    /// Changed file — content hash differs from stored version.
    Changed(String, ParsedDocument), // (doc_id, parsed)
    /// Unchanged file — content hash matches, skip.
    Unchanged,
}

/// Classify a file as new, changed, or unchanged (without performing DB writes).
fn classify_file(
    path: &Path,
    existing_docs: &HashMap<String, (String, String)>,
    privacy: &PrivacyConfig,
) -> Result<FileClassification, CoreError> {
    let mut parsed = parse_file(path)?;

    // Apply content redaction when privacy is enabled.
    if privacy.enabled {
        for chunk in &mut parsed.chunks {
            chunk.content = privacy::redact_content(&chunk.content, &privacy.redact_patterns);
        }
    }

    match existing_docs.get(&parsed.file_path) {
        Some((doc_id, existing_hash)) => {
            if *existing_hash == parsed.content_hash {
                debug!("Skipping unchanged file: {}", parsed.file_path);
                Ok(FileClassification::Unchanged)
            } else {
                debug!("File changed: {}", parsed.file_path);
                Ok(FileClassification::Changed(doc_id.clone(), parsed))
            }
        }
        None => {
            debug!("New file: {}", parsed.file_path);
            Ok(FileClassification::New(parsed))
        }
    }
}

/// Insert chunks for a document within an existing transaction.
fn insert_chunks(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    chunks: &[ParsedChunk],
) -> Result<(), CoreError> {
    for chunk in chunks {
        let chunk_id = uuid::Uuid::new_v4().to_string();
        let chunk_hash = blake3::hash(chunk.content.as_bytes())
            .to_hex()
            .to_string();
        let line_end = chunk.content.lines().count().max(1) as i64;
        let metadata = match &chunk.heading_context {
            Some(h) => format!(
                r#"{{"heading_context":{}}}"#,
                serde_json::to_string(h).unwrap_or_default()
            ),
            None => "{}".to_string(),
        };

        tx.execute(
            "INSERT INTO chunks (id, document_id, chunk_index, kind, content,
                                 start_offset, end_offset, line_start, line_end,
                                 content_hash, metadata_json)
             VALUES (?1, ?2, ?3, 'text', ?4, ?5, ?6, 1, ?7, ?8, ?9)",
            params![
                &chunk_id,
                doc_id,
                chunk.chunk_index,
                &chunk.content,
                chunk.start_offset,
                chunk.end_offset,
                line_end,
                &chunk_hash,
                &metadata,
            ],
        )?;
    }
    Ok(())
}

/// Build a `GlobSet` from a list of glob pattern strings.
fn build_glob_set(patterns: &[String]) -> Result<GlobSet, CoreError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid glob pattern '{pattern}': {e}"))
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| CoreError::InvalidInput(format!("Failed to build glob set: {e}")))
}

/// Ingest a single file for incremental watcher events.
///
/// Parses the file, checks its content hash against the database, and
/// inserts or updates the document accordingly. Returns the outcome.
pub fn ingest_single_file(
    db: &Database,
    source_id: &str,
    path: &Path,
) -> Result<IngestFileResult, CoreError> {
    if !path.is_file() {
        return Err(CoreError::InvalidInput(format!(
            "Path is not a file: {}",
            path.display()
        )));
    }

    // Skip files exceeding the size limit.
    const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() > MAX_FILE_SIZE {
            warn!(
                "Skipping large file ({}MB): {}",
                meta.len() / 1024 / 1024,
                path.display()
            );
            return Ok(IngestFileResult::Unchanged);
        }
    }

    // Load privacy config for redaction.
    let privacy_cfg = db.load_privacy_config()?;

    let mut parsed = parse_file(path)?;

    // Apply content redaction when privacy is enabled.
    if privacy_cfg.enabled {
        for chunk in &mut parsed.chunks {
            chunk.content = privacy::redact_content(&chunk.content, &privacy_cfg.redact_patterns);
        }
    }

    // Check if the document already exists.
    match db.get_document_by_path(&parsed.file_path)? {
        Some((doc_id, existing_hash)) => {
            if existing_hash == parsed.content_hash {
                debug!("Single-file ingest: unchanged {}", parsed.file_path);
                Ok(IngestFileResult::Unchanged)
            } else {
                debug!("Single-file ingest: updating {}", parsed.file_path);
                db.update_document(&doc_id, &parsed)?;
                Ok(IngestFileResult::Updated)
            }
        }
        None => {
            debug!("Single-file ingest: adding {}", parsed.file_path);
            db.insert_document(source_id, &parsed)?;
            Ok(IngestFileResult::Added)
        }
    }
}

/// Recursively walk a directory, collecting all file paths (sorted).
fn walk_directory(root: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let mut files = Vec::new();
    walk_recursive(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), CoreError> {
    let entries = fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_recursive(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;
    use std::fs;
    use tempfile::TempDir;

    fn test_db() -> Database {
        let db = Database::open_memory().expect("open in-memory db");
        db.save_embedder_config(&crate::embed::EmbedderConfig {
            provider: "tfidf".into(),
            ..crate::embed::EmbedderConfig::default()
        }).expect("set tfidf config for test");
        db
    }

    fn create_test_source(
        db: &Database,
        dir: &Path,
        include: Vec<String>,
        exclude: Vec<String>,
    ) -> String {
        db.add_source(CreateSourceInput {
            root_path: dir.to_string_lossy().to_string(),
            include_globs: include,
            exclude_globs: exclude,
            watch_enabled: false,
        })
        .expect("add source")
        .id
    }

    fn vault_path() -> PathBuf {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest
            .join("..")
            .join("..")
            .join("testdata")
            .join("sample_vault")
    }

    // ── Scan sample vault ───────────────────────────────────────────────

    #[test]
    fn test_scan_sample_vault() {
        let vp = vault_path();
        if !vp.exists() {
            eprintln!("Skipping: test vault not found at {}", vp.display());
            return;
        }

        let db = test_db();
        let source_id = create_test_source(&db, &vp, vec![], vec![]);

        let result = scan_source(&db, &source_id).expect("scan_source");

        assert_eq!(result.source_id, source_id);
        // 6 files: 2 in docs/, 2 in notes/, 2 in logs/
        assert!(
            result.files_scanned >= 6,
            "expected >= 6 files scanned, got {}",
            result.files_scanned
        );
        assert_eq!(result.files_added, result.files_scanned);
        assert_eq!(result.files_updated, 0);
        assert_eq!(result.files_skipped, 0);
    }

    // ── Incremental scanning ────────────────────────────────────────────

    #[test]
    fn test_incremental_scan_skips_unchanged() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("hello.md");
        fs::write(
            &file,
            "# Hello\n\nThis is a test document with enough content to pass the \
             minimum chunk size threshold for parsing. It needs at least fifty \
             characters to not be discarded by the chunker.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);

        // First scan — adds the file.
        let r1 = scan_source(&db, &sid).unwrap();
        assert_eq!(r1.files_added, 1);
        assert_eq!(r1.files_skipped, 0);

        // Second scan — same content, should skip.
        let r2 = scan_source(&db, &sid).unwrap();
        assert_eq!(r2.files_added, 0);
        assert_eq!(r2.files_skipped, 1);
        assert_eq!(r2.files_updated, 0);
    }

    #[test]
    fn test_incremental_scan_detects_changes() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("doc.md");
        fs::write(
            &file,
            "# Original\n\nOriginal content that is long enough to be a valid \
             chunk for the parser to process correctly and not be discarded.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);

        let r1 = scan_source(&db, &sid).unwrap();
        assert_eq!(r1.files_added, 1);

        // Modify file content so the hash changes.
        fs::write(
            &file,
            "# Modified\n\nCompletely different content that should trigger an \
             update because the blake3 hash will differ from the original text.",
        )
        .unwrap();

        let r2 = scan_source(&db, &sid).unwrap();
        assert_eq!(r2.files_updated, 1);
        assert_eq!(r2.files_added, 0);
        assert_eq!(r2.files_skipped, 0);
    }

    // ── Glob filtering ──────────────────────────────────────────────────

    #[test]
    fn test_glob_include_filter() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("readme.md"),
            "# Readme\n\nMarkdown file with plenty of content to satisfy the \
             minimum chunk size requirement for the parser to accept it.",
        )
        .unwrap();
        fs::write(
            tmp.path().join("notes.txt"),
            "Plain text notes that are long enough to pass the minimum chunk \
             size in the parser so they actually produce at least one chunk.",
        )
        .unwrap();
        fs::write(
            tmp.path().join("data.log"),
            "2025-07-15 10:00:00 INFO Log entry data with plenty of content \
             here to meet minimum requirements for chunking algorithm.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(
            &db,
            tmp.path(),
            vec!["**/*.md".to_string()],
            vec![],
        );

        let result = scan_source(&db, &sid).unwrap();
        assert_eq!(result.files_scanned, 1, "only .md files should be scanned");
        assert_eq!(result.files_added, 1);
    }

    #[test]
    fn test_glob_exclude_filter() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("readme.md"),
            "# Readme\n\nA markdown file with enough content to be parsed into \
             at least one chunk by the parser for indexing purposes.",
        )
        .unwrap();
        fs::write(
            tmp.path().join("notes.txt"),
            "Some plain text notes that contain enough words and characters to \
             pass as a valid parseable chunk in the plain text chunker.",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("logs")).unwrap();
        fs::write(
            tmp.path().join("logs").join("app.log"),
            "2025-07-15 10:00:00 INFO Log file entry with timestamp and plenty \
             of text to ensure the chunk size minimum is met by the parser.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(
            &db,
            tmp.path(),
            vec![],
            vec!["**/*.log".to_string()],
        );

        let result = scan_source(&db, &sid).unwrap();
        assert_eq!(result.files_scanned, 2, "log files should be excluded");
    }

    // ── Error handling ──────────────────────────────────────────────────

    #[test]
    fn test_scan_missing_path() {
        let db = test_db();

        // Insert a source with a non-existent path directly (bypassing
        // add_source validation).
        let id = uuid::Uuid::new_v4().to_string();
        {
            let conn = db.conn();
            conn.execute(
                "INSERT INTO sources (id, kind, root_path) \
                 VALUES (?1, 'local_folder', '/nonexistent/path/abc123')",
                params![&id],
            )
            .unwrap();
        }

        let result = scan_source(&db, &id);
        assert!(result.is_err());
        match result.unwrap_err() {
            CoreError::InvalidInput(msg) => {
                assert!(
                    msg.contains("does not exist"),
                    "expected 'does not exist' in: {msg}"
                );
            }
            other => panic!("expected InvalidInput, got: {other:?}"),
        }
    }

    // ── Document CRUD ───────────────────────────────────────────────────

    #[test]
    fn test_document_crud_via_db() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.md");
        fs::write(
            &file,
            "# Test\n\nSome content for the test document that is long enough \
             to meet the minimum chunk size requirement for parsing.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);

        // Parse and insert.
        let parsed = parse_file(&file).unwrap();
        let doc_id = db.insert_document(&sid, &parsed).unwrap();
        assert!(!doc_id.is_empty());

        // Lookup by path — should find the document.
        let found = db.get_document_by_path(&parsed.file_path).unwrap();
        assert!(found.is_some());
        let (fid, fhash) = found.unwrap();
        assert_eq!(fid, doc_id);
        assert_eq!(fhash, parsed.content_hash);

        // Lookup missing path — should return None.
        let missing = db.get_document_by_path("/no/such/file.md").unwrap();
        assert!(missing.is_none());

        // Update with new content.
        fs::write(
            &file,
            "# Updated\n\nNew content for the updated test document that should \
             produce a different blake3 content hash value now.",
        )
        .unwrap();
        let parsed2 = parse_file(&file).unwrap();
        db.update_document(&doc_id, &parsed2).unwrap();

        let (_, new_hash) = db
            .get_document_by_path(&parsed2.file_path)
            .unwrap()
            .unwrap();
        assert_ne!(new_hash, parsed.content_hash);
        assert_eq!(new_hash, parsed2.content_hash);

        // Delete all docs for source.
        let deleted = db.delete_documents_for_source(&sid).unwrap();
        assert_eq!(deleted, 1);

        let gone = db.get_document_by_path(&parsed2.file_path).unwrap();
        assert!(gone.is_none());
    }

    // ── FTS integration ─────────────────────────────────────────────────

    #[test]
    fn test_ingested_chunks_are_fts_searchable() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("searchable.md"),
            "# Searchable\n\nThis document contains the unique sentinel word \
             xylophonezebra that we will search for in the full-text index.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);

        scan_source(&db, &sid).unwrap();

        let conn = db.conn();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_chunks WHERE fts_chunks MATCH 'xylophonezebra'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "FTS should find the ingested chunk");
    }

    // ── Embedding integration ───────────────────────────────────────────

    #[test]
    fn test_embed_source_after_scan() {
        let vp = vault_path();
        if !vp.exists() {
            eprintln!("Skipping: test vault not found at {}", vp.display());
            return;
        }

        let db = test_db();
        let sid = create_test_source(&db, &vp, vec![], vec![]);

        // Scan first to populate chunks.
        let scan = scan_source(&db, &sid).unwrap();
        assert!(scan.files_added > 0);

        // Now embed.
        let embed = embed_source(&db, &sid).unwrap();
        assert_eq!(embed.source_id, sid);
        assert!(embed.chunks_embedded > 0, "should embed at least one chunk");
        assert_eq!(embed.model, "tfidf-v1");
    }

    #[test]
    fn test_all_chunks_get_embeddings() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("file1.md"),
            "# File One\n\nFirst document with enough content to satisfy \
             the minimum chunk size requirement for the parser to accept it.",
        )
        .unwrap();
        fs::write(
            tmp.path().join("file2.md"),
            "# File Two\n\nSecond document also with plenty of content to \
             be properly chunked and indexed by the ingestion pipeline.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);
        scan_source(&db, &sid).unwrap();

        let embed = embed_source(&db, &sid).unwrap();
        assert!(embed.chunks_embedded > 0);

        // Every chunk should now have an embedding.
        let missing = db.get_chunks_without_embeddings("tfidf-v1").unwrap();
        assert!(
            missing.is_empty(),
            "all chunks should have embeddings, but {} are missing",
            missing.len()
        );
    }

    #[test]
    fn test_rebuild_embeddings_clears_and_reembeds() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("doc.md"),
            "# Rebuild Test\n\nDocument used to verify that rebuild_embeddings \
             deletes existing vectors and creates fresh ones from scratch.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);
        scan_source(&db, &sid).unwrap();

        // Initial embed.
        let e1 = embed_source(&db, &sid).unwrap();
        assert!(e1.chunks_embedded > 0);

        // Rebuild.
        let rebuild = rebuild_embeddings(&db).unwrap();
        assert!(rebuild.chunks_embedded > 0);
        assert_eq!(rebuild.chunks_skipped, 0);
        assert_eq!(rebuild.model, "tfidf-v1");

        // All chunks should still have embeddings after rebuild.
        let missing = db.get_chunks_without_embeddings("tfidf-v1").unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn test_incremental_embedding() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("first.md"),
            "# First\n\nInitial document with enough text to be parsed and \
             chunked properly by the ingestion system for embedding.",
        )
        .unwrap();

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);
        scan_source(&db, &sid).unwrap();

        let e1 = embed_source(&db, &sid).unwrap();
        let initial_embedded = e1.chunks_embedded;
        assert!(initial_embedded > 0);

        // Add a second file.
        fs::write(
            tmp.path().join("second.md"),
            "# Second\n\nA brand new document added after the first embedding \
             run to verify that only new chunks get embedded incrementally.",
        )
        .unwrap();

        // Re-scan picks up the new file.
        let scan2 = scan_source(&db, &sid).unwrap();
        assert_eq!(scan2.files_added, 1);

        // Embed again — should only embed the new chunks.
        let e2 = embed_source(&db, &sid).unwrap();
        assert!(
            e2.chunks_embedded > 0,
            "new chunks should be embedded"
        );

        // No chunks should be missing.
        let missing = db.get_chunks_without_embeddings("tfidf-v1").unwrap();
        assert!(missing.is_empty());
    }

    #[test]
    fn test_batch_insert_documents() {
        let tmp = TempDir::new().unwrap();
        for i in 0..100 {
            fs::write(
                tmp.path().join(format!("doc_{:03}.md", i)),
                format!(
                    "# Document {i}\n\nThis is test document number {i} with enough \
                     content to pass the minimum chunk size requirement for parsing.",
                ),
            )
            .unwrap();
        }

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);

        // Parse all files.
        let mut parsed_docs: Vec<crate::parse::ParsedDocument> = Vec::new();
        for entry in fs::read_dir(tmp.path()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                parsed_docs.push(parse_file(&path).unwrap());
            }
        }
        assert_eq!(parsed_docs.len(), 100);

        let count = batch_insert_documents(&db, &sid, &parsed_docs).unwrap();
        assert_eq!(count, 100);

        // Verify all documents exist.
        for parsed in &parsed_docs {
            let found = db.get_document_by_path(&parsed.file_path).unwrap();
            assert!(
                found.is_some(),
                "Document {} should exist",
                parsed.file_path
            );
        }
    }

    #[test]
    fn test_scan_source_prefetch_many_files() {
        let tmp = TempDir::new().unwrap();
        for i in 0..10 {
            fs::write(
                tmp.path().join(format!("file_{}.md", i)),
                format!(
                    "# File {i}\n\nContent of file number {i} with sufficient text to \
                     pass the minimum chunk size requirement for the parser.",
                ),
            )
            .unwrap();
        }

        let db = test_db();
        let sid = create_test_source(&db, tmp.path(), vec![], vec![]);

        // First scan adds all files.
        let r1 = scan_source(&db, &sid).unwrap();
        assert_eq!(r1.files_added, 10);

        // Second scan — pre-fetched paths/hashes make all lookups skip.
        let r2 = scan_source(&db, &sid).unwrap();
        assert_eq!(r2.files_skipped, 10);
        assert_eq!(r2.files_added, 0);
        assert_eq!(r2.files_updated, 0);
    }
}
