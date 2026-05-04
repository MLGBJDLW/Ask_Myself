//! Source file-tree browsing for UI previews.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{SecondsFormat, Utc};
use globset::{Glob, GlobSet, GlobSetBuilder};
use rusqlite::params;
use serde::Serialize;

use crate::db::Database;
use crate::error::CoreError;
use crate::models::Source;

const MAX_TREE_DEPTH: usize = 4;
const MAX_TREE_LIMIT: usize = 1_000;
const DEFAULT_TREE_LIMIT: usize = 300;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTree {
    pub source_id: String,
    pub root_path: String,
    pub relative_path: String,
    pub nodes: Vec<SourceTreeNode>,
    pub total_entries: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceTreeNode {
    pub name: String,
    pub path: String,
    pub relative_path: String,
    pub kind: SourceTreeNodeKind,
    pub extension: Option<String>,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<String>,
    pub indexed: bool,
    pub document_id: Option<String>,
    pub chunk_count: Option<usize>,
    pub children: Option<Vec<SourceTreeNode>>,
    pub children_truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceTreeNodeKind {
    Directory,
    File,
}

#[derive(Debug, Clone)]
struct IndexedDocument {
    id: String,
    chunk_count: usize,
}

pub fn list_source_tree(
    db: &Database,
    source_id: &str,
    relative_path: Option<&str>,
    depth: Option<usize>,
    limit: Option<usize>,
) -> Result<SourceTree, CoreError> {
    let source = db.get_source(source_id)?;
    let root = std::fs::canonicalize(Path::new(&source.root_path)).map_err(|e| {
        CoreError::InvalidInput(format!(
            "Cannot resolve source root '{}': {e}",
            source.root_path
        ))
    })?;
    if !root.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "Source root is not a directory: {}",
            source.root_path
        )));
    }

    let rel = normalize_relative_path(relative_path.unwrap_or(""))?;
    let target = if rel.is_empty() {
        root.clone()
    } else {
        std::fs::canonicalize(root.join(Path::new(&rel))).map_err(|e| {
            CoreError::InvalidInput(format!("Cannot resolve source subfolder '{rel}': {e}"))
        })?
    };
    if !target.starts_with(&root) || !target.is_dir() {
        return Err(CoreError::InvalidInput(format!(
            "Path '{rel}' is not a directory inside the source"
        )));
    }

    let include_set = build_globset(&source.include_globs)?;
    let exclude_set = build_globset(&source.exclude_globs)?;
    let indexed = indexed_documents(db, &source, &root)?;
    let depth = depth.unwrap_or(1).min(MAX_TREE_DEPTH);
    let limit = limit.unwrap_or(DEFAULT_TREE_LIMIT).clamp(1, MAX_TREE_LIMIT);
    let mut seen = 0usize;
    let mut truncated = false;
    let nodes = list_nodes(
        &root,
        &target,
        depth,
        limit,
        &mut seen,
        &mut truncated,
        !source.include_globs.is_empty(),
        &include_set,
        &exclude_set,
        &indexed,
    )?;

    Ok(SourceTree {
        source_id: source.id,
        root_path: source.root_path,
        relative_path: rel,
        nodes,
        total_entries: seen,
        truncated,
    })
}

fn normalize_relative_path(raw: &str) -> Result<String, CoreError> {
    let trimmed = raw.trim().trim_matches(&['/', '\\'][..]);
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains('\0') {
        return Err(CoreError::InvalidInput(
            "Tree path must not contain null bytes".to_string(),
        ));
    }
    let path = Path::new(trimmed);
    if path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(CoreError::InvalidInput(
            "Tree path must be source-relative and must not contain '..'".to_string(),
        ));
    }
    Ok(trimmed.replace('\\', "/"))
}

fn build_globset(patterns: &[String]) -> Result<GlobSet, CoreError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid glob pattern '{pattern}': {e}"))
        })?);
    }
    builder.build().map_err(|e| {
        CoreError::InvalidInput(format!("Invalid glob configuration for source tree: {e}"))
    })
}

fn indexed_documents(
    db: &Database,
    source: &Source,
    root: &Path,
) -> Result<HashMap<String, IndexedDocument>, CoreError> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT d.path, d.id, COUNT(c.id)
         FROM documents d
         LEFT JOIN chunks c ON c.document_id = d.id
         WHERE d.source_id = ?1
         GROUP BY d.id, d.path",
    )?;
    let rows = stmt.query_map(params![&source.id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (doc_path, id, chunk_count) = row?;
        let doc_path_buf = PathBuf::from(&doc_path);
        let key = if doc_path_buf.is_absolute() {
            doc_path_buf
                .strip_prefix(root)
                .map(|p| normalize_path_for_tree(p))
                .unwrap_or_else(|_| normalize_path_string(&doc_path))
        } else {
            normalize_path_string(&doc_path)
        };
        map.insert(
            key,
            IndexedDocument {
                id,
                chunk_count: chunk_count.max(0) as usize,
            },
        );
    }
    Ok(map)
}

