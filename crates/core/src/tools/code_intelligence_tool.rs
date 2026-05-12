//! CodeIntelligenceTool - lightweight source-scoped code symbol/reference lookup.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::json;

use crate::db::Database;
use crate::error::CoreError;

use super::path_utils::{resolve_path_for_file_access, PathKind};
use super::{
    file_access_policy, scope_is_active, Tool, ToolCategory, ToolDef, ToolOutput, ToolResult,
    TrustBoundary,
};

static DEF: OnceLock<ToolDef> = OnceLock::new();
const DEF_JSON: &str = include_str!("../../prompts/tools/code_intelligence.json");

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_MAX_RESULTS: usize = 80;
const MAX_RESULTS: usize = 300;
const MAX_DISPLAY_CHARS: usize = 240;
const MAX_SCANNED_FILES: usize = 15_000;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CodeIntelligenceAction {
    Symbols,
    References,
}

#[derive(Debug, Deserialize)]
struct CodeIntelligenceArgs {
    action: CodeIntelligenceAction,
    query: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    case_sensitive: bool,
    #[serde(default = "default_whole_word")]
    whole_word: bool,
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
struct CodeMatch {
    path: String,
    line_number: usize,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    preview: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CodeIntelligenceData {
    kind: &'static str,
    action: &'static str,
    query: String,
    path: Option<String>,
    truncated: bool,
    searched_files: usize,
    skipped_files: usize,
    matches: Vec<CodeMatch>,
}

struct SymbolPattern {
    kind_family: &'static str,
    regex: Regex,
}

pub struct CodeIntelligenceTool;

#[async_trait]
impl Tool for CodeIntelligenceTool {
    fn name(&self) -> &str {
        "code_intelligence"
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

    fn is_read_only(&self, _args: &serde_json::Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _args: &serde_json::Value) -> bool {
        true
    }

    async fn execute(
        &self,
        call_id: &str,
        arguments: &str,
        db: &Database,
        source_scope: &[String],
    ) -> Result<ToolResult, CoreError> {
        let args: CodeIntelligenceArgs = serde_json::from_str(arguments).map_err(|e| {
            CoreError::InvalidInput(format!("Invalid code_intelligence arguments: {e}"))
        })?;

        let query = args.query.trim().to_string();
        if query.is_empty() {
            return Err(CoreError::InvalidInput(
                "code_intelligence query must not be empty.".to_string(),
            ));
        }

        let db = db.clone();
        let call_id = call_id.to_string();
        let source_scope = source_scope.to_vec();
        tokio::task::spawn_blocking(move || {
            let max_results = args
                .max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .clamp(1, MAX_RESULTS);
            let file_policy = file_access_policy(&db, &source_scope)?;
            let roots = resolve_search_roots(args.path.as_deref(), &file_policy)?;
            if roots.is_empty() {
                return Ok(ToolResult::from_output(
                    call_id,
                    true,
                    ToolOutput::text("No source directories are available for code intelligence."),
                ));
            }

            let mut matches = Vec::new();
            let mut searched_files = 0usize;
            let mut skipped_files = 0usize;

            'roots: for root in roots {
                if root.root.is_file() {
                    if should_scan_file(&root.root) {
                        searched_files += 1;
                        scan_file(
                            &root.root,
                            &root.display_base,
                            &args,
                            &query,
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
                    .parents(true)
                    .filter_entry(|entry| !is_heavy_directory(entry.path()));

                for entry in builder.build() {
                    if searched_files >= MAX_SCANNED_FILES {
                        break 'roots;
                    }
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
                    if !should_scan_file(path) {
                        continue;
                    }
                    searched_files += 1;
                    scan_file(
                        path,
                        &root.display_base,
                        &args,
                        &query,
                        max_results,
                        &mut matches,
                    )?;
                    if matches.len() >= max_results {
                        break 'roots;
                    }
                }
            }

            let truncated = matches.len() >= max_results || searched_files >= MAX_SCANNED_FILES;
            let action_label = match args.action {
                CodeIntelligenceAction::Symbols => "symbols",
                CodeIntelligenceAction::References => "references",
            };
            let data = CodeIntelligenceData {
                kind: "codeIntelligenceResults",
                action: action_label,
                query: query.clone(),
                path: args.path.clone(),
                truncated,
                searched_files,
                skipped_files,
                matches,
            };

            let llm_content = format_code_intelligence_summary(&data, true);
            let display_content = format_code_intelligence_summary(&data, false);
            let output = ToolOutput {
                llm_content,
                display_content,
                data: Some(serde_json::to_value(&data)?),
                artifacts: Some(json!({
                    "trustBoundary": TrustBoundary::local_source_evidence(scope_is_active(&source_scope)),
                    "limits": {
                        "maxResults": max_results,
                        "maxFileBytes": MAX_FILE_BYTES,
                        "maxScannedFiles": MAX_SCANNED_FILES
                    }
                })),
                attachments: Vec::new(),
            };

            Ok(ToolResult::from_output(call_id, false, output))
        })
        .await
        .map_err(|e| CoreError::Internal(format!("task join failed: {e}")))?
    }
}

fn default_whole_word() -> bool {
    true
}

fn resolve_search_roots(
    path: Option<&str>,
    file_policy: &super::FileAccessPolicy,
) -> Result<Vec<SearchRoot>, CoreError> {
    if let Some(path) = path {
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

fn scan_file(
    path: &Path,
    base: &Path,
    args: &CodeIntelligenceArgs,
    query: &str,
    max_results: usize,
    matches: &mut Vec<CodeMatch>,
) -> Result<(), CoreError> {
    if matches.len() >= max_results {
        return Ok(());
    }

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

    match args.action {
        CodeIntelligenceAction::Symbols => {
            scan_symbols(path, base, &content, args, query, max_results, matches)
        }
        CodeIntelligenceAction::References => {
            scan_references(path, base, &content, args, query, max_results, matches)
        }
    }
}

fn scan_symbols(
    path: &Path,
    base: &Path,
    content: &str,
    args: &CodeIntelligenceArgs,
    query: &str,
    max_results: usize,
    matches: &mut Vec<CodeMatch>,
) -> Result<(), CoreError> {
    let display_path = normalize_path_for_display(path.strip_prefix(base).unwrap_or(path));
    for pattern in symbol_patterns() {
        for captures in pattern.regex.captures_iter(content) {
            if matches.len() >= max_results {
                return Ok(());
            }
            let Some(name_match) = captures.name("name") else {
                continue;
            };
            let name = name_match.as_str();
            if !symbol_name_matches(name, query, args.case_sensitive) {
                continue;
            }
            let Some(match_span) = captures.get(0) else {
                continue;
            };
            let kind = captures
                .name("kind")
                .map(|kind| kind.as_str())
                .unwrap_or(pattern.kind_family);
            let line_number = line_number_at(content, match_span.start());
            matches.push(CodeMatch {
                path: display_path.clone(),
                line_number,
                kind: kind.to_string(),
                name: Some(name.to_string()),
                preview: preview_line_at(content, match_span.start()),
            });
        }
    }
    Ok(())
}

fn scan_references(
    path: &Path,
    base: &Path,
    content: &str,
    args: &CodeIntelligenceArgs,
    query: &str,
    max_results: usize,
    matches: &mut Vec<CodeMatch>,
) -> Result<(), CoreError> {
    let matcher = reference_matcher(query, args.case_sensitive, args.whole_word)?;
    let display_path = normalize_path_for_display(path.strip_prefix(base).unwrap_or(path));
    for (idx, line) in content.lines().enumerate() {
        if matches.len() >= max_results {
            break;
        }
        if matcher.is_match(line) {
            matches.push(CodeMatch {
                path: display_path.clone(),
                line_number: idx + 1,
                kind: "reference".to_string(),
                name: None,
                preview: truncate_line(line.trim()),
            });
        }
    }
    Ok(())
}

fn symbol_patterns() -> &'static [SymbolPattern] {
    static PATTERNS: OnceLock<Vec<SymbolPattern>> = OnceLock::new();
    PATTERNS
        .get_or_init(|| {
            vec![
                SymbolPattern {
                    kind_family: "rust",
                    regex: Regex::new(
                        r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?P<kind>fn|struct|enum|trait|type)\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)",
                    )
                    .expect("valid rust symbol regex"),
                },
                SymbolPattern {
                    kind_family: "typescript",
                    regex: Regex::new(
                        r"(?m)^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(?P<kind>function|class|interface|type|const|let|var)\s+(?P<name>[A-Za-z_$][A-Za-z0-9_$]*)",
                    )
                    .expect("valid typescript symbol regex"),
                },
                SymbolPattern {
                    kind_family: "python",
                    regex: Regex::new(
                        r"(?m)^\s*(?:async\s+)?(?P<kind>def|class)\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)",
                    )
                    .expect("valid python symbol regex"),
                },
                SymbolPattern {
                    kind_family: "go",
                    regex: Regex::new(
                        r"(?m)^\s*(?P<kind>func|type)\s+(?:\([^)]+\)\s*)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)",
                    )
                    .expect("valid go symbol regex"),
                },
                SymbolPattern {
                    kind_family: "jvm_dotnet",
                    regex: Regex::new(
                        r"(?m)^\s*(?:(?:public|private|protected|internal|static|final|abstract|sealed|partial)\s+)*(?P<kind>class|interface|enum|record)\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)",
                    )
                    .expect("valid jvm/dotnet symbol regex"),
                },
            ]
        })
        .as_slice()
}

fn reference_matcher(
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
) -> Result<Regex, CoreError> {
    let pattern = if whole_word && is_identifier_like(query) {
        format!(r"\b{}\b", regex::escape(query))
    } else {
        regex::escape(query)
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| CoreError::InvalidInput(format!("Invalid reference matcher: {e}")))
}

fn symbol_name_matches(name: &str, query: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        name.contains(query)
    } else {
        name.to_lowercase().contains(&query.to_lowercase())
    }
}

fn should_scan_file(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "c"
            | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hpp"
            | "cs"
            | "rb"
            | "php"
            | "sql"
            | "html"
            | "css"
            | "scss"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "md"
            | "txt"
    )
}

fn is_heavy_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".nuxt"
            | ".venv"
            | "venv"
            | "__pycache__"
    )
}

