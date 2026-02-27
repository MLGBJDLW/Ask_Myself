//! TF-IDF embedding module for local vector search.
//!
//! Provides a trait-based pluggable embedder design with a concrete
//! TF-IDF implementation that requires no external services.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::CoreError;

// ── Embedder Config ─────────────────────────────────────────────────

/// Persisted configuration that determines which embedder the application
/// uses and how it's parameterised.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedderConfig {
    /// `"local"` | `"api"` | `"tfidf"`
    pub provider: String,
    pub api_key: String,
    pub api_base_url: String,
    pub api_model: String,
    pub model_path: String,
    pub vector_dimensions: u32,
    /// Which local ONNX model to use: `"MultilingualMiniLM"` (default) or `"MultilingualE5Base"`.
    #[serde(default)]
    pub local_model: String,
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            provider: "local".into(),
            api_key: String::new(),
            api_base_url: "https://api.openai.com/v1".into(),
            api_model: "text-embedding-3-small".into(),
            model_path: String::new(),
            vector_dimensions: 384,
            local_model: LocalEmbeddingModel::default().to_config_str().to_string(),
        }
    }
}

impl EmbedderConfig {
    /// Parse the `local_model` field into a [`LocalEmbeddingModel`].
    pub fn local_embedding_model(&self) -> LocalEmbeddingModel {
        LocalEmbeddingModel::from_config_str(&self.local_model)
    }
}

/// Create the appropriate embedder based on an [`EmbedderConfig`].
///
/// - `"local"` → [`OnnxEmbedder`] (downloads model on first use)
/// - `"api"`   → [`ApiEmbedder`] (OpenAI-compatible)
/// - `"tfidf"` → returns an error; TF-IDF requires corpus-based
///    construction via [`TfIdfEmbedder::build_from_corpus`] so this
///    factory cannot create one directly. Callers should handle TF-IDF
///    separately.
pub fn create_embedder(config: &EmbedderConfig) -> Result<Box<dyn Embedder>, CoreError> {
    match config.provider.as_str() {
        "local" => {
            let model_path = if config.model_path.is_empty() {
                None
            } else {
                Some(config.model_path.as_str())
            };
            let local_model = config.local_embedding_model();
            if check_local_model_exists_for(model_path, &local_model) {
                let model_dir = model_path.map(PathBuf::from);
                let embedder = OnnxEmbedder::new(model_dir, local_model)?;
                Ok(Box::new(embedder))
            } else {
                tracing::warn!(
                    "ONNX model not available, falling back to TF-IDF — \
                     search quality will be degraded. \
                     Download the model in Settings."
                );
                Ok(Box::new(TfIdfEmbedder::build_from_corpus(&[])))
            }
        }
        "api" => {
            let base_url = if config.api_base_url.is_empty() {
                None
            } else {
                Some(config.api_base_url.clone())
            };
            let model = if config.api_model.is_empty() {
                None
            } else {
                Some(config.api_model.clone())
            };
            let dims = if config.vector_dimensions > 0 {
                Some(config.vector_dimensions as usize)
            } else {
                None
            };
            let embedder = ApiEmbedder::new(config.api_key.clone(), base_url, model, dims)?;
            Ok(Box::new(embedder))
        }
        "tfidf" => Err(CoreError::InvalidInput(
            "TF-IDF embedder requires corpus construction; use embed_source() directly".into(),
        )),
        other => Err(CoreError::InvalidInput(format!(
            "Unknown embedder provider: {other}"
        ))),
    }
}

/// Check whether the local ONNX model files exist in the default (or given) directory.
pub fn check_local_model_exists(model_path: Option<&str>) -> bool {
    check_local_model_exists_for(model_path, &LocalEmbeddingModel::default())
}

/// Check whether a specific local ONNX model's files exist.
pub fn check_local_model_exists_for(model_path: Option<&str>, model: &LocalEmbeddingModel) -> bool {
    let dir = match model_path {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => match default_model_dir_for(model) {
            Ok(d) => d,
            Err(_) => return false,
        },
    };
    dir.join("model.onnx").exists() && dir.join("tokenizer.json").exists()
}

/// Download the local ONNX model files to the default (or given) directory.
pub fn download_local_model(model_path: Option<&str>) -> Result<(), CoreError> {
    download_local_model_for(model_path, &LocalEmbeddingModel::default())
}

/// Download a specific local ONNX model's files.
pub fn download_local_model_for(
    model_path: Option<&str>,
    model: &LocalEmbeddingModel,
) -> Result<(), CoreError> {
    let dir = match model_path {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => default_model_dir_for(model)?,
    };
    download_model_files(&dir, model)
}

/// Progress information emitted during model download.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub filename: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub file_index: usize,
    pub total_files: usize,
}

/// Download a specific local ONNX model's files, reporting progress via a callback.
pub fn download_local_model_for_with_progress(
    model_path: Option<&str>,
    model: &LocalEmbeddingModel,
    on_progress: impl Fn(DownloadProgress),
) -> Result<(), CoreError> {
    let dir = match model_path {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => default_model_dir_for(model)?,
    };
    download_model_files_with_progress(&dir, model, on_progress)
}

// ── Embedder trait ──────────────────────────────────────────────────

/// Pluggable embedding interface.
pub trait Embedder: Send + Sync {
    /// Human-readable model identifier (e.g. `"tfidf-v1"`).
    fn model_name(&self) -> &str;

    /// Dimensionality of the output vectors.
    fn dimensions(&self) -> usize;

    /// Embed a single text into a dense vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError>;

    /// Embed a batch of texts.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError>;
}

// ── TF-IDF Embedder ─────────────────────────────────────────────────

/// Maximum vocabulary size (and therefore vector dimensionality).
const MAX_DIMENSIONS: usize = 2048;

/// Basic English stop words filtered during tokenization.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "had", "has", "have",
    "he", "her", "his", "how", "if", "in", "into", "is", "it", "its", "me", "my", "no", "nor",
    "not", "of", "on", "or", "our", "out", "own", "she", "so", "than", "that", "the", "their",
    "them", "then", "there", "these", "they", "this", "to", "too", "up", "us", "very", "was", "we",
    "were", "what", "when", "where", "which", "who", "whom", "why", "will", "with", "would", "you",
    "your",
];

/// TF-IDF based embedder that builds a vocabulary from a corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TfIdfEmbedder {
    pub vocabulary: HashMap<String, usize>,
    pub idf: Vec<f32>,
    pub dimensions: usize,
}

