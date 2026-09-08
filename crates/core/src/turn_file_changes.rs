//! Net changes from committed mutations, retained independently of live files.
use crate::{db::Database, error::CoreError, file_checkpoint::FileCheckpoint};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_CONTENT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FileChangeOwner {
    pub conversation_id: String,
    pub turn_id: String,
    pub mutation_namespace: Option<String>,
}

#[derive(Clone)]
pub struct FileChangeScope {
    db: Database,
    owner: FileChangeOwner,
}

/// No hash means absent. A hash without bytes means content was omitted.
pub struct FileChangeContent<'a> {
    pub hash: Option<&'a str>,
    pub bytes: Option<&'a [u8]>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnFileChange {
    pub path: String,
    pub absolute_path: String,
    pub operation: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub content_kind: String,
    pub partial: bool,
    pub revision: u64,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnFileChangeSummary {
    pub turn_id: String,
    pub revision: u64,
    pub files: Vec<TurnFileChange>,
    pub additions: u64,
    pub deletions: u64,
    pub unknown_files: u64,
    pub partial: bool,
    pub pending: bool,
}

pub struct PendingFileChange {
    scope: FileChangeScope,
    mutation_id: String,
    finished: bool,
}

impl PendingFileChange {
    pub fn finish(mut self, partial: bool) {
        self.scope.finish_pending(&self.mutation_id, partial);
        self.finished = true;
    }
}

impl Drop for PendingFileChange {
    fn drop(&mut self) {
        if !self.finished {
            self.scope.finish_pending(&self.mutation_id, true);
        }
    }
}

impl FileChangeScope {
    pub fn begin_pending(&self, call_id: &str) -> PendingFileChange {
        let mutation_id = format!("pending-native:{call_id}");
        let scoped = self.scoped_mutation_id(&mutation_id);
        if let Err(error) = self.db.conn().execute("INSERT INTO turn_file_change_events(conversation_id,turn_id,mutation_id,pending) VALUES (?1,?2,?3,1)
            ON CONFLICT(conversation_id,turn_id,mutation_id) DO UPDATE SET pending=1",
            params![self.owner.conversation_id,self.owner.turn_id,scoped]) {
            tracing::warn!(%error, "Could not record a pending native file mutation");
        }
        PendingFileChange {
            scope: self.clone(),
            mutation_id,
            finished: false,
        }
    }

    fn finish_pending(&self, mutation_id: &str, partial: bool) {
        let result = (|| -> Result<(), CoreError> {
            let mut conn = self.db.conn();
            let tx = conn.transaction()?;
            tx.execute("UPDATE turn_file_change_events SET pending=0,partial=MAX(partial,?4) WHERE conversation_id=?1 AND turn_id=?2 AND mutation_id=?3",
                params![self.owner.conversation_id,self.owner.turn_id,self.scoped_mutation_id(mutation_id),partial])?;
            tx.execute("INSERT OR IGNORE INTO turn_file_change_events(conversation_id,turn_id,mutation_id,partial) VALUES (?1,?2,?3,?4)",
                params![self.owner.conversation_id,self.owner.turn_id,self.scoped_mutation_id(&format!("finished:{mutation_id}")),partial])?;
            tx.commit()?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "Could not settle native file mutation tracking");
        }
    }
    pub fn from_context(context: &crate::tools::ToolExecutionContext<'_>) -> Option<Self> {
        let owner = context.file_change_owner.clone().or_else(|| {
            Some(FileChangeOwner {
                conversation_id: context.conversation_id?.to_string(),
                turn_id: context.turn_id?.to_string(),
                mutation_namespace: None,
            })
        })?;
        Some(Self {
            db: context.db.clone(),
            owner,
        })
    }