fn is_identifier_like(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn line_number_at(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn preview_line_at(content: &str, byte_offset: usize) -> String {
    let start = content[..byte_offset]
        .rfind('\n')
        .map(|idx| idx + 1)
        .unwrap_or(0);
    let end = content[byte_offset..]
        .find('\n')
        .map(|idx| byte_offset + idx)
        .unwrap_or(content.len());
    truncate_line(content[start..end].trim())
}

fn truncate_line(line: &str) -> String {
    let mut out = line.chars().take(MAX_DISPLAY_CHARS).collect::<String>();
    if line.chars().count() > MAX_DISPLAY_CHARS {
        out.push_str("...");
    }
    out
}

fn normalize_path_for_display(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn format_code_intelligence_summary(data: &CodeIntelligenceData, compact: bool) -> String {
    let mut text = format!(
        "Found {} {} match(es) for {:?} across {} searched file(s).",
        data.matches.len(),
        data.action,
        data.query,
        data.searched_files
    );
    if data.truncated {
        text.push_str(" Results were truncated.");
    }
    if data.skipped_files > 0 {
        text.push_str(&format!(
            " Skipped {} unreadable path(s).",
            data.skipped_files
        ));
    }
    text.push('\n');

    let display_limit = if compact { 30 } else { data.matches.len() };
    for item in data.matches.iter().take(display_limit) {
        match &item.name {
            Some(name) => text.push_str(&format!(
                "\n{}:{} [{} {}] {}",
                item.path, item.line_number, item.kind, name, item.preview
            )),
            None => text.push_str(&format!(
                "\n{}:{} [{}] {}",
                item.path, item.line_number, item.kind, item.preview
            )),
        }
    }
    if compact && data.matches.len() > display_limit {
        text.push_str(&format!(
            "\n... {} more match(es) available in artifacts.data.matches.",
            data.matches.len() - display_limit
        ));
    }
    text
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
    async fn finds_symbols_across_source_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("lib.rs"),
            "pub struct AgentRuntime {}\nimpl AgentRuntime {}\npub async fn run_agent() {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("ui.ts"),
            "export interface AgentCard {}\nexport function renderAgent() {}\n",
        )
        .unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = CodeIntelligenceTool;
        let args = json!({
            "action": "symbols",
            "query": "Agent",
            "case_sensitive": true,
            "max_results": 10
        });

        let result = tool
            .execute("code-1", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        assert!(result.llm_context_content().contains("AgentRuntime"));
        assert!(result.llm_context_content().contains("AgentCard"));
        let data = &result.artifacts.as_ref().unwrap()["data"];
        assert_eq!(data["kind"], "codeIntelligenceResults");
        assert_eq!(data["matches"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn finds_whole_word_references_without_substring_noise() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("main.ts"),
            "const agent = createAgent();\nconst agentic = false;\nagent.run();\n",
        )
        .unwrap();

        let db = setup_db_with_source(dir.path());
        let tool = CodeIntelligenceTool;
        let args = json!({
            "action": "references",
            "query": "agent",
            "max_results": 10
        });

        let result = tool
            .execute("code-2", &args.to_string(), &db, &[])
            .await
            .unwrap();

        assert!(!result.is_error, "unexpected error: {}", result.content);
        let data = &result.artifacts.as_ref().unwrap()["data"];
        let matches = data["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);
        assert!(result.llm_context_content().contains("main.ts:1"));
        assert!(result.llm_context_content().contains("main.ts:3"));
        assert!(!result.llm_context_content().contains("agentic"));
    }
}