impl TfIdfEmbedder {
    /// Build a new embedder from a corpus of documents.
    ///
    /// 1. Tokenizes every document.
    /// 2. Selects the top `MAX_DIMENSIONS` most frequent terms as vocabulary.
    /// 3. Computes IDF for each term.
    pub fn build_from_corpus(documents: &[&str]) -> Self {
        let total_docs = documents.len() as f32;

        // Tokenize all documents.
        let tokenized: Vec<Vec<String>> = documents.iter().map(|d| tokenize(d)).collect();

        // Count total occurrences of each term across all documents.
        let mut global_freq: HashMap<String, usize> = HashMap::new();
        // Count how many documents contain each term (for IDF).
        let mut doc_freq: HashMap<String, usize> = HashMap::new();

        for tokens in &tokenized {
            for token in tokens {
                *global_freq.entry(token.clone()).or_insert(0) += 1;
            }
            // Unique terms in this document.
            let unique: HashSet<&String> = tokens.iter().collect();
            for term in unique {
                *doc_freq.entry(term.clone()).or_insert(0) += 1;
            }
        }

        // Sort by frequency descending, take top MAX_DIMENSIONS.
        let mut freq_list: Vec<(String, usize)> = global_freq.into_iter().collect();
        freq_list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        freq_list.truncate(MAX_DIMENSIONS);

        let dimensions = freq_list.len();
        let mut vocabulary = HashMap::with_capacity(dimensions);
        let mut idf = Vec::with_capacity(dimensions);

        for (idx, (term, _)) in freq_list.iter().enumerate() {
            vocabulary.insert(term.clone(), idx);
            let df = *doc_freq.get(term).unwrap_or(&0) as f32;
            idf.push((total_docs / (1.0 + df)).ln());
        }

        Self {
            vocabulary,
            idf,
            dimensions,
        }
    }

    /// Reconstruct an embedder from a previously saved vocabulary and IDF.
    pub fn from_vocabulary(vocabulary: HashMap<String, usize>, idf: Vec<f32>) -> Self {
        let dimensions = vocabulary.len();
        Self {
            vocabulary,
            idf,
            dimensions,
        }
    }
}

impl Embedder for TfIdfEmbedder {
    fn model_name(&self) -> &str {
        "tfidf-v1"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
        if self.dimensions == 0 {
            return Ok(vec![]);
        }

        let tokens = tokenize(text);
        if tokens.is_empty() {
            return Ok(vec![0.0; self.dimensions]);
        }

        // Compute term frequency for this text.
        let mut tf_counts: HashMap<&str, f32> = HashMap::new();
        let total_tokens = tokens.len() as f32;
        for tok in &tokens {
            *tf_counts.entry(tok.as_str()).or_insert(0.0) += 1.0;
        }

        // Build TF-IDF vector.
        let mut vector = vec![0.0f32; self.dimensions];
        for (term, &count) in &tf_counts {
            if let Some(&idx) = self.vocabulary.get(*term) {
                let tf = count / total_tokens;
                vector[idx] = tf * self.idf[idx];
            }
        }

        // L2 normalize.
        l2_normalize(&mut vector);
        Ok(vector)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
}

// ── Tokenization ────────────────────────────────────────────────────

/// Tokenize text into lowercase terms.
///
/// - Splits on non-alphanumeric boundaries.
/// - Keeps individual CJK characters as tokens.
/// - Removes English tokens shorter than 2 characters.
/// - Filters common English stop words.
fn tokenize(text: &str) -> Vec<String> {
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    let lower = text.to_lowercase();
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in lower.chars() {
        if is_cjk(ch) {
            // Flush any accumulated ASCII word.
            if !current.is_empty() {
                push_if_valid(&mut tokens, &current, &stop);
                current.clear();
            }
            // Each CJK character is its own token.
            tokens.push(ch.to_string());
        } else if ch.is_alphanumeric() {
            current.push(ch);
        } else {
            // Separator — flush current word.
            if !current.is_empty() {
                push_if_valid(&mut tokens, &current, &stop);
                current.clear();
            }
        }
    }
    // Flush trailing word.
    if !current.is_empty() {
        push_if_valid(&mut tokens, &current, &stop);
    }

    tokens
}

/// Push a token if it passes length and stop-word filters.
fn push_if_valid(tokens: &mut Vec<String>, word: &str, stop: &HashSet<&str>) {
    if word.len() >= 2 && !stop.contains(word) {
        tokens.push(word.to_owned());
    }
}

/// Returns `true` if the character is in the CJK Unified Ideographs block.
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x4E00..=0x9FFF   // CJK Unified Ideographs
        | 0x3400..=0x4DBF // Extension A
        | 0xF900..=0xFAFF // Compatibility Ideographs
    )
}

// ── Vector utilities ────────────────────────────────────────────────

/// L2 (Euclidean) normalize a vector in place.
fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity between two vectors.
///
/// Returns 0.0 if either vector has zero magnitude.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "vectors must have equal length");
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Serialize an f32 vector to a little-endian byte blob (for SQLite BLOB).
pub fn vector_to_blob(vec: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vec.len() * 4);
    for &v in vec {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    bytes
}

/// Deserialize a little-endian byte blob back into an f32 vector.
pub fn blob_to_vector(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().expect("chunk is exactly 4 bytes");
            f32::from_le_bytes(arr)
        })
        .collect()
}

// ── Database operations ─────────────────────────────────────────────

impl Database {
    /// Store (upsert) a vector embedding for a chunk.
    // TODO: integrate — single-row insert, production uses batch_store_embeddings()
    pub fn store_embedding(
        &self,
        chunk_id: &str,
        model: &str,
        vector: &[f32],
    ) -> Result<(), CoreError> {
        let blob = vector_to_blob(vector);
        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn();
        conn.execute(
            "INSERT INTO embeddings (id, chunk_id, model, vector, dimensions)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(chunk_id, model) DO UPDATE SET
                vector = excluded.vector,
                dimensions = excluded.dimensions,
                created_at = datetime('now')",
            rusqlite::params![id, chunk_id, model, blob, vector.len() as i64],
        )?;
        Ok(())
    }