#[allow(clippy::too_many_arguments)]
fn list_nodes(
    root: &Path,
    dir: &Path,
    depth: usize,
    limit: usize,
    seen: &mut usize,
    truncated: &mut bool,
    has_includes: bool,
    include_set: &GlobSet,
    exclude_set: &GlobSet,
    indexed: &HashMap<String, IndexedDocument>,
) -> Result<Vec<SourceTreeNode>, CoreError> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|a, b| {
        let a_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let b_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
        b_dir
            .cmp(&a_dir)
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });

    let mut nodes = Vec::new();
    for entry in entries {
        if *seen >= limit {
            *truncated = true;
            break;
        }

        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let rel = normalize_path_for_tree(path.strip_prefix(root).map_err(|_| {
            CoreError::InvalidInput(format!(
                "Path escaped source root while listing tree: {}",
                path.display()
            ))
        })?);
        if rel.is_empty() || is_excluded(&rel, exclude_set) {
            continue;
        }

        if file_type.is_file() && has_includes && !include_set.is_match(&rel) {
            continue;
        }

        let metadata = entry.metadata().ok();
        *seen += 1;
        let name = entry.file_name().to_string_lossy().to_string();

        if file_type.is_dir() {
            let mut child_truncated = false;
            let children = if depth > 1 {
                Some(list_nodes(
                    root,
                    &path,
                    depth - 1,
                    limit,
                    seen,
                    &mut child_truncated,
                    has_includes,
                    include_set,
                    exclude_set,
                    indexed,
                )?)
            } else {
                None
            };
            *truncated |= child_truncated;
            nodes.push(SourceTreeNode {
                name,
                path: path.to_string_lossy().to_string(),
                relative_path: rel,
                kind: SourceTreeNodeKind::Directory,
                extension: None,
                size_bytes: None,
                modified_at: metadata.as_ref().and_then(modified_at_iso),
                indexed: false,
                document_id: None,
                chunk_count: None,
                children,
                children_truncated: child_truncated,
            });
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let indexed_doc = indexed.get(&rel);
        nodes.push(SourceTreeNode {
            name,
            path: path.to_string_lossy().to_string(),
            relative_path: rel.clone(),
            kind: SourceTreeNodeKind::File,
            extension: path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase()),
            size_bytes: metadata.as_ref().map(|m| m.len()),
            modified_at: metadata.as_ref().and_then(modified_at_iso),
            indexed: indexed_doc.is_some(),
            document_id: indexed_doc.map(|doc| doc.id.clone()),
            chunk_count: indexed_doc.map(|doc| doc.chunk_count),
            children: None,
            children_truncated: false,
        });
    }

    Ok(nodes)
}

fn is_excluded(relative_path: &str, exclude_set: &GlobSet) -> bool {
    exclude_set.is_match(relative_path) || exclude_set.is_match(format!("{relative_path}/"))
}

fn normalize_path_for_tree(path: &Path) -> String {
    normalize_path_string(&path.to_string_lossy())
}

fn normalize_path_string(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_string()
}

fn modified_at_iso(metadata: &std::fs::Metadata) -> Option<String> {
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(
        chrono::DateTime::<Utc>::from(UNIX_EPOCH + duration)
            .to_rfc3339_opts(SecondsFormat::Secs, true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    #[test]
    fn source_tree_lists_matching_files_and_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes/private")).unwrap();
        std::fs::write(dir.path().join("notes/a.md"), "# A").unwrap();
        std::fs::write(dir.path().join("notes/private/secret.md"), "# Secret").unwrap();
        std::fs::write(dir.path().join("notes/skip.log"), "skip").unwrap();

        let db = Database::open_memory().unwrap();
        let source = db
            .add_source(CreateSourceInput {
                root_path: dir.path().to_string_lossy().to_string(),
                include_globs: vec!["**/*.md".to_string()],
                exclude_globs: vec!["**/private/**".to_string()],
                watch_enabled: false,
            })
            .unwrap();

        let tree = list_source_tree(&db, &source.id, None, Some(3), Some(100)).unwrap();
        let notes = tree
            .nodes
            .iter()
            .find(|node| node.relative_path == "notes")
            .expect("notes dir");
        let children = notes.children.as_ref().expect("children loaded");
        assert!(children
            .iter()
            .any(|node| node.relative_path == "notes/a.md"));
        assert!(!children
            .iter()
            .any(|node| node.relative_path == "notes/skip.log"));
        assert!(!children
            .iter()
            .any(|node| node.relative_path == "notes/private"));
    }

    #[test]
    fn source_tree_rejects_parent_traversal() {
        let err = normalize_relative_path("../outside").unwrap_err();
        assert!(matches!(err, CoreError::InvalidInput(_)));
    }
}