    pub fn record_checkpoint(&self, checkpoint: &FileCheckpoint, after: &[u8]) {
        let result = (|| {
            let before: Option<Vec<u8>> = self.db.conn().query_row(
                "SELECT CASE WHEN length(content_before) <= ?2 THEN content_before ELSE NULL END FROM file_checkpoints WHERE id=?1",
                params![checkpoint.id, MAX_CONTENT_BYTES], |row| row.get(0))?;
            let after_hash = blake3::hash(after).to_hex().to_string();
            let identity = crate::file_mutation::canonical_file_identity(std::path::Path::new(
                &checkpoint.absolute_path,
            ))?;
            self.record(
                &checkpoint.id,
                &identity.to_string_lossy(),
                &checkpoint.path,
                FileChangeContent {
                    hash: checkpoint.hash_before.as_deref(),
                    bytes: before.as_deref(),
                },
                FileChangeContent {
                    hash: Some(&after_hash),
                    bytes: Some(after),
                },
            )
        })();
        self.settle_recording(&checkpoint.id, result);
    }

    /// Reconstruct append bytes from the immutable checkpoint, never from a
    /// live path that another mutation may already have changed.
    pub fn record_append(&self, checkpoint: &FileCheckpoint, suffix: &[u8]) {
        let result = (|| {
            let mut after: Vec<u8> = self.db.conn().query_row(
                "SELECT content_before FROM file_checkpoints WHERE id=?1",
                [&checkpoint.id],
                |row| row.get(0),
            )?;
            after.extend_from_slice(suffix);
            self.record_checkpoint(checkpoint, &after);
            Ok(())
        })();
        self.settle_recording(&checkpoint.id, result);
    }

    fn settle_recording(&self, mutation_id: &str, result: Result<(), CoreError>) {
        if let Err(error) = result {
            tracing::warn!(%error, %mutation_id, "Committed file mutation could not be recorded");
            self.mark_partial(mutation_id);
        }
    }

    fn scoped_mutation_id(&self, mutation_id: &str) -> String {
        self.owner
            .mutation_namespace
            .as_ref()
            .map(|namespace| format!("{namespace}:{mutation_id}"))
            .unwrap_or_else(|| mutation_id.to_string())
    }

    pub fn mark_partial(&self, mutation_id: &str) {
        let mutation_id = self.scoped_mutation_id(mutation_id);
        if let Err(error) = self.db.conn().execute(
            "INSERT INTO turn_file_change_events(conversation_id,turn_id,mutation_id,partial) VALUES (?1,?2,?3,1)
             ON CONFLICT(conversation_id,turn_id,mutation_id) DO UPDATE SET partial=1",
            params![self.owner.conversation_id, self.owner.turn_id, mutation_id]) {
            tracing::warn!(%error, "Could not persist incomplete file-change coverage");
        }
    }

