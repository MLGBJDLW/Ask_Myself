//! Bounded, cancellable embedding jobs.
//!
//! This module owns source scoping, bounded batches, cancellation checkpoints,
//! progress, and immediate per-batch persistence for embedding work.

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::db::Database;
use crate::embed::{create_embedder_with_limits, Embedder, EmbeddingRuntimeLimits, TfIdfEmbedder};
use crate::error::CoreError;

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

/// Stable resource policy for one embedding job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddingJobLimits {
    pub batch_size: usize,
    pub max_intra_threads: usize,
}

impl Default for EmbeddingJobLimits {
    fn default() -> Self {
        Self {
            batch_size: 8,
            max_intra_threads: 2,
        }
    }
}

/// Control plane supplied by the caller of an embedding job.
pub trait EmbeddingJobControl {
    fn checkpoint(&self) -> Result<(), CoreError> {
        Ok(())
    }

    fn on_progress(&self, _progress: ScanProgress) {}
}

#[derive(Debug, Default)]
pub struct NoopEmbeddingJobControl;

impl EmbeddingJobControl for NoopEmbeddingJobControl {}

struct CallbackControl<'a> {
    callback: &'a dyn Fn(ScanProgress),
}

impl EmbeddingJobControl for CallbackControl<'_> {
    fn on_progress(&self, progress: ScanProgress) {
        (self.callback)(progress);
    }
}

pub fn embed_source(db: &Database, source_id: &str) -> Result<EmbedResult, CoreError> {
    run_source(
        db,
        source_id,
        EmbeddingJobLimits::default(),
        &NoopEmbeddingJobControl,
    )
}

pub fn embed_source_with_progress(
    db: &Database,
    source_id: &str,
    on_progress: impl Fn(ScanProgress),
) -> Result<EmbedResult, CoreError> {
    run_source(
        db,
        source_id,
        EmbeddingJobLimits::default(),
        &CallbackControl {
            callback: &on_progress,
        },
    )
}

