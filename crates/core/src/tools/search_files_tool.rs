//! SearchFilesTool - source-scoped text search for local files.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;

use super::path_utils::{resolve_path_for_file_access, PathKind};
use super::{file_access_policy, Tool, ToolCategory, ToolDef, ToolResult};

static DEF: OnceLock<ToolDef> = OnceLock::new();
static GREP_DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/search_files.json");
const GREP_DEF_JSON: &str = include_str!("../../prompts/tools/grep_files.json");

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_MAX_RESULTS: usize = 50;
const MAX_RESULTS: usize = 200;
const MAX_CONTEXT_LINES: usize = 3;
const MAX_DISPLAY_CHARS: usize = 300;

#[derive(Deserialize)]
struct SearchFilesArgs {
    query: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    regex: bool,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default)]
    include_globs: Vec<String>,
    #[serde(default)]
    exclude_globs: Vec<String>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    context_lines: Option<usize>,
    #[serde(default)]
    include_hidden: bool,
}

#[derive(Debug)]
struct SearchRoot {
    root: PathBuf,
    display_base: PathBuf,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ContextLine {
    line_number: usize,
    text: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FileMatch {
    path: String,
    line_number: usize,
    line: String,
    before: Vec<ContextLine>,
    after: Vec<ContextLine>,
}

enum Matcher {
    Literal { query: String, case_sensitive: bool },
    Regex(regex::Regex),
}

impl Matcher {
    fn new(query: &str, regex: bool, case_sensitive: bool) -> Result<Self, String> {
        if query.trim().is_empty() {
            return Err("search_files query must not be empty.".to_string());
        }

        if regex {
            RegexBuilder::new(query)
                .case_insensitive(!case_sensitive)
                .build()
                .map(Self::Regex)
                .map_err(|e| format!("Invalid regex: {e}"))
        } else {
            Ok(Self::Literal {
                query: query.to_string(),
                case_sensitive,
            })
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Literal {
                query,
                case_sensitive,
            } => {
                if *case_sensitive {
                    line.contains(query)
                } else {
                    line.to_lowercase().contains(&query.to_lowercase())
                }
            }
            Self::Regex(regex) => regex.is_match(line),
        }
    }
}

pub struct SearchFilesTool;
pub struct GrepFilesTool;

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
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
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: SearchFilesArgs = serde_json::from_str(arguments)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid search_files arguments: {e}")))?;

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let matcher = Matcher::new(&args.query, args.regex, args.case_sensitive)
                .map_err(CoreError::InvalidInput)?;
            let max_results = args
                .max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .min(MAX_RESULTS);
            let context_lines = args.context_lines.unwrap_or(0).min(MAX_CONTEXT_LINES);
            let include_set = build_globset(&args.include_globs)?;
            let exclude_set = build_globset(&args.exclude_globs)?;

            let file_policy = file_access_policy(&db, &source_scope)?;
            let roots = resolve_search_roots(&args, &file_policy)?;
            if roots.is_empty() {
                return Ok(ToolResult {
                    call_id,
                    content: "No source directories are available to search.".to_string(),
                    is_error: true,
                    artifacts: None,
                });
            }

            let mut matches = Vec::new();
            let mut searched_files = 0usize;
            let mut skipped_files = 0usize;

            'roots: for root in roots {
                if root.root.is_file() {
                    if file_included(&root.root, &root.display_base, &include_set, &exclude_set) {
                        searched_files += 1;
                        search_file(
                            &root.root,
                            &root.display_base,
                            &matcher,
                            context_lines,
                            max_results,
                            &mut matches,
                        )?;
                    }
                    if matches.len() >= max_results {
                        break;
                    }
                    continue;
                }

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
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => {
                            skipped_files += 1;
                            continue;
                        }
                    };
                    if !entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_file())
                    {
                        continue;
                    }
                    let path = entry.path();
                    if !file_included(path, &root.display_base, &include_set, &exclude_set) {
                        continue;
                    }
                    searched_files += 1;
                    search_file(
                        path,
                        &root.display_base,
                        &matcher,
                        context_lines,
                        max_results,
                        &mut matches,
                    )?;
                    if matches.len() >= max_results {
                        break 'roots;
                    }
                }
            }

            let truncated = matches.len() >= max_results;
            let mut text = format!(
                "Found {} matching line(s) for {:?} across {} searched file(s).",
                matches.len(),
                args.query,
                searched_files
            );
            if truncated {
                text.push_str(&format!(" Results truncated at {max_results}."));
            }
            if skipped_files > 0 {
                text.push_str(&format!(" Skipped {skipped_files} unreadable path(s)."));
            }
            text.push('\n');

            for item in &matches {
                text.push_str(&format!(
                    "\n{}:{}: {}",
                    item.path, item.line_number, item.line
                ));
            }

            Ok(ToolResult {
                call_id,
                content: text,
                is_error: false,
                artifacts: Some(json!({
                    "kind": "fileSearchResults",
                    "query": args.query,
                    "regex": args.regex,
                    "caseSensitive": args.case_sensitive,
                    "truncated": truncated,
                    "searchedFiles": searched_files,
                    "matches": matches,
                })),
            })
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

#[async_trait]
impl Tool for GrepFilesTool {
    fn name(&self) -> &str {
        "grep_files"
    }

    fn description(&self) -> &str {
        &ToolDef::from_json(&GREP_DEF, GREP_DEF_JSON).description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        ToolDef::from_json(&GREP_DEF, GREP_DEF_JSON)
            .parameters
            .clone()
    }