    pub fn record(
        &self,
        mutation_id: &str,
        absolute_path: &str,
        display_path: &str,
        before: FileChangeContent<'_>,
        after: FileChangeContent<'_>,
    ) -> Result<(), CoreError> {
        let mutation_id = self.scoped_mutation_id(mutation_id);
        // Callers supply backend-resolved absolute identities. Preserve case
        // for case-sensitive directories, including those on Windows.
        let mut conn = self.db.conn();
        let transaction = conn.transaction()?;
        let admitted = transaction.execute(
            "INSERT OR IGNORE INTO turn_file_change_events(conversation_id,turn_id,mutation_id) VALUES (?1,?2,?3)",
            params![self.owner.conversation_id, self.owner.turn_id, mutation_id])?;
        if admitted == 0 {
            return Ok(());
        }
        type Baseline = (Option<Vec<u8>>, Option<String>, Option<String>, bool);
        let baseline: Option<Baseline> = transaction.query_row(
            "SELECT before_content,before_hash,after_hash,partial FROM turn_file_changes WHERE conversation_id=?1 AND turn_id=?2 AND absolute_path=?3",
            params![self.owner.conversation_id, self.owner.turn_id, absolute_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).optional()?;
        let partial = baseline
            .as_ref()
            .is_some_and(|row| row.3 || row.2.as_deref() != before.hash);
        if partial {
            transaction.execute("UPDATE turn_file_change_events SET partial=1 WHERE conversation_id=?1 AND turn_id=?2 AND mutation_id=?3",
                params![self.owner.conversation_id, self.owner.turn_id, mutation_id])?;
        }
        let original_hash = baseline
            .as_ref()
            .map(|row| row.1.as_deref())
            .unwrap_or(before.hash);
        let original_bytes = baseline
            .as_ref()
            .map(|row| row.0.as_deref())
            .unwrap_or(before.bytes);
        let old_bytes = original_bytes.filter(|bytes| bytes.len() <= MAX_CONTENT_BYTES);
        let new_bytes = after.bytes.filter(|bytes| bytes.len() <= MAX_CONTENT_BYTES);
        let old_text = if original_hash.is_none() {
            Some("")
        } else {
            old_bytes.and_then(text_content)
        };
        let new_text = if after.hash.is_none() {
            Some("")
        } else {
            new_bytes.and_then(text_content)
        };
        let (kind, additions, deletions) = match (old_text, new_text) {
            (Some(old), Some(new)) => {
                let diff =
                    crate::tools::diff_stats::text_diff_artifact(display_path, "edit", old, new);
                if diff["statsExact"].as_bool() == Some(true) {
                    (
                        "text",
                        diff["additions"].as_i64(),
                        diff["deletions"].as_i64(),
                    )
                } else {
                    ("unavailable", None, None)
                }
            }
            _ if (original_hash.is_some() && old_bytes.is_none())
                || (after.hash.is_some() && new_bytes.is_none()) =>
            {
                ("too_large", None, None)
            }
            _ => ("binary", None, None),
        };
        transaction.execute(
            "INSERT INTO turn_file_changes(conversation_id,turn_id,absolute_path,display_path,before_content,after_content,before_hash,after_hash,
              existed_before,exists_after,additions,deletions,content_kind,partial)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
             ON CONFLICT(conversation_id,turn_id,absolute_path) DO UPDATE SET
              after_content=excluded.after_content, after_hash=excluded.after_hash, exists_after=excluded.exists_after,
              additions=excluded.additions,deletions=excluded.deletions,content_kind=excluded.content_kind,
              partial=excluded.partial, revision=turn_file_changes.revision+1",
            params![self.owner.conversation_id,self.owner.turn_id,absolute_path,display_path,old_bytes,new_bytes,original_hash,after.hash,
                original_hash.is_some(),after.hash.is_some(),additions,deletions,kind,partial])?;
        transaction.commit()?;
        Ok(())
    }
}