    /// Store multiple embeddings in a single transaction for bulk operations.
    ///
    /// Much faster than calling `store_embedding` in a loop, as SQLite
    /// transactions are expensive per-call.
    pub fn batch_store_embeddings(
        &self,
        embeddings: &[(String, String, Vec<f32>)],
    ) -> Result<(), CoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (chunk_id, model, vector) in embeddings {
            let blob = vector_to_blob(vector);
            let id = uuid::Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO embeddings (id, chunk_id, model, vector, dimensions)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(chunk_id, model) DO UPDATE SET
                    vector = excluded.vector,
                    dimensions = excluded.dimensions,
                    created_at = datetime('now')",
                rusqlite::params![id, chunk_id, model, blob, vector.len() as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Retrieve the embedding vector for a specific chunk + model.
    // TODO: integrate — single-row retrieval, production uses batch versions
    pub fn get_embedding(
        &self,
        chunk_id: &str,
        model: &str,
    ) -> Result<Option<Vec<f32>>, CoreError> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT vector FROM embeddings WHERE chunk_id = ?1 AND model = ?2")?;
        let mut rows = stmt.query(rusqlite::params![chunk_id, model])?;
        match rows.next()? {
            Some(row) => {
                let blob: Vec<u8> = row.get(0)?;
                Ok(Some(blob_to_vector(&blob)))
            }
            None => Ok(None),
        }
    }

    /// Retrieve all embeddings for a given model as `(chunk_id, vector)` pairs.
    pub fn get_all_embeddings(&self, model: &str) -> Result<Vec<(String, Vec<f32>)>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT chunk_id, vector FROM embeddings WHERE model = ?1")?;
        let rows = stmt.query_map(rusqlite::params![model], |row| {
            let chunk_id: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((chunk_id, blob))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (chunk_id, blob) = row?;
            results.push((chunk_id, blob_to_vector(&blob)));
        }
        Ok(results)
    }

    /// Retrieve embeddings in batches using LIMIT/OFFSET for streaming large datasets.
    ///
    /// Avoids loading all embeddings into memory at once — essential for 1M+ chunks.
    pub fn get_embeddings_batched(
        &self,
        model: &str,
        batch_size: usize,
        offset: usize,
    ) -> Result<Vec<(String, Vec<f32>)>, CoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT chunk_id, vector FROM embeddings WHERE model = ?1
             ORDER BY chunk_id LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![model, batch_size as i64, offset as i64],
            |row| {
                let chunk_id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((chunk_id, blob))
            },
        )?;
        let mut results = Vec::new();
        for row in rows {
            let (chunk_id, blob) = row?;
            results.push((chunk_id, blob_to_vector(&blob)));
        }
        Ok(results)
    }

    /// Delete all embeddings belonging to chunks of a given document.
    // TODO: integrate — per-document cleanup for incremental re-indexing
    pub fn delete_embeddings_for_document(&self, document_id: &str) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute(
            "DELETE FROM embeddings WHERE chunk_id IN (
                SELECT id FROM chunks WHERE document_id = ?1
            )",
            rusqlite::params![document_id],
        )?;
        Ok(())
    }

    /// Read the [`EmbedderConfig`] from the `embedder_config` key-value table.
    ///
    /// Returns `EmbedderConfig::default()` if the table does not exist or
    /// no rows are present.
    pub fn get_embedder_config(&self) -> Result<EmbedderConfig, CoreError> {
        let conn = self.conn();

        let table_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='embedder_config')",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(EmbedderConfig::default());
        }

        let mut stmt = conn.prepare("SELECT key, value FROM embedder_config")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut config = EmbedderConfig::default();
        for row in rows {
            let (key, value) = row?;
            match key.as_str() {
                "provider" => config.provider = value,
                "api_key" => config.api_key = value,
                "api_base_url" => config.api_base_url = value,
                "api_model" => config.api_model = value,
                "model_path" => config.model_path = value,
                "vector_dimensions" => {
                    config.vector_dimensions = value.parse::<u32>().unwrap_or(384);
                }
                "local_model" => config.local_model = value,
                _ => {} // ignore unknown keys for forward compat
            }
        }
        Ok(config)
    }

    /// Persist an [`EmbedderConfig`] to the `embedder_config` key-value table.
    pub fn save_embedder_config(&self, config: &EmbedderConfig) -> Result<(), CoreError> {
        let conn = self.conn();
        let pairs: &[(&str, String)] = &[
            ("provider", config.provider.clone()),
            ("api_key", config.api_key.clone()),
            ("api_base_url", config.api_base_url.clone()),
            ("api_model", config.api_model.clone()),
            ("model_path", config.model_path.clone()),
            ("vector_dimensions", config.vector_dimensions.to_string()),
            ("local_model", config.local_model.clone()),
        ];
        for (key, value) in pairs {
            conn.execute(
                "INSERT INTO embedder_config (key, value)
                 VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        Ok(())
    }

    /// Persist the embedder's vocabulary and IDF so it can be restored later.
    ///
    /// Uses a lightweight `model_state` table (auto-created on first call).
    pub fn save_embedder_state(
        &self,
        model: &str,
        vocabulary: &HashMap<String, usize>,
        idf: &[f32],
    ) -> Result<(), CoreError> {
        let conn = self.conn();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS model_state (
                model TEXT PRIMARY KEY,
                vocab_json TEXT NOT NULL,
                idf_json TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )?;
        let vocab_json = serde_json::to_string(vocabulary)?;
        let idf_json = serde_json::to_string(idf)?;
        conn.execute(
            "INSERT INTO model_state (model, vocab_json, idf_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(model) DO UPDATE SET
                vocab_json = excluded.vocab_json,
                idf_json = excluded.idf_json,
                updated_at = datetime('now')",
            rusqlite::params![model, vocab_json, idf_json],
        )?;
        Ok(())
    }

    /// Load a previously saved embedder state.
    ///
    /// Returns `None` if the model has never been saved.
    pub fn load_embedder_state(
        &self,
        model: &str,
    ) -> Result<Option<(HashMap<String, usize>, Vec<f32>)>, CoreError> {
        let conn = self.conn();
        // Table might not exist yet.
        let table_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='model_state')",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(None);
        }

        let mut stmt =
            conn.prepare("SELECT vocab_json, idf_json FROM model_state WHERE model = ?1")?;
        let mut rows = stmt.query(rusqlite::params![model])?;
        match rows.next()? {
            Some(row) => {
                let vocab_json: String = row.get(0)?;
                let idf_json: String = row.get(1)?;
                let vocabulary: HashMap<String, usize> = serde_json::from_str(&vocab_json)?;
                let idf: Vec<f32> = serde_json::from_str(&idf_json)?;
                Ok(Some((vocabulary, idf)))
            }
            None => Ok(None),
        }
    }
}

// ── Local Embedding Model Selection ──────────────────────────────────

/// Selectable local ONNX embedding model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LocalEmbeddingModel {
    /// ~46 MB, 384-dim, 50+ languages. Default, fast.
    MultilingualMiniLM,
    /// ~470 MB, 768-dim, 100+ languages. Highest quality.
    MultilingualE5Base,
}

impl Default for LocalEmbeddingModel {
    fn default() -> Self {
        Self::MultilingualMiniLM
    }
}

impl LocalEmbeddingModel {
    pub fn model_name(&self) -> &str {
        match self {
            Self::MultilingualMiniLM => "paraphrase-multilingual-MiniLM-L12-v2",
            Self::MultilingualE5Base => "multilingual-e5-base",
        }
    }

