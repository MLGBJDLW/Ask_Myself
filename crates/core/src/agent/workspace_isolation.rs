//! Controller-owned isolated Git worktree for Code Ultra writes.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use uuid::Uuid;

use crate::db::Database;
use crate::error::CoreError;
use crate::llm::{ToolCallRequest, ToolDefinition};
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

fn is_unscoped_isolation_tool(name: &str) -> bool {
    name == "project_tool" || name == "mcp_tool" || name.starts_with("mcp__")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IsolationPromotion {
    pub(super) changed: bool,
    pub(super) detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct IsolationReview {
    pub(super) changed: bool,
    pub(super) detail: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkspaceIsolationCleanupReport {
    pub removed_worktrees: usize,
    pub removed_sources: usize,
    pub retained_unverifiable_entries: usize,
}

#[derive(Debug)]
struct WorkspaceIsolationOwnership {
    id: String,
    owner_turn_id: Option<String>,
    original_repo_root: PathBuf,
    worktree_root: PathBuf,
    isolated_source_root: PathBuf,
    source_id: Option<String>,
    owner_status: Option<String>,
}

/// Reclaims controller-owned worktrees and temporary Source rows left by a
/// process crash. Only UUID-named direct children of Nexa's managed temp root
/// and Source roots beneath that exact directory are eligible.
pub fn cleanup_orphaned_workspace_isolations(
    db: &Database,
) -> Result<WorkspaceIsolationCleanupReport, CoreError> {
    let base = std::env::temp_dir().join("nexa-code-ultra");
    let mut report = WorkspaceIsolationCleanupReport::default();
    let ownerships = load_workspace_isolation_ownerships(db)?;
    let known_roots = ownerships
        .iter()
        .map(|ownership| ownership.worktree_root.clone())
        .collect::<HashSet<_>>();
    for ownership in ownerships {
        if owner_status_preserves_isolation(ownership.owner_status.as_deref()) {
            report.retained_unverifiable_entries += 1;
            continue;
        }
        let Some(worktree_root) = managed_uuid_worktree_root(&ownership.worktree_root, &base)
        else {
            report.retained_unverifiable_entries += 1;
            continue;
        };
        if worktree_root.exists() {
            let common_dir = match run_git(
                &worktree_root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .and_then(|output| canonicalize_git_path(&output.stdout))
            {
                Ok(path) => path,
                Err(_) => {
                    report.retained_unverifiable_entries += 1;
                    continue;
                }
            };
            let Some(repo_root) = common_dir.parent() else {
                report.retained_unverifiable_entries += 1;
                continue;
            };
            let stored_repo_root = match canonicalize_host_path(&ownership.original_repo_root) {
                Ok(path) => path,
                Err(_) => {
                    report.retained_unverifiable_entries += 1;
                    continue;
                }
            };
            if repo_root != stored_repo_root {
                report.retained_unverifiable_entries += 1;
                continue;
            }
            if remove_worktree(repo_root, &worktree_root).is_err() {
                report.retained_unverifiable_entries += 1;
                continue;
            }
            report.removed_worktrees += 1;
        }

        let mut source_ids = db
            .list_sources()?
            .into_iter()
            .filter(|source| {
                Path::new(&source.root_path) == ownership.isolated_source_root.as_path()
            })
            .map(|source| source.id)
            .collect::<Vec<_>>();
        if let Some(source_id) = ownership.source_id {
            source_ids.push(source_id);
        }
        source_ids.sort();
        source_ids.dedup();
        for source_id in source_ids {
            if db.get_source(&source_id).is_ok() {
                db.delete_source(&source_id)?;
                report.removed_sources += 1;
            }
        }
        delete_workspace_isolation_ownership(db, &ownership.id)?;
    }

    let mut legacy_roots = BTreeMap::<PathBuf, ()>::new();
    for source in db.list_sources()? {
        let source_root = PathBuf::from(source.root_path);
        if let Some(worktree_root) = source_root
            .ancestors()
            .find(|candidate| candidate.parent() == Some(base.as_path()))
            .map(Path::to_path_buf)
            .filter(|root| !known_roots.contains(root))
        {
            legacy_roots.insert(worktree_root, ());
        }
    }
    if base.is_dir() {
        for entry in std::fs::read_dir(&base)? {
            let path = entry?.path();
            if !known_roots.contains(&path) {
                legacy_roots.insert(path, ());
            }
        }
    }
    report.retained_unverifiable_entries += legacy_roots.len();
    Ok(report)
}

fn managed_uuid_worktree_root(path: &Path, base: &Path) -> Option<PathBuf> {
    (path.parent() == Some(base))
        .then(|| path.file_name())
        .flatten()
        .filter(|name| Uuid::parse_str(&name.to_string_lossy()).is_ok())
        .map(|_| path.to_path_buf())
}

fn load_workspace_isolation_ownerships(
    db: &Database,
) -> Result<Vec<WorkspaceIsolationOwnership>, CoreError> {
    let conn = db.conn();
    let mut statement = conn.prepare(
        "SELECT w.id, w.owner_turn_id, w.original_repo_root, w.worktree_root,
                w.isolated_source_root, w.source_id, r.status
         FROM workspace_isolation_ownership w
         LEFT JOIN agent_task_runs r ON r.turn_id = w.owner_turn_id
         ORDER BY w.created_at, w.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(WorkspaceIsolationOwnership {
            id: row.get(0)?,
            owner_turn_id: row.get(1)?,
            original_repo_root: PathBuf::from(row.get::<_, String>(2)?),
            worktree_root: PathBuf::from(row.get::<_, String>(3)?),
            isolated_source_root: PathBuf::from(row.get::<_, String>(4)?),
            source_id: row.get(5)?,
            owner_status: row.get(6)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
}

fn insert_workspace_isolation_intent(
    db: &Database,
    id: &str,
    owner_turn_id: Option<&str>,
    original_repo_root: &Path,
    worktree_root: &Path,
    isolated_source_root: &Path,
) -> Result<(), CoreError> {
    db.conn().execute(
        "INSERT INTO workspace_isolation_ownership
             (id, owner_turn_id, original_repo_root, worktree_root, isolated_source_root)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            id,
            owner_turn_id,
            original_repo_root.to_string_lossy(),
            worktree_root.to_string_lossy(),
            isolated_source_root.to_string_lossy(),
        ],
    )?;
    Ok(())
}

fn activate_workspace_isolation_ownership(
    db: &Database,
    id: &str,
    source_id: &str,
) -> Result<(), CoreError> {
    db.conn().execute(
        "UPDATE workspace_isolation_ownership
         SET source_id = ?2, state = 'active', updated_at = datetime('now')
         WHERE id = ?1",
        rusqlite::params![id, source_id],
    )?;
    Ok(())
}

fn delete_workspace_isolation_ownership(db: &Database, id: &str) -> Result<(), CoreError> {
    db.conn().execute(
        "DELETE FROM workspace_isolation_ownership WHERE id = ?1",
        [id],
    )?;
    Ok(())
}

fn owner_status_preserves_isolation(status: Option<&str>) -> bool {
    status.is_some_and(|status| matches!(status, "paused" | "awaiting_user_input" | "resuming"))
}

/// A temporary worktree that owns every filesystem mutation for one Code
/// Ultra turn. The verified patch is promoted to the original clean worktree
/// only once all other Workflow IR gates have passed.
pub(super) struct WorkspaceIsolationRuntime {
    db: Database,
    isolation_id: String,
    original_repo_root: PathBuf,
    original_source_root: PathBuf,
    isolated_worktree_root: PathBuf,
    isolated_source_root: PathBuf,
    isolated_source_id: Option<String>,
    saw_mutation_tool: bool,
    finalized: bool,
}

impl WorkspaceIsolationRuntime {
    pub(super) fn prepare(
        db: &Database,
        source_scope: &[String],
        owner_turn_id: Option<&str>,
    ) -> Result<Self, CoreError> {
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

        ensure_process_sandbox_available()?;

        let original_source_root = canonicalize_host_path(&roots[0])?;
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

        if let Some(owner_turn_id) = owner_turn_id {
            if let Some(ownership) =
                load_workspace_isolation_ownerships(db)?
                    .into_iter()
                    .find(|ownership| {
                        ownership.owner_turn_id.as_deref() == Some(owner_turn_id)
                            && (owner_status_preserves_isolation(ownership.owner_status.as_deref())
                                || ownership.owner_status.as_deref() == Some("running"))
                    })
            {
                return Self::restore_owned(
                    db,
                    ownership,
                    &original_repo_root,
                    &original_source_root,
                );
            }
        }

        let isolation_base = std::env::temp_dir().join("nexa-code-ultra");
        std::fs::create_dir_all(&isolation_base)?;
        let isolation_id = Uuid::new_v4().to_string();
        let isolated_worktree_root = isolation_base.join(&isolation_id);
        let relative_source_root = original_source_root
            .strip_prefix(&original_repo_root)
            .map_err(|error| CoreError::Internal(error.to_string()))?;
        let isolated_source_root = isolated_worktree_root.join(relative_source_root);
        insert_workspace_isolation_intent(
            db,
            &isolation_id,
            owner_turn_id,
            &original_repo_root,
            &isolated_worktree_root,
            &isolated_source_root,
        )?;
        let worktree_arg = isolated_worktree_root.to_string_lossy().to_string();
        if let Err(error) = run_git(
            &original_repo_root,
            &["worktree", "add", "--detach", &worktree_arg, "HEAD"],
        ) {
            // Keep an ownership row if Git left a partial worktree; otherwise
            // the empty intention can be removed immediately.
            if !isolated_worktree_root.exists() {
                let _ = delete_workspace_isolation_ownership(db, &isolation_id);
            }
            return Err(error);
        }
        let source = match db.add_source(CreateSourceInput {
            root_path: isolated_source_root.to_string_lossy().to_string(),
            include_globs: vec!["**/*".to_string()],
            exclude_globs: vec![".git/**".to_string()],
            watch_enabled: false,
        }) {
            Ok(source) => source,
            Err(error) => {
                if remove_worktree(&original_repo_root, &isolated_worktree_root).is_ok() {
                    let _ = delete_workspace_isolation_ownership(db, &isolation_id);
                }
                return Err(error);
            }
        };
        if let Err(error) = activate_workspace_isolation_ownership(db, &isolation_id, &source.id) {
            if remove_worktree(&original_repo_root, &isolated_worktree_root).is_ok() {
                let _ = db.delete_source(&source.id);
                let _ = delete_workspace_isolation_ownership(db, &isolation_id);
            }
            return Err(error);
        }

        Ok(Self {
            db: db.clone(),
            isolation_id,
            original_repo_root,
            original_source_root,
            isolated_worktree_root,
            isolated_source_root,
            isolated_source_id: Some(source.id),
            saw_mutation_tool: false,
            finalized: false,
        })
    }

    fn restore_owned(
        db: &Database,
        ownership: WorkspaceIsolationOwnership,
        expected_repo_root: &Path,
        expected_source_root: &Path,
    ) -> Result<Self, CoreError> {
        let base = std::env::temp_dir().join("nexa-code-ultra");
        let worktree_root = managed_uuid_worktree_root(&ownership.worktree_root, &base)
            .filter(|path| path.is_dir())
            .ok_or_else(|| {
                CoreError::InvalidInput(
                    "The resumable isolated workspace is missing or outside Nexa's managed root."
                        .to_string(),
                )
            })?;
        let stored_repo_root = canonicalize_host_path(&ownership.original_repo_root)?;
        if stored_repo_root != expected_repo_root {
            return Err(CoreError::InvalidInput(
                "The resumable isolated workspace no longer belongs to the selected repository."
                    .to_string(),
            ));
        }
        let common_dir = run_git(
            &worktree_root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .and_then(|output| canonicalize_git_path(&output.stdout))?;
        if common_dir.parent() != Some(stored_repo_root.as_path()) {
            return Err(CoreError::InvalidInput(
                "The resumable isolated workspace failed Git ownership verification.".to_string(),
            ));
        }
        let relative_source_root = ownership
            .isolated_source_root
            .strip_prefix(&worktree_root)
            .map_err(|_| {
                CoreError::InvalidInput(
                    "The resumable temporary Source escapes its owned worktree.".to_string(),
                )
            })?;
        if stored_repo_root.join(relative_source_root) != expected_source_root {
            return Err(CoreError::InvalidInput(
                "The resumable temporary Source no longer matches the selected Source root."
                    .to_string(),
            ));
        }
        let source_id = ownership.source_id.ok_or_else(|| {
            CoreError::InvalidInput(
                "The resumable isolated workspace has no durable temporary Source binding."
                    .to_string(),
            )
        })?;
        let source = db.get_source(&source_id)?;
        if Path::new(&source.root_path) != ownership.isolated_source_root.as_path() {
            return Err(CoreError::InvalidInput(
                "The resumable temporary Source binding changed after suspension.".to_string(),
            ));
        }

        Ok(Self {
            db: db.clone(),
            isolation_id: ownership.id,
            original_repo_root: stored_repo_root,
            original_source_root: expected_source_root.to_path_buf(),
            isolated_worktree_root: worktree_root,
            isolated_source_root: ownership.isolated_source_root,
            isolated_source_id: Some(source_id),
            saw_mutation_tool: false,
            finalized: false,
        })
    }

    pub(super) fn source_id(&self) -> Option<&str> {
        self.isolated_source_id.as_deref()
    }

    pub(super) fn prompt_section(&self) -> String {
        format!(
            "## Controller-enforced write isolation\n\nCode Ultra created an isolated Git worktree at `{}`. Every filesystem path, shell cwd, and repository path argument is controller-routed into this worktree. Process execution is placed in an OS filesystem sandbox where the host is read-only and only this worktree plus an ephemeral temp directory are writable. `run_shell` requires exact `program` + `args`; free-form `command`, shell interpreters, and inline interpreter code are blocked as defense in depth. Use repository scripts from the isolated source instead of `project_tool`. MCP tools are unavailable because their external processes cannot yet inherit this filesystem sandbox. Do not target the original source root. The controller will promote the verified patch only after all other required gates pass.",
            self.isolated_source_root.display()
        )
    }

    pub(super) fn retain_safe_tool_definitions(tool_defs: &mut Vec<ToolDefinition>) {
        tool_defs.retain(|definition| !is_unscoped_isolation_tool(&definition.name));
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
            if is_unscoped_isolation_tool(&call.name) {
                return Err(CoreError::InvalidInput(format!(
                    "Code Ultra blocks `{}` because it executes outside the controller-owned filesystem sandbox. Use an isolation-safe built-in tool instead.",
                    call.name
                )));
            }
            if !FILESYSTEM_TOOLS.contains(&call.name.as_str()) {
                continue;
            }

            let mut arguments: Value = serde_json::from_str(&call.arguments)?;
            if call.name == "run_shell" {
                let object = arguments.as_object_mut().ok_or_else(|| {
                    CoreError::InvalidInput("run_shell arguments must be an object".to_string())
                })?;
                self.rewrite_shell_invocation(object)?;
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

    fn rewrite_shell_invocation(
        &self,
        object: &mut serde_json::Map<String, Value>,
    ) -> Result<(), CoreError> {
        if object
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| !command.trim().is_empty())
        {
            return Err(CoreError::InvalidInput(
                "Code Ultra write isolation blocks free-form run_shell.command. Use an exact program plus args invocation rooted in the isolated workspace."
                    .to_string(),
            ));
        }
        if object
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || object
                .get("service_action")
                .and_then(Value::as_str)
                .is_some_and(|action| !action.eq_ignore_ascii_case("run"))
        {
            return Err(CoreError::InvalidInput(
                "Code Ultra write isolation does not allow detached or previously managed processes."
                    .to_string(),
            ));
        }

        let program = object.get_mut("program").ok_or_else(|| {
            CoreError::InvalidInput(
                "Code Ultra run_shell requires a program plus args invocation.".to_string(),
            )
        })?;
        let Value::String(program) = program else {
            return Err(CoreError::InvalidInput(
                "Code Ultra run_shell program must be a string.".to_string(),
            ));
        };
        *program = self.rewrite_repository_roots(program);
        let program_path = Path::new(program);
        if program_path.is_absolute()
            && (program_path.starts_with(&self.original_repo_root)
                || program_path.starts_with(&self.original_source_root)
                || program_path.starts_with(&self.isolated_worktree_root)
                || program_path.starts_with(&self.isolated_source_root))
        {
            *program = self
                .route_repository_path(program_path)?
                .to_string_lossy()
                .to_string();
        } else if program.contains('/') || program.contains('\\') {
            *program = self.route_path(program)?.to_string_lossy().to_string();
        }

        let program_name = Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(program)
            .to_ascii_lowercase();
        if matches!(
            program_name.as_str(),
            "bash"
                | "bash.exe"
                | "cmd"
                | "cmd.exe"
                | "powershell"
                | "powershell.exe"
                | "pwsh"
                | "pwsh.exe"
                | "sh"
                | "sh.exe"
        ) {
            return Err(CoreError::InvalidInput(
                "Code Ultra write isolation blocks shell interpreter programs; invoke the required executable directly with args."
                    .to_string(),
            ));
        }

        let args = object
            .entry("args".to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| {
                CoreError::InvalidInput("Code Ultra run_shell args must be an array.".to_string())
            })?;
        for argument in args.iter_mut() {
            let Value::String(value) = argument else {
                return Err(CoreError::InvalidInput(
                    "Code Ultra run_shell args must contain only strings.".to_string(),
                ));
            };
            *value = self.rewrite_shell_argument(value)?;
        }
        let interpreter = matches!(
            program_name.as_str(),
            "node" | "node.exe" | "python" | "python.exe" | "python3" | "python3.exe"
        );
        let interpreter_eval = interpreter
            && args.iter().filter_map(Value::as_str).any(|argument| {
                matches!(
                    argument.to_ascii_lowercase().as_str(),
                    "-c" | "-e" | "--eval"
                )
            });
        let interpreter_stdin = interpreter
            && object
                .get("stdin")
                .and_then(Value::as_str)
                .is_some_and(|stdin| !stdin.is_empty());
        if interpreter_eval || interpreter_stdin {
            return Err(CoreError::InvalidInput(
                "Code Ultra write isolation blocks inline interpreter code; create a script inside the isolated source and execute that file."
                    .to_string(),
            ));
        }
        object.insert(
            "_nexaIsolationSandbox".to_string(),
            serde_json::json!({
                "worktreeRoot": self.isolated_worktree_root.to_string_lossy()
            }),
        );
        Ok(())
    }

    fn rewrite_shell_argument(&self, argument: &str) -> Result<String, CoreError> {
        let rewritten = self.rewrite_repository_roots(argument);
        let candidate = rewritten
            .split_once('=')
            .map(|(_, value)| value)
            .unwrap_or(rewritten.as_str());
        let candidate_path = Path::new(candidate);
        if candidate_path.is_absolute() {
            let routed = self.route_repository_path(candidate_path)?;
            return Ok(rewritten.replacen(candidate, &routed.to_string_lossy(), 1));
        }
        let mut depth = 0usize;
        for component in candidate_path.components() {
            match component {
                Component::Normal(_) => depth = depth.saturating_add(1),
                Component::ParentDir if depth > 0 => depth -= 1,
                Component::ParentDir => {
                    return Err(CoreError::InvalidInput(format!(
                        "Code Ultra rejected shell argument '{argument}' because it escapes the isolated working directory."
                    )));
                }
                _ => {}
            }
        }
        Ok(rewritten)
    }

    fn rewrite_repository_roots(&self, value: &str) -> String {
        let pairs = [
            (&self.original_source_root, &self.isolated_source_root),
            (&self.original_repo_root, &self.isolated_worktree_root),
        ];
        pairs
            .into_iter()
            .fold(value.to_string(), |current, (from, to)| {
                let from_native = from.to_string_lossy();
                let to_native = to.to_string_lossy();
                let replaced = current.replace(from_native.as_ref(), to_native.as_ref());
                replaced.replace(
                    &from_native.replace('\\', "/"),
                    &to_native.replace('\\', "/"),
                )
            })
    }

    fn route_repository_path(&self, requested: &Path) -> Result<PathBuf, CoreError> {
        if let Ok(relative) = requested.strip_prefix(&self.isolated_worktree_root) {
            return self.join_without_escape(&self.isolated_worktree_root, relative, requested);
        }
        if let Ok(relative) = requested.strip_prefix(&self.original_repo_root) {
            return self.join_without_escape(&self.isolated_worktree_root, relative, requested);
        }
        Err(CoreError::InvalidInput(format!(
            "Code Ultra rejected repository path '{}' because it is outside the isolated worktree.",
            requested.display()
        )))
    }

    pub(super) fn promote_verified_patch(&mut self) -> Result<IsolationPromotion, CoreError> {
        if self.finalized {
            return Ok(IsolationPromotion {
                changed: self.saw_mutation_tool,
                detail: "Controller already promoted and cleaned the isolated patch workspace."
                    .to_string(),
            });
        }

        let patch = self.prepare_verified_patch()?;
        let changed = !patch.is_empty();
        if changed {
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

    /// Performs the isolated schedule's independent review without delegation
    /// or nested writers. It verifies patch structure and applicability but
    /// deliberately leaves the original checkout untouched.
    pub(super) fn review_isolated_patch(&self) -> Result<IsolationReview, CoreError> {
        if self.finalized {
            return Err(CoreError::InvalidInput(
                "Cannot review an isolated patch after its workspace was finalized.".to_string(),
            ));
        }
        let patch = self.prepare_verified_patch()?;
        let changed = !patch.is_empty();
        Ok(IsolationReview {
            changed,
            detail: if changed {
                "Controller completed a non-delegating independent Git patch review: the isolated diff passed git diff --check and applies cleanly to the original clean HEAD."
                    .to_string()
            } else {
                "Controller completed a non-delegating independent review and found no Git patch to promote."
                    .to_string()
            },
        })
    }

    fn prepare_verified_patch(&self) -> Result<Vec<u8>, CoreError> {
        let original_status = run_git(
            &self.original_repo_root,
            &["status", "--porcelain", "--untracked-files=normal"],
        )?;
        if !original_status.stdout.is_empty() {
            return Err(CoreError::InvalidInput(
                "The original checkout changed while the isolated patch was running; review and promotion were refused."
                    .to_string(),
            ));
        }
        run_git(&self.isolated_worktree_root, &["add", "-N", "--", "."])?;
        run_git(
            &self.isolated_worktree_root,
            &["diff", "--check", "HEAD", "--"],
        )?;
        let patch = run_git(
            &self.isolated_worktree_root,
            &["diff", "--binary", "--no-ext-diff", "HEAD", "--"],
        )?
        .stdout;
        if !patch.is_empty() {
            run_git_with_input(
                &self.original_repo_root,
                &["apply", "--check", "--whitespace=nowarn", "-"],
                &patch,
            )?;
        }
        Ok(patch)
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
        self.join_without_escape(&self.isolated_source_root, relative, requested)
    }

    fn join_without_escape(
        &self,
        base: &Path,
        relative: &Path,
        requested: &Path,
    ) -> Result<PathBuf, CoreError> {
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
        Ok(base.join(normalized))
    }

    fn cleanup(&mut self) -> Result<(), CoreError> {
        remove_worktree(&self.original_repo_root, &self.isolated_worktree_root)?;
        if let Some(source_id) = self.isolated_source_id.take() {
            self.db.delete_source(&source_id)?;
        }
        delete_workspace_isolation_ownership(&self.db, &self.isolation_id)
    }
}

impl Drop for WorkspaceIsolationRuntime {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        if load_workspace_isolation_ownerships(&self.db)
            .ok()
            .and_then(|ownerships| {
                ownerships
                    .into_iter()
                    .find(|ownership| ownership.id == self.isolation_id)
            })
            .is_some_and(|ownership| {
                owner_status_preserves_isolation(ownership.owner_status.as_deref())
            })
        {
            return;
        }
        if remove_worktree(&self.original_repo_root, &self.isolated_worktree_root).is_err() {
            return;
        }
        if let Some(source_id) = self.isolated_source_id.take() {
            if self.db.delete_source(&source_id).is_err() {
                return;
            }
        }
        let _ = delete_workspace_isolation_ownership(&self.db, &self.isolation_id);
    }
}

fn canonicalize_git_path(stdout: &[u8]) -> Result<PathBuf, CoreError> {
    let path = String::from_utf8_lossy(stdout).trim().to_string();
    if path.is_empty() {
        return Err(CoreError::InvalidInput(
            "Git did not return a repository root for Code Ultra isolation.".to_string(),
        ));
    }
    canonicalize_host_path(Path::new(&path))
}

fn canonicalize_host_path(path: &Path) -> Result<PathBuf, CoreError> {
    let canonical = std::fs::canonicalize(path)?;
    #[cfg(target_os = "windows")]
    {
        use std::path::Prefix;

        let mut components = canonical.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return Ok(canonical);
        };
        match prefix.kind() {
            Prefix::VerbatimDisk(drive) => {
                let mut normalized = PathBuf::from(format!("{}:", char::from(drive)));
                normalized.extend(components);
                Ok(normalized)
            }
            Prefix::VerbatimUNC(server, share) => {
                let mut normalized = PathBuf::from(r"\\");
                normalized.push(server);
                normalized.push(share);
                normalized.extend(components);
                Ok(normalized)
            }
            _ => Ok(canonical),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(canonical)
    }
}

fn run_background_output(command: &mut Command) -> std::io::Result<Output> {
    crate::background_process::configure_std_background(command);
    command.output()
}

fn ensure_process_sandbox_available() -> Result<(), CoreError> {
    #[cfg(target_os = "windows")]
    let output = {
        let mut command = Command::new("wsl.exe");
        command.args([
            "--exec",
            "bwrap",
            "--ro-bind",
            "/",
            "/",
            "--",
            "/usr/bin/true",
        ]);
        run_background_output(&mut command)
    };
    #[cfg(target_os = "linux")]
    let output = {
        let mut command = Command::new("bwrap");
        command.args(["--ro-bind", "/", "/", "--", "/usr/bin/true"]);
        run_background_output(&mut command)
    };
    #[cfg(target_os = "macos")]
    let output = {
        let mut command = Command::new("sandbox-exec");
        command.args(["-p", "(version 1) (allow default)", "/usr/bin/true"]);
        run_background_output(&mut command)
    };
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    let output: std::io::Result<Output> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "unsupported operating system",
    ));

    match output {
        Ok(result) if result.status.success() => Ok(()),
        Ok(result) => Err(CoreError::InvalidInput(format!(
            "Code Ultra requires an OS filesystem sandbox for process execution, but the sandbox probe failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))),
        Err(error) => Err(CoreError::InvalidInput(format!(
            "Code Ultra requires an OS filesystem sandbox for process execution: {error}"
        ))),
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<Output, CoreError> {
    let mut command = Command::new("git");
    command.arg("-C").arg(cwd).args(args);
    let output = run_background_output(&mut command)?;
    ensure_git_success(output, args)
}

fn run_git_with_input(cwd: &Path, args: &[&str], input: &[u8]) -> Result<Output, CoreError> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::background_process::configure_std_background(&mut command);
    let mut child = command.spawn()?;
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

    #[test]
    fn isolated_tool_surface_blocks_unscoped_runtime_tools() {
        let mut definitions = vec![
            ToolDefinition {
                name: "read_file".to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            },
            ToolDefinition {
                name: "project_tool".to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            },
            ToolDefinition {
                name: "mcp_tool".to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            },
            ToolDefinition {
                name: "mcp__filesystem__write_file".to_string(),
                description: String::new(),
                parameters: serde_json::json!({}),
            },
        ];

        WorkspaceIsolationRuntime::retain_safe_tool_definitions(&mut definitions);

        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec!["read_file"]
        );
        assert!(is_unscoped_isolation_tool("mcp__shell__run"));
    }

    fn git(cwd: &Path, args: &[&str]) {
        run_git(cwd, args).expect("git fixture command");
    }

    #[test]
    fn isolated_patch_has_a_non_delegating_review_before_promotion() {
        if ensure_process_sandbox_available().is_err() {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init"]);
        git(repo.path(), &["config", "user.email", "nexa@example.test"]);
        git(repo.path(), &["config", "user.name", "Nexa Test"]);
        git(repo.path(), &["config", "core.autocrlf", "false"]);
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
            WorkspaceIsolationRuntime::prepare(&db, std::slice::from_ref(&source.id), None)
                .unwrap();
        std::fs::write(
            isolation.isolated_source_root.join("tracked.txt"),
            "after\n",
        )
        .unwrap();

        let review = isolation.review_isolated_patch().unwrap();
        assert!(review.changed);
        assert!(review.detail.contains("non-delegating"));
        assert_eq!(
            std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "before\n",
            "review must not mutate the original checkout"
        );

        isolation.promote_verified_patch().unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "after\n"
        );
    }

    #[test]
    fn startup_cleanup_reclaims_crashed_worktree_and_temporary_source() {
        if ensure_process_sandbox_available().is_err() {
            return;
        }
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
        let isolation =
            WorkspaceIsolationRuntime::prepare(&db, std::slice::from_ref(&source.id), None)
                .unwrap();
        let worktree_root = isolation.isolated_worktree_root.clone();
        let isolated_source_id = isolation.isolated_source_id.clone().unwrap();
        std::mem::forget(isolation);

        let report = cleanup_orphaned_workspace_isolations(&db).unwrap();
        assert_eq!(report.removed_worktrees, 1);
        assert_eq!(report.removed_sources, 1);
        assert!(!worktree_root.exists());
        assert!(db.get_source(&isolated_source_id).is_err());
    }

    #[test]
    fn startup_cleanup_retains_unowned_legacy_temp_sources() {
        let db = Database::open_memory().unwrap();
        let legacy_root = std::env::temp_dir()
            .join("nexa-code-ultra")
            .join(format!("legacy-unverifiable-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&legacy_root).unwrap();
        let source = db
            .add_source(CreateSourceInput {
                root_path: legacy_root.to_string_lossy().to_string(),
                include_globs: vec!["**/*".to_string()],
                exclude_globs: Vec::new(),
                watch_enabled: false,
            })
            .unwrap();

        let report = cleanup_orphaned_workspace_isolations(&db).unwrap();

        assert!(report.retained_unverifiable_entries >= 1);
        assert!(db.get_source(&source.id).is_ok());
        assert!(legacy_root.exists());
        db.delete_source(&source.id).unwrap();
        std::fs::remove_dir(&legacy_root).unwrap();
    }

    #[test]
    fn startup_cleanup_preserves_resumable_isolation_until_owner_is_terminal() {
        if ensure_process_sandbox_available().is_err() {
            return;
        }
        use crate::conversation::{ConversationMessage, CreateConversationInput};
        use crate::llm::Role;

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
        let conversation = db
            .create_conversation(&CreateConversationInput {
                provider: "mock".into(),
                model: "mock".into(),
                system_prompt: None,
                collection_context: None,
                project_id: None,
                persona_id: None,
            })
            .unwrap();
        let message = ConversationMessage {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation.id.clone(),
            role: Role::User,
            content: "isolated task".into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            artifacts: None,
            token_count: 2,
            created_at: String::new(),
            sort_order: 0,
            thinking: None,
            image_attachments: None,
        };
        db.add_message(&message).unwrap();
        let turn = db
            .create_conversation_turn(&conversation.id, &message.id, None)
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO agent_task_runs
                     (id, conversation_id, turn_id, user_message_id, status, phase)
                 VALUES (?1, ?2, ?3, ?4, 'paused', 'paused')",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    conversation.id,
                    turn.id,
                    message.id
                ],
            )
            .unwrap();
        let isolation = WorkspaceIsolationRuntime::prepare(
            &db,
            std::slice::from_ref(&source.id),
            Some(&turn.id),
        )
        .unwrap();
        let worktree_root = isolation.isolated_worktree_root.clone();
        let isolated_source_id = isolation.isolated_source_id.clone().unwrap();
        std::mem::forget(isolation);

        let retained = cleanup_orphaned_workspace_isolations(&db).unwrap();
        assert_eq!(retained.removed_worktrees, 0);
        assert!(worktree_root.exists());
        assert!(db.get_source(&isolated_source_id).is_ok());
        let resumed = WorkspaceIsolationRuntime::prepare(
            &db,
            std::slice::from_ref(&source.id),
            Some(&turn.id),
        )
        .unwrap();
        assert_eq!(resumed.isolated_worktree_root, worktree_root);
        std::mem::forget(resumed);

        db.conn()
            .execute(
                "UPDATE agent_task_runs SET status = 'cancelled' WHERE turn_id = ?1",
                [&turn.id],
            )
            .unwrap();
        let removed = cleanup_orphaned_workspace_isolations(&db).unwrap();
        assert_eq!(removed.removed_worktrees, 1);
        assert!(!worktree_root.exists());
        assert!(db.get_source(&isolated_source_id).is_err());
    }

    #[test]
    fn isolated_patch_is_routed_verified_and_promoted() {
        if ensure_process_sandbox_available().is_err() {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init"]);
        git(repo.path(), &["config", "user.email", "nexa@example.test"]);
        git(repo.path(), &["config", "user.name", "Nexa Test"]);
        git(repo.path(), &["config", "core.autocrlf", "false"]);
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
            WorkspaceIsolationRuntime::prepare(&db, std::slice::from_ref(&source.id), None)
                .unwrap();
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
        let mut mcp_calls = vec![ToolCallRequest {
            id: "mcp-1".to_string(),
            name: "mcp__filesystem__write_file".to_string(),
            arguments: r#"{"path":"tracked.txt"}"#.to_string(),
            thought_signature: None,
        }];
        assert!(isolation
            .rewrite_tool_calls(&mut mcp_calls)
            .unwrap_err()
            .to_string()
            .contains("outside the controller-owned filesystem sandbox"));
        let mut shell_calls = vec![ToolCallRequest {
            id: "shell-1".to_string(),
            name: "run_shell".to_string(),
            arguments: serde_json::json!({
                "program": "python",
                "args": [repo.path().join("tracked.txt").to_string_lossy()],
                "cwd": repo.path().to_string_lossy()
            })
            .to_string(),
            thought_signature: None,
        }];
        isolation.rewrite_tool_calls(&mut shell_calls).unwrap();
        let shell_args: Value = serde_json::from_str(&shell_calls[0].arguments).unwrap();
        assert!(Path::new(shell_args["cwd"].as_str().unwrap())
            .starts_with(&isolation.isolated_source_root));
        assert!(Path::new(shell_args["args"][0].as_str().unwrap())
            .starts_with(&isolation.isolated_worktree_root));

        let mut free_form_shell = vec![ToolCallRequest {
            id: "shell-2".to_string(),
            name: "run_shell".to_string(),
            arguments: r#"{"command":"git status"}"#.to_string(),
            thought_signature: None,
        }];
        assert!(isolation
            .rewrite_tool_calls(&mut free_form_shell)
            .unwrap_err()
            .to_string()
            .contains("free-form"));
        let mut traversing_shell = vec![ToolCallRequest {
            id: "shell-3".to_string(),
            name: "run_shell".to_string(),
            arguments: r#"{"program":"cargo","args":["--manifest-path=../Cargo.toml"]}"#
                .to_string(),
            thought_signature: None,
        }];
        assert!(isolation
            .rewrite_tool_calls(&mut traversing_shell)
            .unwrap_err()
            .to_string()
            .contains("escapes"));
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