fn text_content(bytes: &[u8]) -> Option<&str> {
    if bytes.contains(&0) {
        None
    } else {
        std::str::from_utf8(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup() -> (Database, FileChangeScope) {
        let db = Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(
                &serde_json::from_value(json!({ "provider": "open_ai", "model": "test" })).unwrap(),
            )
            .unwrap();
        let scope = FileChangeScope {
            db: db.clone(),
            owner: FileChangeOwner {
                conversation_id: conversation.id,
                turn_id: "turn-1".into(),
                mutation_namespace: None,
            },
        };
        (db, scope)
    }
    fn record(
        scope: &FileChangeScope,
        id: &str,
        path: &str,
        before: Option<&[u8]>,
        after: Option<&[u8]>,
    ) {
        let old_hash = before.map(|bytes| blake3::hash(bytes).to_hex().to_string());
        let new_hash = after.map(|bytes| blake3::hash(bytes).to_hex().to_string());
        scope
            .record(
                id,
                path,
                path,
                FileChangeContent {
                    hash: old_hash.as_deref(),
                    bytes: before,
                },
                FileChangeContent {
                    hash: new_hash.as_deref(),
                    bytes: after,
                },
            )
            .unwrap();
    }
    fn summary(db: &Database, scope: &FileChangeScope) -> TurnFileChangeSummary {
        db.conversation_file_changes(&scope.owner.conversation_id)
            .unwrap()
            .remove(0)
    }

    #[test]
    fn repeated_edits_are_net_deduplicated_and_reverts_clear_the_capsule() {
        let (db, scope) = setup();
        let original = b"old\nkeep\nkeep\nkeep\nkeep\nkeep\nkeep\nkeep\nlast\n";
        let changed = b"new\nkeep\nkeep\nkeep\nkeep\nkeep\nkeep\nkeep\nend\n";
        record(&scope, "edit-1", "/a.txt", Some(original), Some(changed));
        let first = summary(&db, &scope);
        assert_eq!(
            (first.files.len(), first.additions, first.deletions),
            (1, 2, 2)
        );
        record(&scope, "edit-2", "/a.txt", Some(changed), Some(b"final\n"));
        record(&scope, "edit-1", "/a.txt", Some(original), Some(changed));
        let second = summary(&db, &scope);
        assert_eq!(
            (second.files.len(), second.additions, second.deletions),
            (1, 1, 9)
        );
        record(&scope, "revert", "/a.txt", Some(b"final\n"), Some(original));
        let reverted = summary(&db, &scope);
        assert!(reverted.files.is_empty());
        assert!(reverted.revision > second.revision);
        assert!(!reverted.partial);
    }

    #[test]
    fn existence_binary_and_unavailable_counts_are_distinct_and_not_capped_at_100() {
        let (db, scope) = setup();
        record(&scope, "create-empty", "/empty", None, Some(b""));
        assert_eq!(summary(&db, &scope).files.len(), 1);
        record(&scope, "delete-empty", "/empty", Some(b""), None);
        assert!(summary(&db, &scope).files.is_empty());
        for i in 0..125 {
            record(
                &scope,
                &format!("create-{i}"),
                &format!("/file-{i}"),
                None,
                Some(b"line\n"),
            );
        }
        record(&scope, "binary", "/image.bin", None, Some(b"\0\x01\x02"));
        let oversized = vec![b'a'; MAX_CONTENT_BYTES + 1];
        record(&scope, "large", "/large.txt", None, Some(&oversized));
        let result = summary(&db, &scope);
        assert_eq!(
            (result.files.len(), result.additions, result.unknown_files),
            (127, 125, 2)
        );
        assert!(result
            .files
            .iter()
            .find(|file| file.path == "/image.bin")
            .unwrap()
            .additions
            .is_none());
    }

    #[test]
    fn historical_diff_keeps_its_after_state_and_marks_intervening_edits() {
        let (db, scope) = setup();
        record(&scope, "first", "/a.txt", Some(b"old\n"), Some(b"saved\n"));
        let next_turn = FileChangeScope {
            db: db.clone(),
            owner: FileChangeOwner {
                turn_id: "turn-2".into(),
                ..scope.owner.clone()
            },
        };
        record(
            &next_turn,
            "second",
            "/a.txt",
            Some(b"saved\n"),
            Some(b"later\n"),
        );
        let diff = db
            .turn_file_diff(&scope.owner.conversation_id, "turn-1", "/a.txt")
            .unwrap()
            .to_string();
        assert!(diff.contains("saved"));
        assert!(!diff.contains("later"));
        record(
            &next_turn,
            "foreign-gap",
            "/a.txt",
            Some(b"external\n"),
            Some(b"agent\n"),
        );
        assert!(
            db.conversation_file_changes(&scope.owner.conversation_id)
                .unwrap()
                .iter()
                .find(|value| value.turn_id == "turn-2")
                .unwrap()
                .partial
        );
        scope.mark_partial("untracked-shell");
        assert!(summary(&db, &scope).partial);
    }

    #[tokio::test]
    async fn native_tools_and_child_registry_share_the_explicit_parent_turn() {
        use crate::tools::{ToolExecutionContext, ToolRegistry};
        let (db, scope) = setup();
        let directory = tempfile::tempdir().unwrap();
        db.add_source(crate::sources::CreateSourceInput {
            root_path: directory.path().to_string_lossy().into(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .unwrap();
        let path = directory.path().join("notes.txt");
        let mut registry = ToolRegistry::new().with_file_change_owner(scope.owner.clone());
        registry.register(Box::new(crate::tools::create_file_tool::CreateFileTool));
        registry.register(Box::new(crate::tools::edit_file_tool::EditFileTool));
        registry.register(Box::new(crate::tools::multi_edit_tool::MultiEditTool));
        registry.register(Box::new(crate::tools::write_note_tool::WriteNoteTool));
        let registry = registry.filtered(&[
            "create_file".into(),
            "edit_file".into(),
            "multi_edit".into(),
            "write_note".into(),
        ]);
        for (index, (tool, args)) in [
            (
                "create_file",
                json!({ "path": path, "content": "one\n", "mode": "create" }),
            ),
            (
                "create_file",
                json!({ "path": path, "content": "two\n", "mode": "append", "expected_bytes": 4 }),
            ),
            (
                "edit_file",
                json!({ "path": path, "old_str": "one", "new_str": "first" }),
            ),
            (
                "multi_edit",
                json!({ "path": path, "edits": [{ "old_str": "two", "new_str": "second" }] }),
            ),
            (
                "write_note",
                json!({ "filename": "extra.md", "content": "note\n", "mode": "create" }),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let call = format!("child-call-{index}");
            let arguments = args.to_string();
            let result = registry
                .execute(tool, ToolExecutionContext::new(&call, &arguments, &db, &[]))
                .await
                .unwrap();
            assert!(!result.is_error, "{}", result.content);
        }
        let result = summary(&db, &scope);
        assert_eq!(
            (result.files.len(), result.additions, result.deletions),
            (2, 3, 0)
        );
        assert_eq!(result.files[0].operation, "create");
        assert!(!result.partial);
        std::fs::write(&path, "external after completion").unwrap();
        let diff = db
            .turn_file_diff(
                &scope.owner.conversation_id,
                "turn-1",
                &result.files[0].absolute_path,
            )
            .unwrap()
            .to_string();
        assert!(diff.contains("first") && diff.contains("second") && !diff.contains("external"));
        let storage = directory.path().join("persisted.db");
        db.conn()
            .execute("VACUUM INTO ?1", [storage.to_string_lossy().as_ref()])
            .unwrap();
        let reopened = Database::new(&storage).unwrap();
        assert_eq!(summary(&reopened, &scope).files.len(), 2);
        assert!(reopened
            .turn_file_diff(
                &scope.owner.conversation_id,
                "turn-1",
                &result.files[0].absolute_path
            )
            .unwrap()
            .to_string()
            .contains("second"));
    }

    #[tokio::test]
    async fn native_copy_and_text_edit_share_one_canonical_baseline() {
        use crate::tools::{ToolExecutionContext, ToolRegistry};
        let (db, scope) = setup();
        let directory = tempfile::tempdir().unwrap();
        db.add_source(crate::sources::CreateSourceInput {
            root_path: directory.path().to_string_lossy().into(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .unwrap();
        let path = directory.path().join("file.txt");
        let backup = directory.path().join("backup.txt");
        std::fs::write(&path, "original\n").unwrap();
        std::fs::write(&backup, "original\n").unwrap();
        std::fs::create_dir(directory.path().join("sub")).unwrap();
        let mut registry = ToolRegistry::new().with_file_change_owner(scope.owner.clone());
        registry.register(Box::new(crate::tools::edit_file_tool::EditFileTool));
        registry.register(Box::new(crate::tools::run_shell_tool::RunShellTool));
        let edit = json!({ "path": path, "old_str": "original", "new_str": "changed" }).to_string();
        let result = registry
            .execute(
                "edit_file",
                ToolExecutionContext::new("edit", &edit, &db, &[]),
            )
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.content);
        assert_eq!(summary(&db, &scope).files.len(), 1);
        let copy = json!({ "program": "cp", "args": [backup, directory.path().join("sub/../file.txt")], "cwd": directory.path() }).to_string();
        let result = registry
            .execute(
                "run_shell",
                ToolExecutionContext::new("copy", &copy, &db, &[]),
            )
            .await
            .unwrap();
        assert!(!result.is_error, "{}", result.content);
        let result = summary(&db, &scope);
        assert!(result.files.is_empty(), "{result:?}");
        assert!(!result.partial);
    }

    #[test]
    fn user_restore_clears_active_changes_and_keeps_completed_history() {
        let (db, scope) = setup();
        let directory = tempfile::tempdir().unwrap();
        db.add_source(crate::sources::CreateSourceInput {
            root_path: directory.path().to_string_lossy().into(),
            include_globs: vec![],
            exclude_globs: vec![],
            watch_enabled: false,
        })
        .unwrap();
        db.conn().execute("INSERT INTO messages(id,conversation_id,role,content,sort_order) VALUES ('restore-user',?1,'user','Edit',0)", [&scope.owner.conversation_id]).unwrap();
        db.conn().execute("INSERT INTO conversation_turns(id,conversation_id,user_message_id,status) VALUES ('turn-1',?1,'restore-user','running')", [&scope.owner.conversation_id]).unwrap();
        db.conn().execute("INSERT INTO agent_task_runs(id,conversation_id,turn_id,user_message_id,status) VALUES ('restore-run',?1,'turn-1','restore-user','running')", [&scope.owner.conversation_id]).unwrap();
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "original\n").unwrap();
        let checkpoint = db
            .create_file_checkpoint(crate::file_checkpoint::CreateFileCheckpointInput {
                conversation_id: Some(&scope.owner.conversation_id),
                tool_call_id: "edit",
                tool_name: "edit_file",
                operation: "edit",
                path: "file.txt",
                absolute_path: &path,
            })
            .unwrap();
        std::fs::write(&path, "changed\n").unwrap();
        scope.record_checkpoint(&checkpoint, b"changed\n");
        db.conn()
            .execute(
                "UPDATE agent_task_runs SET status='waiting_approval' WHERE id='restore-run'",
                [],
            )
            .unwrap();
        db.restore_file_checkpoint(&checkpoint.id).unwrap();
        assert!(summary(&db, &scope).files.is_empty());
        db.conn()
            .execute(
                "UPDATE agent_task_runs SET status='cancelling' WHERE id='restore-run'",
                [],
            )
            .unwrap();
        std::fs::write(&path, "another change\n").unwrap();
        db.record_active_file_change(&path, Some(b"original\n"), Some(b"another change\n"));
        db.restore_file_checkpoint(&checkpoint.id).unwrap();
        assert!(summary(&db, &scope).files.is_empty());
        std::fs::write(&path, "saved\n").unwrap();
        db.record_active_file_change(&path, Some(b"original\n"), Some(b"saved\n"));
        db.conn()
            .execute(
                "UPDATE agent_task_runs SET status='completed' WHERE id='restore-run'",
                [],
            )
            .unwrap();
        db.restore_file_checkpoint(&checkpoint.id).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original\n");
        let result = summary(&db, &scope);
        assert_eq!(result.files.len(), 1);
        assert!(db
            .turn_file_diff(
                &scope.owner.conversation_id,
                "turn-1",
                &result.files[0].absolute_path
            )
            .unwrap()
            .to_string()
            .contains("saved"));
    }

    #[test]
    fn interrupted_pending_changes_settle_without_polling_forever_after_restart() {
        let (db, scope) = setup();
        let pending = scope.begin_pending("copy");
        assert!(summary(&db, &scope).pending);
        drop(pending);
        let result = summary(&db, &scope);
        assert!(!result.pending && result.partial);
        let pending = scope.begin_pending("next-copy");
        assert!(summary(&db, &scope).pending);
        assert_eq!(db.recover_pending_file_changes().unwrap(), 1);
        let result = summary(&db, &scope);
        assert!(!result.pending && result.partial);
        pending.finish(true);
    }
}

impl Database {
    /// Only desktop startup clears owners lost with the preceding process.
    pub fn recover_pending_file_changes(&self) -> Result<usize, CoreError> {
        Ok(self.conn().execute(
            "UPDATE turn_file_change_events SET pending=0,partial=1 WHERE pending=1",
            [],
        )?)
    }

    pub fn record_active_file_change(
        &self,
        path: &std::path::Path,
        before: Option<&[u8]>,
        after: Option<&[u8]>,
    ) {
        let result = (|| -> Result<(), CoreError> {
            let identity = crate::file_mutation::canonical_file_identity(path)?
                .to_string_lossy()
                .into_owned();
            let owners: Vec<FileChangeOwner> = {
                let conn = self.conn();
                let mut statement = conn.prepare("SELECT DISTINCT changes.conversation_id,changes.turn_id FROM turn_file_changes changes
                    JOIN agent_task_runs run ON run.conversation_id=changes.conversation_id AND run.turn_id=changes.turn_id
                    WHERE changes.absolute_path=?1 AND run.status IN ('queued','running','waiting_approval','paused','awaiting_user_input','cancelling')")?;
                let owners = statement
                    .query_map([&identity], |row| {
                        Ok(FileChangeOwner {
                            conversation_id: row.get(0)?,
                            turn_id: row.get(1)?,
                            mutation_namespace: None,
                        })
                    })?
                    .collect::<Result<_, _>>()?;
                owners
            };
            let before_hash = before.map(|bytes| blake3::hash(bytes).to_hex().to_string());
            let after_hash = after.map(|bytes| blake3::hash(bytes).to_hex().to_string());
            let mutation = format!("user-file-change:{}", uuid::Uuid::new_v4());
            for owner in owners {
                let scope = FileChangeScope {
                    db: self.clone(),
                    owner,
                };
                let result = scope.record(
                    &mutation,
                    &identity,
                    &identity,
                    FileChangeContent {
                        hash: before_hash.as_deref(),
                        bytes: before,
                    },
                    FileChangeContent {
                        hash: after_hash.as_deref(),
                        bytes: after,
                    },
                );
                scope.settle_recording(&mutation, result);
            }
            Ok(())
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "Could not update an active turn after a user file change");
        }
    }

    pub fn conversation_file_changes(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<TurnFileChangeSummary>, CoreError> {
        let conn = self.conn();
        let mut summaries = BTreeMap::new();
        let mut events = conn.prepare("SELECT turn_id,MAX(id),MAX(partial),MAX(pending) FROM turn_file_change_events WHERE conversation_id=?1 GROUP BY turn_id")?;
        for row in events.query_map([conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u64>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })? {
            let (turn_id, revision, partial, pending) = row?;
            summaries.insert(
                turn_id.clone(),
                TurnFileChangeSummary {
                    turn_id,
                    revision,
                    partial,
                    pending,
                    ..Default::default()
                },
            );
        }
        let mut files = conn.prepare("SELECT turn_id,display_path,absolute_path,existed_before,exists_after,additions,deletions,content_kind,partial,revision
            FROM turn_file_changes WHERE conversation_id=?1 AND (existed_before!=exists_after OR before_hash IS NOT after_hash) ORDER BY absolute_path")?;
        for row in files.query_map([conversation_id], |row| {
            let existed: bool = row.get(3)?;
            let exists: bool = row.get(4)?;
            Ok((
                row.get::<_, String>(0)?,
                TurnFileChange {
                    path: row.get(1)?,
                    absolute_path: row.get(2)?,
                    operation: if !exists {
                        "delete"
                    } else if !existed {
                        "create"
                    } else {
                        "edit"
                    }
                    .into(),
                    additions: row.get(5)?,
                    deletions: row.get(6)?,
                    content_kind: row.get(7)?,
                    partial: row.get(8)?,
                    revision: row.get(9)?,
                },
            ))
        })? {
            let (turn, file) = row?;
            if let Some(summary) = summaries.get_mut(&turn) {
                summary.additions += file.additions.unwrap_or(0);
                summary.deletions += file.deletions.unwrap_or(0);
                summary.unknown_files +=
                    u64::from(file.additions.is_none() || file.deletions.is_none());
                summary.partial |= file.partial;
                summary.files.push(file);
            }
        }
        Ok(summaries.into_values().collect())
    }

    pub fn turn_file_diff(
        &self,
        conversation_id: &str,
        turn_id: &str,
        absolute_path: &str,
    ) -> Result<serde_json::Value, CoreError> {
        let (before, after, existed, exists, path, kind): (Option<Vec<u8>>, Option<Vec<u8>>, bool, bool, String, String) = self.conn().query_row(
            "SELECT before_content,after_content,existed_before,exists_after,display_path,content_kind FROM turn_file_changes WHERE conversation_id=?1 AND turn_id=?2 AND absolute_path=?3",
            params![conversation_id,turn_id,absolute_path], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)))?;
        if kind != "text" {
            return Err(CoreError::InvalidInput(format!(
                "Diff content unavailable: {kind}"
            )));
        }
        let old = if existed {
            before.as_deref().and_then(text_content)
        } else {
            Some("")
        };
        let new = if exists {
            after.as_deref().and_then(text_content)
        } else {
            Some("")
        };
        let (Some(old), Some(new)) = (old, new) else {
            return Err(CoreError::InvalidInput(
                "Stored diff content unavailable".into(),
            ));
        };
        let mut diff = crate::tools::diff_stats::text_diff_artifact(
            &path,
            if !exists {
                "delete"
            } else if !existed {
                "create"
            } else {
                "edit"
            },
            old,
            new,
        );
        diff["absolutePath"] = absolute_path.into();
        Ok(diff)
    }
}