    pub fn dimensions(&self) -> usize {
        match self {
            Self::MultilingualMiniLM => 384,
            Self::MultilingualE5Base => 768,
        }
    }

    pub fn max_length(&self) -> usize {
        match self {
            Self::MultilingualMiniLM => 128,
            Self::MultilingualE5Base => 512,
        }
    }

    pub fn model_url(&self) -> &str {
        match self {
            Self::MultilingualMiniLM => "https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/onnx/model.onnx",
            Self::MultilingualE5Base => "https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/onnx/model.onnx",
        }
    }

    pub fn tokenizer_url(&self) -> &str {
        match self {
            Self::MultilingualMiniLM => "https://huggingface.co/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/tokenizer.json",
            Self::MultilingualE5Base => "https://huggingface.co/intfloat/multilingual-e5-base/resolve/main/tokenizer.json",
        }
    }

    pub fn fallback_model_url(&self) -> &str {
        match self {
            Self::MultilingualMiniLM => "https://hf-mirror.com/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/onnx/model.onnx",
            Self::MultilingualE5Base => "https://hf-mirror.com/intfloat/multilingual-e5-base/resolve/main/onnx/model.onnx",
        }
    }

    pub fn fallback_tokenizer_url(&self) -> &str {
        match self {
            Self::MultilingualMiniLM => "https://hf-mirror.com/sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/tokenizer.json",
            Self::MultilingualE5Base => "https://hf-mirror.com/intfloat/multilingual-e5-base/resolve/main/tokenizer.json",
        }
    }

    /// Whether this model requires a prefix on input texts (e.g. E5 models).
    pub fn requires_prefix(&self) -> bool {
        matches!(self, Self::MultilingualE5Base)
    }

    /// Parse from string stored in DB / config.
    pub fn from_config_str(s: &str) -> Self {
        match s {
            "multilingual-e5-base" | "MultilingualE5Base" => Self::MultilingualE5Base,
            _ => Self::MultilingualMiniLM, // default
        }
    }

    /// String suitable for storing in config.
    pub fn to_config_str(&self) -> &str {
        match self {
            Self::MultilingualMiniLM => "MultilingualMiniLM",
            Self::MultilingualE5Base => "MultilingualE5Base",
        }
    }
}

/// Public constant: default ONNX model name used for embedding lookup.
/// This now delegates to `LocalEmbeddingModel::default().model_name()`
/// but callers that import the constant directly can still work.
pub const ONNX_MODEL_NAME: &str = "paraphrase-multilingual-MiniLM-L12-v2";

/// ONNX-based sentence embedder with configurable model selection.
///
/// Produces L2-normalized embeddings suitable for semantic similarity
/// search.  Model files are downloaded from HuggingFace on first use
/// and cached locally.
pub struct OnnxEmbedder {
    session: std::sync::Mutex<ort::session::Session>,
    tokenizer: tokenizers::Tokenizer,
    model: LocalEmbeddingModel,
}

impl OnnxEmbedder {
    /// Create a new ONNX embedder.
    ///
    /// If `model_dir` is `None`, uses the default cache path at
    /// `<data_dir>/ask-myself/models/<model_name>/`.
    /// Downloads model files from HuggingFace when not already present.
    pub fn new(model_dir: Option<PathBuf>, model: LocalEmbeddingModel) -> Result<Self, CoreError> {
        let dir = match model_dir {
            Some(d) => d,
            None => default_model_dir_for(&model)?,
        };

        let model_path = dir.join("model.onnx");
        let tokenizer_path = dir.join("tokenizer.json");

        if !model_path.exists() || !tokenizer_path.exists() {
            return Err(CoreError::Embedding(
                "ONNX model not downloaded. Please download the model \
                 in Settings before using local embeddings."
                    .into(),
            ));
        }

        // Use half of available CPUs for intra-op parallelism, minimum 1.
        let num_threads = std::thread::available_parallelism()
            .map(|n| (n.get() / 2).max(1))
            .unwrap_or(1);
        tracing::info!(
            "Loading ONNX model from {} (intra_threads={})",
            model_path.display(),
            num_threads
        );
        let session = ort::session::Session::builder()
            .map_err(|e| CoreError::Embedding(format!("session builder: {e}")))?
            .with_intra_threads(num_threads)
            .map_err(|e| CoreError::Embedding(format!("set intra threads: {e}")))?
            .commit_from_file(&model_path)
            .map_err(|e| CoreError::Embedding(format!("load ONNX model: {e}")))?;

        tracing::info!("Loading tokenizer from {}", tokenizer_path.display());
        let mut tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| CoreError::Embedding(format!("load tokenizer: {e}")))?;

        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: model.max_length(),
                ..Default::default()
            }))
            .map_err(|e| CoreError::Embedding(format!("set truncation: {e}")))?;

        tokenizer.with_padding(Some(tokenizers::PaddingParams::default()));

        Ok(Self {
            session: std::sync::Mutex::new(session),
            tokenizer,
            model,
        })
    }
}

impl Embedder for OnnxEmbedder {
    fn model_name(&self) -> &str {
        self.model.model_name()
    }

    fn dimensions(&self) -> usize {
        self.model.dimensions()
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
        let results = self.embed_batch(&[text])?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| CoreError::Embedding("empty result from embed_batch".into()))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        // E5 models require "query: " or "passage: " prefix.
        // For batch embedding (typically documents), use "passage: ".
        let prefixed: Vec<String>;
        let input_texts: Vec<&str> = if self.model.requires_prefix() {
            prefixed = texts.iter().map(|t| format!("passage: {t}")).collect();
            prefixed.iter().map(|s| s.as_str()).collect()
        } else {
            texts.to_vec()
        };

        let encodings = self
            .tokenizer
            .encode_batch(input_texts, true)
            .map_err(|e| CoreError::Embedding(format!("tokenization: {e}")))?;

        let batch_size = encodings.len();
        let seq_len = encodings[0].get_ids().len();

        let mut input_ids = ndarray::Array2::<i64>::zeros((batch_size, seq_len));
        let mut attention_mask = ndarray::Array2::<i64>::zeros((batch_size, seq_len));
        let mut token_type_ids = ndarray::Array2::<i64>::zeros((batch_size, seq_len));

        for (i, enc) in encodings.iter().enumerate() {
            for (j, &id) in enc.get_ids().iter().enumerate() {
                input_ids[[i, j]] = id as i64;
            }
            for (j, &mask) in enc.get_attention_mask().iter().enumerate() {
                attention_mask[[i, j]] = mask as i64;
            }
            for (j, &tid) in enc.get_type_ids().iter().enumerate() {
                token_type_ids[[i, j]] = tid as i64;
            }
        }

