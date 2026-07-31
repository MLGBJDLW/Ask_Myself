//! Controller-owned isolated Git worktree for Code Ultra writes.

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::ToolCallRequest;
use crate::sources::CreateSourceInput;

const FILESYSTEM_TOOLS: &[&str] = &[
    "code_intelligence",
    "create_file",
    "edit_file",
    "glob_files",
    "list_dir",
    "multi_edit",
    "read_file",
    "read_files",
    "run_shell",
    "search_files",
];

const MUTATION_TOOLS: &[&str] = &["create_file", "edit_file", "multi_edit", "run_shell"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IsolationPromotion {
    pub(super) changed: bool,
    pub(super) detail: String,
}

/// A temporary worktree that owns every filesystem mutation for one Code
/// Ultra turn. The verified patch is promoted to the original clean worktree
/// only once all other Workflow IR gates have passed.
pub(super) struct WorkspaceIsolationRuntime {
    db: Database,
    original_repo_root: PathBuf,
    original_source_root: PathBuf,
    isolated_worktree_root: PathBuf,
    isolated_source_root: PathBuf,
    isolated_source_id: Option<String>,
    saw_mutation_tool: bool,
    finalized: bool,
}

impl WorkspaceIsolationRuntime {
    pub(super) fn prepare(db: &Database, source_scope: &[String]) -> Result<Self, CoreError> {
        let candidates = if source_scope.is_empty() {
            db.list_sources()?
        } else {
            source_scope
                .iter()
                .map(|id| db.get_source(id))
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut roots = candidates
            .into_iter()
            .map(|source| PathBuf::from(source.root_path))
            .filter(|root| root.is_dir())
            .collect::<Vec<_>>();
        roots.sort();
        roots.dedup();
        if roots.len() != 1 {
            return Err(CoreError::InvalidInput(format!(
                "Code Ultra write isolation requires exactly one local source root; found {}. Link one clean Git repository or choose another profile.",
                roots.len()
            )));
        }

        let original_source_root = std::fs::canonicalize(&roots[0])?;
        let repo_output = run_git(&original_source_root, &["rev-parse", "--show-toplevel"])?;
        let original_repo_root = canonicalize_git_path(&repo_output.stdout)?;
        if !original_source_root.starts_with(&original_repo_root) {
            return Err(CoreError::InvalidInput(
                "The selected source root is not inside its resolved Git repository.".to_string(),
            ));
        }
        let status = run_git(
            &original_repo_root,
            &["status", "--porcelain", "--untracked-files=normal"],
        )?;
        if !status.stdout.is_empty() {
            return Err(CoreError::InvalidInput(
                "Code Ultra requires a clean source worktree before creating an isolated patch workspace. Commit or otherwise preserve existing changes first."
                    .to_string(),
            ));
        }

        let isolation_base = std::env::temp_dir().join("nexa-code-ultra");
        std::fs::create_dir_all(&isolation_base)?;
        let isolated_worktree_root = isolation_base.join(Uuid::new_v4().to_string());
        let worktree_arg = isolated_worktree_root.to_string_lossy().to_string();
        run_git(
            &original_repo_root,
            &["worktree", "add", "--detach", &worktree_arg, "HEAD"],
        )?;

        let relative_source_root = original_source_root
            .strip_prefix(&original_repo_root)
            .map_err(|error| CoreError::Internal(error.to_string()))?;
        let isolated_source_root = isolated_worktree_root.join(relative_source_root);
        let source = match db.add_source(CreateSourceInput {
            root_path: isolated_source_root.to_string_lossy().to_string(),
            include_globs: vec!["**/*".to_string()],
            exclude_globs: vec![".git/**".to_string()],
            watch_enabled: false,
        }) {
            Ok(source) => source,
            Err(error) => {
                let _ = remove_worktree(&original_repo_root, &isolated_worktree_root);
                return Err(error);
            }
        };

        Ok(Self {
            db: db.clone(),
            original_repo_root,
            original_source_root,
            isolated_worktree_root,
            isolated_source_root,
            isolated_source_id: Some(source.id),
            saw_mutation_tool: false,
            finalized: false,
        })
    }

    pub(super) fn source_id(&self) -> Option<&str> {
        self.isolated_source_id.as_deref()
    }

    pub(super) fn prompt_section(&self) -> String {
        format!(
            "## Controller-enforced write isolation\n\nCode Ultra created an isolated Git worktree at `{}`. Every filesystem path and shell cwd is controller-routed into this worktree. Use `run_shell` instead of `project_tool` for repository commands. Do not target the original source root. The controller will promote the verified patch only after all other required gates pass.",
            self.isolated_source_root.display()
        )
    }

    pub(super) fn rewrite_tool_calls(
        &mut self,
        tool_calls: &mut [ToolCallRequest],
    ) -> Result<(), CoreError> {
        if self.finalized
            && tool_calls
                .iter()
                .any(|call| FILESYSTEM_TOOLS.contains(&call.name.as_str()))
        {
            return Err(CoreError::InvalidInput(
                "Code Ultra already promoted and closed its isolated patch workspace; no further filesystem calls are allowed in this turn."
                    .to_string(),
            ));
        }
        for call in tool_calls {
            if call.name == "project_tool" {
                let arguments: Value = serde_json::from_str(&call.arguments)?;
                if arguments.get("action").and_then(Value::as_str) == Some("run") {
                    return Err(CoreError::InvalidInput(
                        "Code Ultra blocks project_tool run because its manifest executes in the original source. Use run_shell in the controller-provided isolated worktree."
                            .to_string(),
                    ));
                }
                continue;
            }
            if !FILESYSTEM_TOOLS.contains(&call.name.as_str()) {
                continue;
            }

            let mut arguments: Value = serde_json::from_str(&call.arguments)?;
            if call.name == "run_shell" {
                let object = arguments.as_object_mut().ok_or_else(|| {
                    CoreError::InvalidInput("run_shell arguments must be an object".to_string())
                })?;
                let cwd = object
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_default();
                object.insert(
                    "cwd".to_string(),
                    Value::String(self.route_path(&cwd)?.to_string_lossy().to_string()),
                );
            } else {
                self.rewrite_path_fields(&mut arguments, None)?;
            }
            call.arguments = serde_json::to_string(&arguments)?;
            if MUTATION_TOOLS.contains(&call.name.as_str()) {
                self.saw_mutation_tool = true;
            }
        }
        Ok(())
    }

    pub(super) fn promote_verified_patch(&mut self) -> Result<IsolationPromotion, CoreError> {
        if self.finalized {
            return Ok(IsolationPromotion {
                changed: self.saw_mutation_tool,
                detail: "Controller already promoted and cleaned the isolated patch workspace."
                    .to_string(),
            });
        }

        run_git(&self.isolated_worktree_root, &["add", "-N", "--", "."])?;
        let patch = run_git(
            &self.isolated_worktree_root,
            &["diff", "--binary", "--no-ext-diff", "HEAD", "--"],
        )?
        .stdout;
        let changed = !patch.is_empty();
        if changed {
            run_git_with_input(
                &self.original_repo_root,
                &["apply", "--check", "--whitespace=nowarn", "-"],
                &patch,
            )?;
            run_git_with_input(
                &self.original_repo_root,
                &["apply", "--whitespace=nowarn", "-"],
                &patch,
            )?;
        }
        self.cleanup()?;
        self.finalized = true;

        Ok(IsolationPromotion {
            changed,
            detail: if changed {
                "Controller promoted the verified Git patch from the isolated worktree into the original clean source and removed the temporary source."
                    .to_string()
            } else {
                "Controller verified that the isolated worktree produced no source patch and removed the temporary source."
                    .to_string()
            },
        })
    }

    fn rewrite_path_fields(
        &self,
        value: &mut Value,
        parent_key: Option<&str>,
    ) -> Result<(), CoreError> {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    self.rewrite_path_fields(child, Some(key))?;
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.rewrite_path_fields(item, parent_key)?;
                }
            }
            Value::String(path)
                if matches!(
                    parent_key,
                    Some("path" | "paths" | "cwd" | "directory" | "root")
                ) =>
            {
                *path = self.route_path(path)?.to_string_lossy().to_string();
            }
            _ => {}
        }
        Ok(())
    }

    fn route_path(&self, raw: &str) -> Result<PathBuf, CoreError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "." {
            return Ok(self.isolated_source_root.clone());
        }
        let requested = Path::new(trimmed);
        let relative = if requested.is_absolute() {
            requested
                .strip_prefix(&self.isolated_source_root)
                .or_else(|_| requested.strip_prefix(&self.original_source_root))
                .map_err(|_| {
                    CoreError::InvalidInput(format!(
                        "Code Ultra rejected path '{}' because it is outside the isolated source.",
                        requested.display()
                    ))
                })?
        } else {
            requested
        };
        let mut normalized = PathBuf::new();
        for component in relative.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => normalized.push(part),
                Component::ParentDir if normalized.pop() => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(CoreError::InvalidInput(format!(
                        "Code Ultra rejected path '{}' because it escapes the isolated source.",
                        requested.display()
                    )));
                }
            }
        }
        Ok(self.isolated_source_root.join(normalized))
    }

    fn cleanup(&mut self) -> Result<(), CoreError> {
        if let Some(source_id) = self.isolated_source_id.take() {
            self.db.delete_source(&source_id)?;
        }
        remove_worktree(&self.original_repo_root, &self.isolated_worktree_root)
    }
}