    fn categories(&self) -> &'static [ToolCategory] {
        &[ToolCategory::FileSystem]
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        SearchFilesTool
            .execute(call_id, arguments, db, source_scope)
            .await
    }
}

fn resolve_search_roots(
    args: &SearchFilesArgs,
    file_policy: &super::FileAccessPolicy,
) -> Result<Vec<SearchRoot>, CoreError> {
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
            PathKind::Any,
            false,
            file_policy.allow_unregistered_absolute_paths,
        )
        .map_err(CoreError::InvalidInput)?;
        let display_base = if resolved.is_file() {
            resolved
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| resolved.clone())
        } else {
            resolved.clone()
        };
        return Ok(vec![SearchRoot {
            root: resolved,
            display_base,
        }]);
    }

    let mut roots = Vec::new();
    for source in &file_policy.sources {
        if let Ok(root) = std::fs::canonicalize(&source.root_path) {
            roots.push(SearchRoot {
                root: root.clone(),
                display_base: root,
            });
        }
    }
    Ok(roots)
}

fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>, CoreError> {
    if patterns.is_empty() {
        return Ok(None);
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        add_glob_pattern(&mut builder, pattern)?;
    }
    builder
        .build()
        .map(Some)
        .map_err(|e| CoreError::InvalidInput(format!("Invalid glob pattern: {e}")))
}

fn add_glob_pattern(builder: &mut GlobSetBuilder, pattern: &str) -> Result<(), CoreError> {
    builder.add(
        Glob::new(pattern)
            .map_err(|e| CoreError::InvalidInput(format!("Invalid glob '{pattern}': {e}")))?,
    );
    if !pattern.contains('/') && !pattern.contains('\\') {
        let nested = format!("**/{pattern}");
        builder.add(
            Glob::new(&nested)
                .map_err(|e| CoreError::InvalidInput(format!("Invalid glob '{nested}': {e}")))?,
        );
    }
    Ok(())
}

fn file_included(
    path: &Path,
    base: &Path,
    include_set: &Option<GlobSet>,
    exclude_set: &Option<GlobSet>,
) -> bool {
    let rel = path.strip_prefix(base).unwrap_or(path);
    let rel = normalize_path_for_glob(rel);
    if exclude_set
        .as_ref()
        .is_some_and(|set| set.is_match(rel.as_str()))
    {
        return false;
    }
    include_set
        .as_ref()
        .map(|set| set.is_match(rel.as_str()))
        .unwrap_or(true)
}

fn normalize_path_for_glob(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn search_file(
    path: &Path,
    base: &Path,
    matcher: &Matcher,
    context_lines: usize,
    max_results: usize,
    matches: &mut Vec<FileMatch>,
) -> Result<(), CoreError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };
    if metadata.len() > MAX_FILE_BYTES {
        return Ok(());
    }

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(()),
    };
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Ok(());
    }
    let content = match String::from_utf8(bytes) {
        Ok(content) => content,
        Err(_) => return Ok(()),
    };

    let lines: Vec<&str> = content.lines().collect();
    let display_path = normalize_path_for_glob(path.strip_prefix(base).unwrap_or(path));

    for (idx, line) in lines.iter().enumerate() {
        if matches.len() >= max_results {
            break;
        }
        if !matcher.is_match(line) {
            continue;
        }

        let before_start = idx.saturating_sub(context_lines);
        let before = lines[before_start..idx]
            .iter()
            .enumerate()
            .map(|(offset, text)| ContextLine {
                line_number: before_start + offset + 1,
                text: truncate_line(text),
            })
            .collect();
        let after_end = (idx + 1 + context_lines).min(lines.len());
        let after = lines[idx + 1..after_end]
            .iter()
            .enumerate()
            .map(|(offset, text)| ContextLine {
                line_number: idx + offset + 2,
                text: truncate_line(text),
            })
            .collect();

        matches.push(FileMatch {
            path: display_path.clone(),
            line_number: idx + 1,
            line: truncate_line(line),
            before,
            after,
        });
    }

    Ok(())
}

fn truncate_line(line: &str) -> String {
    let mut out = line.chars().take(MAX_DISPLAY_CHARS).collect::<String>();
    if line.chars().count() > MAX_DISPLAY_CHARS {
        out.push_str("...");
    }
    out
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
    async fn search_files_finds_literal_matches_with_glob_filter() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.md"), "Alpha plan\nBeta plan\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "Alpha scratch\n").unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = SearchFilesTool;
        let args = serde_json::json!({
            "query": "alpha",
            "include_globs": ["*.md"],
            "max_results": 10
        });

        let result = tool
            .execute("search-1", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.content.contains("notes.md:1"));
        assert!(!result.content.contains("notes.txt"));
        assert_eq!(
            result.artifacts.as_ref().unwrap()["matches"][0]["path"],
            "notes.md"
        );
    }

    #[tokio::test]
    async fn search_files_supports_regex_and_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("journal.md"),
            "before\nTicket-123 resolved\nafter\n",
        )
        .unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = SearchFilesTool;
        let args = serde_json::json!({
            "query": "ticket-\\d+",
            "regex": true,
            "context_lines": 1
        });

        let result = tool
            .execute("search-2", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let artifact = result.artifacts.as_ref().unwrap();
        assert_eq!(artifact["matches"][0]["lineNumber"], 2);
        assert_eq!(artifact["matches"][0]["before"][0]["text"], "before");
        assert_eq!(artifact["matches"][0]["after"][0]["text"], "after");
    }
}