        let input_ids_tensor = ort::value::Tensor::from_array(input_ids)
            .map_err(|e| CoreError::Embedding(format!("input_ids tensor: {e}")))?;
        let attention_mask_tensor = ort::value::Tensor::from_array(attention_mask)
            .map_err(|e| CoreError::Embedding(format!("attention_mask tensor: {e}")))?;
        let token_type_ids_tensor = ort::value::Tensor::from_array(token_type_ids)
            .map_err(|e| CoreError::Embedding(format!("token_type_ids tensor: {e}")))?;

        let mut session = self
            .session
            .lock()
            .map_err(|e| CoreError::Embedding(format!("session lock: {e}")))?;

        let outputs = session
            .run(ort::inputs! {
                "input_ids" => input_ids_tensor,
                "attention_mask" => attention_mask_tensor,
                "token_type_ids" => token_type_ids_tensor
            })
            .map_err(|e| CoreError::Embedding(format!("inference: {e}")))?;

        // last_hidden_state: [batch, seq_len, hidden_size]
        let hidden = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| CoreError::Embedding(format!("extract output: {e}")))?;
        let hidden_size = hidden.shape()[2];

        // Mean pooling with attention mask, then L2-normalize.
        let mut results = Vec::with_capacity(batch_size);
        for i in 0..batch_size {
            let mask = encodings[i].get_attention_mask();
            let mut pooled = vec![0.0f32; hidden_size];
            let mut mask_sum = 0.0f32;

            for j in 0..seq_len {
                let m = mask[j] as f32;
                mask_sum += m;
                for k in 0..hidden_size {
                    pooled[k] += hidden[ndarray::IxDyn(&[i, j, k])] * m;
                }
            }

            if mask_sum > 0.0 {
                for v in pooled.iter_mut() {
                    *v /= mask_sum;
                }
            }

            l2_normalize(&mut pooled);
            results.push(pooled);
        }

        Ok(results)
    }
}

/// Default cache directory for a specific ONNX model.
fn default_model_dir_for(model: &LocalEmbeddingModel) -> Result<PathBuf, CoreError> {
    let data_dir = dirs::data_dir()
        .ok_or_else(|| CoreError::Embedding("cannot determine data directory".into()))?;
    Ok(data_dir
        .join("ask-myself")
        .join("models")
        .join(model.model_name()))
}

/// Download `model.onnx` and `tokenizer.json` from HuggingFace.
///
/// Tries the primary HuggingFace URL first, then falls back to the
/// `hf-mirror.com` mirror (useful in regions where HuggingFace is blocked).
///
/// Requires `reqwest` with the `blocking` feature.
fn download_model_files(target_dir: &Path, model: &LocalEmbeddingModel) -> Result<(), CoreError> {
    use std::time::Duration;

    std::fs::create_dir_all(target_dir)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .user_agent("ask-myself/1.0")
        .build()
        .map_err(|e| CoreError::Embedding(format!("create HTTP client: {e}")))?;

    let files = [
        (model.model_url(), model.fallback_model_url(), "model.onnx"),
        (
            model.tokenizer_url(),
            model.fallback_tokenizer_url(),
            "tokenizer.json",
        ),
    ];

    for (primary_url, mirror_url, filename) in &files {
        let dest = target_dir.join(filename);
        if dest.exists() {
            tracing::info!("{filename} already exists, skipping download");
            continue;
        }

        let response = match download_single(&client, primary_url, filename) {
            Ok(resp) => resp,
            Err(primary_err) => {
                tracing::warn!(
                    "Primary download failed for {filename}: {primary_err}, trying mirror..."
                );
                download_single(&client, mirror_url, filename).map_err(|mirror_err| {
                    CoreError::Embedding(format!(
                        "download {filename} failed — primary: {primary_err}; mirror: {mirror_err}"
                    ))
                })?
            }
        };

        let bytes = response
            .bytes()
            .map_err(|e| CoreError::Embedding(format!("read {filename}: {e}")))?;

        std::fs::write(&dest, &bytes)?;
        tracing::info!("Downloaded {filename} ({} bytes)", bytes.len());
    }

    Ok(())
}

/// Download model files with progress reporting.
fn download_model_files_with_progress(
    target_dir: &Path,
    model: &LocalEmbeddingModel,
    on_progress: impl Fn(DownloadProgress),
) -> Result<(), CoreError> {
    use std::io::Read;
    use std::time::Duration;

    std::fs::create_dir_all(target_dir)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .connect_timeout(Duration::from_secs(30))
        .user_agent("ask-myself/1.0")
        .build()
        .map_err(|e| CoreError::Embedding(format!("create HTTP client: {e}")))?;

    let files = [
        (model.model_url(), model.fallback_model_url(), "model.onnx"),
        (
            model.tokenizer_url(),
            model.fallback_tokenizer_url(),
            "tokenizer.json",
        ),
    ];

    let total_files = files.len();

    for (file_index, (primary_url, mirror_url, filename)) in files.iter().enumerate() {
        let dest = target_dir.join(filename);
        if dest.exists() {
            tracing::info!("{filename} already exists, skipping download");
            continue;
        }

        let response = match download_single(&client, primary_url, filename) {
            Ok(resp) => resp,
            Err(primary_err) => {
                tracing::warn!(
                    "Primary download failed for {filename}: {primary_err}, trying mirror..."
                );
                download_single(&client, mirror_url, filename).map_err(|mirror_err| {
                    CoreError::Embedding(format!(
                        "download {filename} failed — primary: {primary_err}; mirror: {mirror_err}"
                    ))
                })?
            }
        };

        let total_bytes = response.content_length();
        let mut bytes_downloaded: u64 = 0;
        let mut reader = response;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 65536];

        on_progress(DownloadProgress {
            filename: filename.to_string(),
            bytes_downloaded: 0,
            total_bytes,
            file_index,
            total_files,
        });

        loop {
            let n = reader
                .read(&mut chunk)
                .map_err(|e| CoreError::Embedding(format!("read {filename}: {e}")))?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            bytes_downloaded += n as u64;
            on_progress(DownloadProgress {
                filename: filename.to_string(),
                bytes_downloaded,
                total_bytes,
                file_index,
                total_files,
            });
        }

        std::fs::write(&dest, &buf)?;
        tracing::info!("Downloaded {filename} ({} bytes)", buf.len());
    }

    Ok(())
}