pub fn run_source(
    db: &Database,
    source_id: &str,
    limits: EmbeddingJobLimits,
    control: &dyn EmbeddingJobControl,
) -> Result<EmbedResult, CoreError> {
    control.checkpoint()?;
    let config = db.get_embedder_config()?;
    let batch_size = limits.batch_size.max(1);

    let (model, embedder): (String, Box<dyn Embedder>) = if config.provider == "tfidf" {
        let model = "tfidf-v1".to_string();
        let embedder = load_or_build_tfidf(db, &model, control)?;
        (model, Box::new(embedder))
    } else {
        let embedder = create_embedder_with_limits(
            &config,
            EmbeddingRuntimeLimits {
                max_intra_threads: limits.max_intra_threads,
            },
        )?;
        (embedder.model_name().to_string(), embedder)
    };

    let total_source_chunks = db.count_chunks_for_source(source_id)?;
    let total_missing = db.count_chunks_without_embeddings_for_source(source_id, &model)?;
    control.on_progress(progress(source_id, 0, total_missing));

    let mut embedded = 0usize;
    loop {
        control.checkpoint()?;
        let chunks =
            db.get_chunks_without_embeddings_for_source_batch(source_id, &model, batch_size)?;
        if chunks.is_empty() {
            break;
        }

        persist_embedded_batch(db, &model, &*embedder, &chunks, control)?;
        embedded += chunks.len();
        control.on_progress(progress(source_id, embedded, total_missing));
    }

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

pub fn rebuild_embeddings(db: &Database) -> Result<EmbedResult, CoreError> {
    rebuild_all(db, EmbeddingJobLimits::default(), &NoopEmbeddingJobControl)
}

pub fn rebuild_embeddings_with_progress(
    db: &Database,
    on_progress: impl Fn(ScanProgress),
) -> Result<EmbedResult, CoreError> {
    rebuild_all(
        db,
        EmbeddingJobLimits::default(),
        &CallbackControl {
            callback: &on_progress,
        },
    )
}

pub fn rebuild_all(
    db: &Database,
    limits: EmbeddingJobLimits,
    control: &dyn EmbeddingJobControl,
) -> Result<EmbedResult, CoreError> {
    control.checkpoint()?;
    let config = db.get_embedder_config()?;
    let batch_size = limits.batch_size.max(1);

    let (model, embedder): (String, Box<dyn Embedder>) = if config.provider == "tfidf" {
        let model = "tfidf-v1".to_string();
        let all_chunks = db.get_all_chunks()?;
        control.checkpoint()?;
        let corpus: Vec<&str> = all_chunks
            .iter()
            .map(|(_, content)| content.as_str())
            .collect();
        let embedder = TfIdfEmbedder::build_from_corpus(&corpus);
        db.save_embedder_state(&model, &embedder.vocabulary, &embedder.idf)?;
        drop(all_chunks);
        (model, Box::new(embedder))
    } else {
        let embedder = create_embedder_with_limits(
            &config,
            EmbeddingRuntimeLimits {
                max_intra_threads: limits.max_intra_threads,
            },
        )?;
        (embedder.model_name().to_string(), embedder)
    };

    let deleted = db.delete_all_embeddings(&model)?;
    info!("Deleted {} existing embeddings", deleted);

    let total_chunks = db.count_all_chunks()?;
    control.on_progress(progress("all", 0, total_chunks));
    let mut embedded = 0usize;

    loop {
        control.checkpoint()?;
        let chunks = db.get_chunks_without_embeddings_batch(&model, batch_size)?;
        if chunks.is_empty() {
            break;
        }
        persist_embedded_batch(db, &model, &*embedder, &chunks, control)?;
        embedded += chunks.len();
        control.on_progress(progress("all", embedded, total_chunks));
    }

    info!(
        "Rebuild complete: {} chunks embedded (provider={})",
        embedded, config.provider
    );
    Ok(EmbedResult {
        source_id: "all".to_string(),
        chunks_embedded: embedded,
        chunks_skipped: 0,
        model,
    })
}

fn load_or_build_tfidf(
    db: &Database,
    model: &str,
    control: &dyn EmbeddingJobControl,
) -> Result<TfIdfEmbedder, CoreError> {
    if let Some((vocab, idf)) = db.load_embedder_state(model)? {
        info!("Loaded existing embedder state for model '{}'", model);
        return Ok(TfIdfEmbedder::from_vocabulary(vocab, idf));
    }

    control.checkpoint()?;
    info!("No saved embedder state; building TF-IDF from full corpus");
    let all_chunks = db.get_all_chunks()?;
    let corpus: Vec<&str> = all_chunks
        .iter()
        .map(|(_, content)| content.as_str())
        .collect();
    let embedder = TfIdfEmbedder::build_from_corpus(&corpus);
    db.save_embedder_state(model, &embedder.vocabulary, &embedder.idf)?;
    Ok(embedder)
}

fn persist_embedded_batch(
    db: &Database,
    model: &str,
    embedder: &dyn Embedder,
    chunks: &[(String, String)],
    control: &dyn EmbeddingJobControl,
) -> Result<(), CoreError> {
    let texts: Vec<&str> = chunks.iter().map(|(_, content)| content.as_str()).collect();
    let vectors = embedder.embed_documents(&texts)?;
    if vectors.len() != chunks.len() {
        return Err(CoreError::Embedding(format!(
            "embedding batch cardinality mismatch: expected {}, got {}",
            chunks.len(),
            vectors.len()
        )));
    }
    control.checkpoint()?;
    let batch: Vec<(String, String, Vec<f32>)> = chunks
        .iter()
        .zip(vectors)
        .map(|((chunk_id, _), vector)| (chunk_id.clone(), model.to_string(), vector))
        .collect();
    db.batch_store_embeddings(&batch)?;
    Ok(())
}

fn progress(source_id: &str, current: usize, total: usize) -> ScanProgress {
    ScanProgress {
        source_id: source_id.to_string(),
        phase: "embedding".to_string(),
        current,
        total,
        current_file: None,
    }
}