impl Drop for WorkspaceIsolationRuntime {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        if let Some(source_id) = self.isolated_source_id.take() {
            let _ = self.db.delete_source(&source_id);
        }
        let _ = remove_worktree(&self.original_repo_root, &self.isolated_worktree_root);
    }
}

fn canonicalize_git_path(stdout: &[u8]) -> Result<PathBuf, CoreError> {
    let path = String::from_utf8_lossy(stdout).trim().to_string();
    if path.is_empty() {
        return Err(CoreError::InvalidInput(
            "Git did not return a repository root for Code Ultra isolation.".to_string(),
        ));
    }
    std::fs::canonicalize(path).map_err(CoreError::Io)
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<Output, CoreError> {
    let output = Command::new("git").arg("-C").arg(cwd).args(args).output()?;
    ensure_git_success(output, args)
}

fn run_git_with_input(cwd: &Path, args: &[&str], input: &[u8]) -> Result<Output, CoreError> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| CoreError::Internal("git apply stdin was unavailable".to_string()))?
        .write_all(input)?;
    let output = child.wait_with_output()?;
    ensure_git_success(output, args)
}

fn ensure_git_success(output: Output, args: &[&str]) -> Result<Output, CoreError> {
    if output.status.success() {
        return Ok(output);
    }
    Err(CoreError::Agent(format!(
        "Code Ultra isolation command `git {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn remove_worktree(repo_root: &Path, worktree_root: &Path) -> Result<(), CoreError> {
    let base = std::env::temp_dir().join("nexa-code-ultra");
    if !worktree_root.starts_with(&base) {
        return Err(CoreError::Internal(
            "Refused to remove an isolation worktree outside the managed temp root.".to_string(),
        ));
    }
    if !worktree_root.exists() {
        return Ok(());
    }
    run_git(
        repo_root,
        &[
            "worktree",
            "remove",
            "--force",
            &worktree_root.to_string_lossy(),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(cwd: &Path, args: &[&str]) {
        run_git(cwd, args).expect("git fixture command");
    }

    #[test]
    fn isolated_patch_is_routed_verified_and_promoted() {
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init"]);
        git(repo.path(), &["config", "user.email", "nexa@example.test"]);
        git(repo.path(), &["config", "user.name", "Nexa Test"]);
        std::fs::write(repo.path().join("tracked.txt"), "before\n").unwrap();
        git(repo.path(), &["add", "tracked.txt"]);
        git(repo.path(), &["commit", "-m", "fixture"]);

        let db = Database::open_memory().unwrap();
        let source = db
            .add_source(CreateSourceInput {
                root_path: repo.path().to_string_lossy().to_string(),
                include_globs: vec!["**/*".to_string()],
                exclude_globs: Vec::new(),
                watch_enabled: false,
            })
            .unwrap();
        let mut isolation =
            WorkspaceIsolationRuntime::prepare(&db, std::slice::from_ref(&source.id)).unwrap();
        let isolated_source_id = isolation.source_id().unwrap().to_string();
        assert!(isolation.route_path("../escape.txt").is_err());
        assert!(isolation
            .route_path(std::env::temp_dir().to_string_lossy().as_ref())
            .is_err());
        assert_eq!(
            isolation
                .route_path(repo.path().join("tracked.txt").to_string_lossy().as_ref())
                .unwrap(),
            isolation.isolated_source_root.join("tracked.txt")
        );
        let mut calls = vec![ToolCallRequest {
            id: "edit-1".to_string(),
            name: "edit_file".to_string(),
            arguments: r#"{"path":"tracked.txt"}"#.to_string(),
            thought_signature: None,
        }];
        isolation.rewrite_tool_calls(&mut calls).unwrap();
        let routed: Value = serde_json::from_str(&calls[0].arguments).unwrap();
        let routed_path = PathBuf::from(routed["path"].as_str().unwrap());
        assert!(routed_path.starts_with(&isolation.isolated_source_root));
        std::fs::write(&routed_path, "after\n").unwrap();

        let promotion = isolation.promote_verified_patch().unwrap();
        assert!(promotion.changed);
        assert_eq!(
            std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "after\n"
        );
        assert!(db.get_source(&isolated_source_id).is_err());
    }
}