/// Try to download a single file, returning the response on success.
fn download_single(
    client: &reqwest::blocking::Client,
    url: &str,
    filename: &str,
) -> Result<reqwest::blocking::Response, String> {
    tracing::info!("Downloading {filename} from {url}");
    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("network error: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    Ok(response)
}

// ── API Embedder (OpenAI-compatible) ─────────────────────────────────

/// Maximum number of texts per API request to avoid payload limits.
const API_BATCH_SIZE: usize = 100;

/// Request body for the OpenAI-compatible embeddings endpoint.
#[derive(Serialize)]
struct ApiEmbeddingRequest<'a> {
    input: &'a [&'a str],
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

/// A single embedding entry in the API response.
#[derive(Deserialize)]
struct ApiEmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

/// Top-level API response from the embeddings endpoint.
#[derive(Deserialize)]
struct ApiEmbeddingResponse {
    data: Vec<ApiEmbeddingData>,
}

/// Error body returned by the API on non-2xx responses.
#[derive(Deserialize)]
struct ApiErrorResponse {
    error: Option<ApiErrorDetail>,
}

#[derive(Deserialize)]
struct ApiErrorDetail {
    message: Option<String>,
}

/// Embedding client for OpenAI-compatible embedding APIs.
///
/// Supports any service that follows the `/v1/embeddings` JSON contract
/// (OpenAI, Azure OpenAI, Ollama, LM Studio, etc.).
pub struct ApiEmbedder {
    client: reqwest::blocking::Client,
    api_key: String,
    base_url: String,
    model: String,
    dimensions: Option<usize>,
}

impl ApiEmbedder {
    /// Create a new API embedder.
    ///
    /// - `api_key` — Bearer token (must not be empty).
    /// - `base_url` — API root, e.g. `https://api.openai.com/v1`. Defaults
    ///   to OpenAI if `None`.
    /// - `model` — Model identifier. Defaults to `text-embedding-3-small`.
    /// - `dimensions` — Optional dimension override (OpenAI supports this
    ///   for `text-embedding-3-*` models).
    pub fn new(
        api_key: String,
        base_url: Option<String>,
        model: Option<String>,
        dimensions: Option<usize>,
    ) -> Result<Self, CoreError> {
        if api_key.trim().is_empty() {
            return Err(CoreError::Embedding("API key is required".into()));
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| CoreError::Embedding(format!("HTTP client init: {e}")))?;

        Ok(Self {
            client,
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
            model: model.unwrap_or_else(|| "text-embedding-3-small".into()),
            dimensions,
        })
    }

    /// Send a single batch (≤ `API_BATCH_SIZE`) to the API and return
    /// embeddings sorted by the original input order.
    fn call_api(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        let url = format!("{}/embeddings", self.base_url.trim_end_matches('/'));

        let body = ApiEmbeddingRequest {
            input: texts,
            model: &self.model,
            dimensions: self.dimensions,
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| CoreError::Embedding(format!("API request failed: {e}")))?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            tracing::warn!("Embedding API rate-limited (429). Consider reducing batch frequency.");
        }

        if !status.is_success() {
            let err_text = response.text().unwrap_or_default();
            let detail = serde_json::from_str::<ApiErrorResponse>(&err_text)
                .ok()
                .and_then(|r| r.error)
                .and_then(|e| e.message)
                .unwrap_or_else(|| err_text);
            return Err(CoreError::Embedding(format!(
                "API returned HTTP {status}: {detail}"
            )));
        }

        let resp: ApiEmbeddingResponse = response
            .json()
            .map_err(|e| CoreError::Embedding(format!("API response parse: {e}")))?;

        // Sort by index to guarantee input order.
        let mut data = resp.data;
        data.sort_by_key(|d| d.index);

        Ok(data.into_iter().map(|d| d.embedding).collect())
    }
}

impl ApiEmbedder {
    /// Maximum number of retry attempts for transient API errors.
    const MAX_RETRIES: u32 = 3;

