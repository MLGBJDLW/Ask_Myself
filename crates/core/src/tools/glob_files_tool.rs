//! GlobFilesTool - source-scoped path globbing for local files.

#[cfg(test)]
use crate::db::Database;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::json;

use crate::error::CoreError;

use super::path_utils::{resolve_path_for_file_access, PathKind};
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/glob_files.json");

const DEFAULT_MAX_RESULTS: usize = 100;
const MAX_RESULTS: usize = 500;

#[derive(Deserialize)]
struct GlobFilesArgs {
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    patterns: Vec<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    include_dirs: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

#[derive(Debug)]
struct GlobRoot {
    root: PathBuf,
    display_base: PathBuf,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GlobMatch {
    path: String,
    entry_type: &'static str,
}

pub struct GlobFilesTool;

#[async_trait]
impl Tool for GlobFilesTool {
    fn name(&self) -> &str {
        "glob_files"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&DEF, DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&DEF, DEF_JSON).parameters.clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::FileSystem]
    }

    async fn execute(
        &self,
        context: crate::tools::ToolExecutionContext<'_>,
    ) -> Result<ToolResult, CoreError> {
        let crate::tools::ToolExecutionContext {
            call_id,
            arguments,
            db,
            source_scope,
            ..
        } = context;
        let args: GlobFilesArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid glob_files arguments: {e}")))?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let patterns = normalize_patterns(&args)?;
            let matcher = build_globset(&patterns)?;
            let max_results = args
                .max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .min(MAX_RESULTS);
            let file_policy = file_access_policy(&db, &source_scope)?;
            let roots = resolve_glob_roots(&args, &file_policy)?;
            if roots.is_empty() {
                return Ok(ToolResult {
                    call_id,
                    content: "No source directories are available to glob.".to_string(),
                    is_error: true,
                    artifacts: None,
                });
            }

            let mut matches = Vec::new();
            'roots: for root in roots {
                let mut builder = ignore::WalkBuilder::new(&root.root);
                builder
                    .follow_links(false)
                    .hidden(!args.include_hidden)
                    .git_ignore(true)
                    .git_exclude(true)
                    .git_global(true)
                    .require_git(false)
                    .parents(true);

                for entry in builder.build() {
                    let Ok(entry) = entry else {
                        continue;
                    };
                    let Some(file_type) = entry.file_type() else {
                        continue;
                    };
                    if !(file_type.is_file() || (args.include_dirs && file_type.is_dir())) {
                        continue;
                    }
                    let rel = entry
                        .path()
                        .strip_prefix(&root.display_base)
                        .unwrap_or(entry.path());
                    let rel_str = normalize_path_for_glob(rel);
                    if rel_str.is_empty() || !matcher.is_match(rel_str.as_str()) {
                        continue;
                    }
                    matches.push(GlobMatch {
                        path: rel_str,
                        entry_type: if file_type.is_dir() { "dir" } else { "file" },
                    });
                    if matches.len() >= max_results {
                        break 'roots;
                    }
                }
            }

            let truncated = matches.len() >= max_results;
            let mut text = format!(
                "Found {} path(s) matching {}.",
                matches.len(),
                patterns.join(", ")
            );
            if truncated {
                text.push_str(&format!(" Results truncated at {max_results}."));
            }
            text.push('\n');
            for item in &matches {
                let marker = if item.entry_type == "dir" { "/" } else { "" };
                text.push_str(&format!("\n{}{}", item.path, marker));
            }

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(json!({
                    "kind": "fileGlobResults",
                    "patterns": patterns,
                    "truncated": truncated,
                    "matches": matches,
                })),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

fn normalize_patterns(args: &GlobFilesArgs) -> Result<Vec<String>, CoreError> {
    let mut patterns = if args.patterns.is_empty() {
        args.pattern.clone().into_iter().collect::<Vec<_>>()
    } else {
        args.patterns.clone()
    };
    patterns = patterns
        .into_iter()
        .map(|pattern| pattern.trim().to_string())
        .filter(|pattern| !pattern.is_empty())
        .collect();
    if patterns.is_empty() {
        return Err(CoreError::InvalidInput(
            "glob_files requires pattern or patterns.".to_string(),
        ));
    }
    if patterns.len() > 20 {
        return Err(CoreError::InvalidInput(
            "glob_files supports at most 20 patterns.".to_string(),
        ));
    }
    Ok(patterns)
}

fn resolve_glob_roots(
    args: &GlobFilesArgs,
    file_policy: &super::FileAccessPolicy,
) -> Result<Vec<GlobRoot>, CoreError> {
    if let Some(path) = args.path.as_deref() {
        let requested = Path::new(path);
        if file_policy.sources.is_empty()
            && !(file_policy.allow_unregistered_absolute_paths && requested.is_absolute())
        {
            return Err(CoreError::InvalidInput(format!(
                "Access denied: '{path}' is not within any directory available in the current source scope."
            )));
        }
        let resolved = resolve_path_for_file_access(
            requested,
            &file_policy.sources,
            PathKind::Directory,
            false,
            file_policy.allow_unregistered_absolute_paths,
        )
        .map_err(CoreError::InvalidInput)?;
        return Ok(vec![GlobRoot {
            root: resolved.clone(),
            display_base: resolved,
        }]);
    }

    let mut roots = Vec::new();
    for source in &file_policy.sources {
        if let Ok(root) = std::fs::canonicalize(&source.root_path) {
            roots.push(GlobRoot {
                root: root.clone(),
                display_base: root,
            });
        }
    }
    Ok(roots)
}

fn build_globset(patterns: &[String]) -> Result<GlobSet, CoreError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            Glob::new(pattern)
                .map_err(|e| CoreError::InvalidInput(format!("Invalid glob '{pattern}': {e}")))?,
        );
        if !pattern.contains('/') && !pattern.contains('\\') {
            let nested = format!("**/{pattern}");
            builder.add(
                Glob::new(&nested).map_err(|e| {
                    CoreError::InvalidInput(format!("Invalid glob '{nested}': {e}"))
                })?,
            );
        }
    }
    builder
        .build()
        .map_err(|e| CoreError::InvalidInput(format!("Invalid glob pattern: {e}")))
}

fn normalize_path_for_glob(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::CreateSourceInput;

    fn setup_db_with_source(root: &Path) -> Database {
        let db = Database::open_memory().expect("open in-memory db");
        db.add_source(CreateSourceInput {
            root_path: root.to_string_lossy().to_string(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .expect("register source root");
        db
    }

    #[tokio::test]
    async fn glob_files_finds_matching_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notes")).unwrap();
        std::fs::write(dir.path().join("notes").join("today.md"), "# Today\n").unwrap();
        std::fs::write(dir.path().join("notes").join("today.txt"), "Today\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = GlobFilesTool;
        let args = serde_json::json!({ "pattern": "*.md" });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "glob-1",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("notes/today.md"));
        assert!(!result.content.contains("notes/today.txt"));
    }

    #[tokio::test]
    async fn glob_files_respects_gitignore_by_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.md\n").unwrap();
        std::fs::write(dir.path().join("kept.md"), "# kept\n").unwrap();
        std::fs::write(dir.path().join("ignored.md"), "# ignored\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = GlobFilesTool;
        let args = serde_json::json!({ "pattern": "*.md" });

        let result = tool
            .execute(crate::tools::ToolExecutionContext::new(
                "glob-2",
                &args.to_string(),
                &db,
                &[],
            ))
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("kept.md"));
        assert!(!result.content.contains("ignored.md"));
    }
}