    /// Send a batch with retry + exponential backoff for transient errors.
    fn call_api_with_retry(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        let mut attempt = 0u32;
        loop {
            match self.call_api(texts) {
                Ok(result) => return Ok(result),
                Err(e) if attempt < Self::MAX_RETRIES && Self::is_retryable(&e) => {
                    attempt += 1;
                    let wait = std::time::Duration::from_millis(100 * 2u64.pow(attempt)); // 200ms, 400ms, 800ms
                    tracing::warn!("Embed API retry {}/{}: {}", attempt, Self::MAX_RETRIES, e);
                    std::thread::sleep(wait);
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Check if an error is transient and worth retrying.
    fn is_retryable(e: &CoreError) -> bool {
        let msg = e.to_string();
        msg.contains("429")
            || msg.contains("500")
            || msg.contains("502")
            || msg.contains("503")
            || msg.contains("timeout")
            || msg.contains("connection")
    }
}

impl Embedder for ApiEmbedder {
    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions.unwrap_or(1536)
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, CoreError> {
        let results = self.embed_batch(&[text])?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| CoreError::Embedding("empty response from API".into()))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        if texts.is_empty() {
            return Ok(vec![]);
        }

        // Split into chunks of API_BATCH_SIZE to avoid payload limits,
        // with retry + exponential backoff on each sub-batch.
        let mut all_embeddings = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(API_BATCH_SIZE) {
            let mut batch_result = self.call_api_with_retry(chunk)?;
            all_embeddings.append(&mut batch_result);
        }

        Ok(all_embeddings)
    }
}

/// Test connectivity to an OpenAI-compatible embedding API.
///
/// Sends a single short text and returns `Ok(true)` if the API responds
/// with a valid embedding. Any error is propagated as `CoreError`.
pub fn test_api_connection(api_key: &str, base_url: &str) -> Result<bool, CoreError> {
    let embedder = ApiEmbedder::new(api_key.to_string(), Some(base_url.to_string()), None, None)?;
    let result = embedder.embed("connection test")?;
    Ok(!result.is_empty())
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── tokenization ────────────────────────────────────────────────

    #[test]
    fn test_tokenize_basic_english() {
        let tokens = tokenize("Hello, World! This is a test.");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"test".to_string()));
        // Stop words removed.
        assert!(!tokens.contains(&"this".to_string()));
        assert!(!tokens.contains(&"is".to_string()));
        assert!(!tokens.contains(&"a".to_string()));
    }

    #[test]
    fn test_tokenize_removes_short_tokens() {
        let tokens = tokenize("I am a go-to x y z person");
        // "i", "x", "y", "z" are single chars → removed.
        assert!(!tokens.contains(&"i".to_string()));
        assert!(!tokens.contains(&"x".to_string()));
        // "am" is a stop word? No, but it's 2 chars and not a stop word — kept.
        assert!(tokens.contains(&"go".to_string()));
        assert!(tokens.contains(&"person".to_string()));
    }

    #[test]
    fn test_tokenize_cjk_characters() {
        let tokens = tokenize("Hello 你好世界 test");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"你".to_string()));
        assert!(tokens.contains(&"好".to_string()));
        assert!(tokens.contains(&"世".to_string()));
        assert!(tokens.contains(&"界".to_string()));
        assert!(tokens.contains(&"test".to_string()));
    }

    #[test]
    fn test_tokenize_mixed_punctuation() {
        let tokens = tokenize("email@example.com http://foo.bar/baz?q=1");
        // Should split on @ . : / ? =
        assert!(tokens.contains(&"email".to_string()));
        assert!(tokens.contains(&"example".to_string()));
        assert!(tokens.contains(&"com".to_string()));
        assert!(tokens.contains(&"http".to_string()));
        assert!(tokens.contains(&"foo".to_string()));
        assert!(tokens.contains(&"bar".to_string()));
        assert!(tokens.contains(&"baz".to_string()));
    }

    // ── TF-IDF construction ─────────────────────────────────────────

    #[test]
    fn test_build_from_corpus() {
        let docs = vec![
            "the cat sat on the mat",
            "the dog chased the cat",
            "the bird flew over the mat",
        ];
        let embedder = TfIdfEmbedder::build_from_corpus(&docs);
        assert!(embedder.dimensions > 0);
        assert!(embedder.dimensions <= MAX_DIMENSIONS);
        assert_eq!(embedder.vocabulary.len(), embedder.dimensions);
        assert_eq!(embedder.idf.len(), embedder.dimensions);
    }

    #[test]
    fn test_build_from_empty_corpus() {
        let embedder = TfIdfEmbedder::build_from_corpus(&[]);
        assert_eq!(embedder.dimensions, 0);
    }

    #[test]
    fn test_from_vocabulary_roundtrip() {
        let docs = vec!["rust is fast", "python is easy"];
        let original = TfIdfEmbedder::build_from_corpus(&docs);
        let restored =
            TfIdfEmbedder::from_vocabulary(original.vocabulary.clone(), original.idf.clone());
        assert_eq!(original.dimensions, restored.dimensions);

        let v1 = original.embed("rust is great").unwrap();
        let v2 = restored.embed("rust is great").unwrap();
        assert_eq!(v1, v2);
    }

    // ── embed ───────────────────────────────────────────────────────

    #[test]
    fn test_embed_produces_normalized_vector() {
        let docs = vec![
            "machine learning algorithms",
            "deep learning neural networks",
            "natural language processing",
        ];
        let embedder = TfIdfEmbedder::build_from_corpus(&docs);
        let vec = embedder.embed("machine learning").unwrap();

        // Non-zero vector.
        assert!(vec.iter().any(|&v| v != 0.0));

        // L2 norm ≈ 1.0
        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "expected unit vector, got norm={}",
            norm
        );
    }

    #[test]
    fn test_embed_empty_text() {
        let embedder = TfIdfEmbedder::build_from_corpus(&["hello world"]);
        let vec = embedder.embed("").unwrap();
        assert!(vec.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_embed_batch() {
        let docs = vec!["alpha beta", "gamma delta"];
        let embedder = TfIdfEmbedder::build_from_corpus(&docs);
        let results = embedder.embed_batch(&["alpha", "gamma"]).unwrap();
        assert_eq!(results.len(), 2);
        for v in &results {
            assert_eq!(v.len(), embedder.dimensions);
        }
    }

    // ── cosine similarity ───────────────────────────────────────────

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![1.0, 2.0];
        let zero = vec![0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &zero), 0.0);
        assert_eq!(cosine_similarity(&zero, &a), 0.0);
    }

    #[test]
    fn test_cosine_similar_documents() {
        // Larger corpus so IDF values distinguish topics properly.
        let docs = vec![
            "rust compiler systems performance memory safety",
            "cpp compiler systems performance memory management",
            "python scripting dynamic typing interpreted language",
            "javascript scripting dynamic typing web browser",
            "cooking recipes food kitchen ingredients preparation",
            "baking pastry dessert oven flour sugar",
        ];
        let embedder = TfIdfEmbedder::build_from_corpus(&docs);

        let v_rust = embedder.embed("rust compiler systems performance").unwrap();
        let v_cpp = embedder.embed("cpp compiler systems performance").unwrap();
        let v_cooking = embedder.embed("cooking recipes food kitchen").unwrap();

        let sim_related = cosine_similarity(&v_rust, &v_cpp);
        let sim_unrelated = cosine_similarity(&v_rust, &v_cooking);

        assert!(
            sim_related > sim_unrelated,
            "related topics should be more similar: related={} vs unrelated={}",
            sim_related,
            sim_unrelated
        );
    }

    // ── vector serialization ────────────────────────────────────────

    #[test]
    fn test_vector_blob_roundtrip() {
        let original = vec![1.0f32, -2.5, 3.14, 0.0, f32::MAX, f32::MIN];
        let blob = vector_to_blob(&original);
        assert_eq!(blob.len(), original.len() * 4);
        let restored = blob_to_vector(&blob);
        assert_eq!(original, restored);
    }

    #[test]
    fn test_vector_blob_empty() {
        let empty: Vec<f32> = vec![];
        let blob = vector_to_blob(&empty);
        assert!(blob.is_empty());
        let restored = blob_to_vector(&blob);
        assert!(restored.is_empty());
    }

    // ── DB operations ───────────────────────────────────────────────

    fn setup_db_with_chunk() -> (Database, String, String) {
        let db = Database::open_memory().unwrap();
        let (source_id, doc_id, chunk_id) = {
            let conn = db.conn();
            let source_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO sources (id, kind, root_path) VALUES (?1, 'local_folder', '/tmp/test')",
                rusqlite::params![&source_id],
            )
            .unwrap();

            let doc_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO documents (id, source_id, path, title, mime_type, file_size, modified_at, content_hash)
                 VALUES (?1, ?2, '/tmp/test.md', 'Test', 'text/plain', 100, datetime('now'), 'hash')",
                rusqlite::params![&doc_id, &source_id],
            )
            .unwrap();

            let chunk_id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, start_offset, end_offset, line_start, line_end, content_hash)
                 VALUES (?1, ?2, 0, 'text', 'hello world', 0, 11, 1, 1, 'chash')",
                rusqlite::params![&chunk_id, &doc_id],
            )
            .unwrap();

            (source_id, doc_id, chunk_id)
        };
        let _ = source_id;
        (db, doc_id, chunk_id)
    }

    #[test]
    fn test_store_and_get_embedding() {
        let (db, _doc_id, chunk_id) = setup_db_with_chunk();
        let vector = vec![0.1, 0.2, 0.3, 0.4];

        db.store_embedding(&chunk_id, "tfidf-v1", &vector).unwrap();
        let result = db.get_embedding(&chunk_id, "tfidf-v1").unwrap();

        assert!(result.is_some());
        assert_eq!(result.unwrap(), vector);
    }

    #[test]
    fn test_get_embedding_not_found() {
        let (db, _doc_id, _chunk_id) = setup_db_with_chunk();
        let result = db.get_embedding("nonexistent", "tfidf-v1").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_store_embedding_upsert() {
        let (db, _doc_id, chunk_id) = setup_db_with_chunk();

        db.store_embedding(&chunk_id, "tfidf-v1", &[1.0, 2.0])
            .unwrap();
        db.store_embedding(&chunk_id, "tfidf-v1", &[3.0, 4.0])
            .unwrap();

        let result = db.get_embedding(&chunk_id, "tfidf-v1").unwrap().unwrap();
        assert_eq!(result, vec![3.0, 4.0]);
    }

    #[test]
    fn test_get_all_embeddings() {
        let (db, _doc_id, chunk_id) = setup_db_with_chunk();
        db.store_embedding(&chunk_id, "tfidf-v1", &[1.0, 2.0])
            .unwrap();

        let all = db.get_all_embeddings("tfidf-v1").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, chunk_id);
        assert_eq!(all[0].1, vec![1.0, 2.0]);
    }

    #[test]
    fn test_delete_embeddings_for_document() {
        let (db, doc_id, chunk_id) = setup_db_with_chunk();
        db.store_embedding(&chunk_id, "tfidf-v1", &[1.0]).unwrap();

        db.delete_embeddings_for_document(&doc_id).unwrap();

        let result = db.get_embedding(&chunk_id, "tfidf-v1").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_save_and_load_embedder_state() {
        let db = Database::open_memory().unwrap();

        let mut vocab = HashMap::new();
        vocab.insert("hello".to_string(), 0);
        vocab.insert("world".to_string(), 1);
        let idf = vec![1.5, 0.8];

        db.save_embedder_state("tfidf-v1", &vocab, &idf).unwrap();
        let loaded = db.load_embedder_state("tfidf-v1").unwrap();

        assert!(loaded.is_some());
        let (loaded_vocab, loaded_idf) = loaded.unwrap();
        assert_eq!(loaded_vocab, vocab);
        assert_eq!(loaded_idf, idf);
    }

    #[test]
    fn test_load_embedder_state_not_found() {
        let db = Database::open_memory().unwrap();
        let result = db.load_embedder_state("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_save_embedder_state_upsert() {
        let db = Database::open_memory().unwrap();

        let mut vocab1 = HashMap::new();
        vocab1.insert("old".to_string(), 0);
        db.save_embedder_state("tfidf-v1", &vocab1, &[1.0]).unwrap();

        let mut vocab2 = HashMap::new();
        vocab2.insert("new".to_string(), 0);
        db.save_embedder_state("tfidf-v1", &vocab2, &[2.0]).unwrap();

        let (loaded_vocab, loaded_idf) = db.load_embedder_state("tfidf-v1").unwrap().unwrap();
        assert_eq!(loaded_vocab, vocab2);
        assert_eq!(loaded_idf, vec![2.0]);
    }

    #[test]
    fn test_batch_store_embeddings() {
        let (db, doc_id, chunk_id) = setup_db_with_chunk();

        // Add a second chunk.
        let chunk_id_2 = {
            let conn = db.conn();
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chunks (id, document_id, chunk_index, kind, content, \
                 start_offset, end_offset, line_start, line_end, content_hash) \
                 VALUES (?1, ?2, 1, 'text', 'second chunk content', 0, 20, 1, 1, 'chash2')",
                rusqlite::params![&id, &doc_id],
            )
            .unwrap();
            id
        };

        let batch = vec![
            (
                chunk_id.clone(),
                "tfidf-v1".to_string(),
                vec![0.1, 0.2, 0.3],
            ),
            (
                chunk_id_2.clone(),
                "tfidf-v1".to_string(),
                vec![0.4, 0.5, 0.6],
            ),
        ];

        db.batch_store_embeddings(&batch).unwrap();

        let v1 = db.get_embedding(&chunk_id, "tfidf-v1").unwrap().unwrap();
        assert_eq!(v1, vec![0.1, 0.2, 0.3]);

        let v2 = db.get_embedding(&chunk_id_2, "tfidf-v1").unwrap().unwrap();
        assert_eq!(v2, vec![0.4, 0.5, 0.6]);
    }

    #[test]
    fn test_get_embeddings_batched() {
        let (db, _doc_id, chunk_id) = setup_db_with_chunk();

        db.store_embedding(&chunk_id, "tfidf-v1", &[1.0, 2.0, 3.0])
            .unwrap();

        let batch = db.get_embeddings_batched("tfidf-v1", 10, 0).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].0, chunk_id);
        assert_eq!(batch[0].1, vec![1.0, 2.0, 3.0]);

        // Offset past all results returns empty.
        let empty = db.get_embeddings_batched("tfidf-v1", 10, 100).unwrap();
        assert!(empty.is_empty());
    }

    // ── ONNX embedder ──────────────────────────────────────────────

    #[test]
    #[ignore] // requires model download (~46 MB)
    fn test_onnx_embed_dimensions() {
        let model = LocalEmbeddingModel::default();
        let embedder = OnnxEmbedder::new(None, model.clone()).unwrap();
        let vec = embedder.embed("hello world").unwrap();
        assert_eq!(vec.len(), model.dimensions());

        let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "expected unit vector, got norm={norm}"
        );
    }

    #[test]
    #[ignore] // requires model download (~46 MB)
    fn test_onnx_cosine_similarity() {
        let embedder = OnnxEmbedder::new(None, LocalEmbeddingModel::default()).unwrap();
        let v1 = embedder.embed("the cat sat on the mat").unwrap();
        let v2 = embedder.embed("a cat was sitting on a mat").unwrap();
        let v3 = embedder
            .embed("quantum physics and the theory of relativity")
            .unwrap();

        let sim_similar = cosine_similarity(&v1, &v2);
        let sim_different = cosine_similarity(&v1, &v3);

        assert!(
            sim_similar > sim_different,
            "similar texts should score higher: similar={sim_similar} vs different={sim_different}"
        );
    }

    #[test]
    #[ignore] // requires model download (~46 MB)
    fn test_onnx_embed_batch() {
        let embedder = OnnxEmbedder::new(None, LocalEmbeddingModel::default()).unwrap();
        let vecs = embedder
            .embed_batch(&["hello world", "goodbye world", "rust programming"])
            .unwrap();
        assert_eq!(vecs.len(), 3);
        for v in &vecs {
            assert_eq!(v.len(), LocalEmbeddingModel::default().dimensions());
        }
    }
}
