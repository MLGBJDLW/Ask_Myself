use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use walkdir::WalkDir;

use super::super::diff_stats::text_diff_artifact;

const MAX_FILE_TRACK_FILES: usize = 5_000;
const MAX_FILE_TRACK_BYTES: u64 = 1024 * 1024;
const MAX_FILE_TRACK_DIFFS: usize = 30;

#[derive(Debug, Clone)]
pub(super) struct FileSnapshotEntry {
    bytes: u64,
    hash: String,
    content: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub(super) struct FileSnapshot {
    files: BTreeMap<PathBuf, FileSnapshotEntry>,
    truncated: bool,
    unreadable_count: usize,
}

pub(super) fn persist_file_changes(
    scope: &crate::turn_file_changes::FileChangeScope,
    call_id: &str,
    before: &FileSnapshot,
    after: &FileSnapshot,
) {
    use crate::turn_file_changes::FileChangeContent;
    if before.truncated
        || after.truncated
        || before.unreadable_count > 0
        || after.unreadable_count > 0
    {
        scope.mark_partial(call_id);
    }
    let paths: BTreeSet<_> = before.files.keys().chain(after.files.keys()).collect();
    for path in paths {
        let old = before.files.get(path);
        let new = after.files.get(path);
        if old.map(|entry| &entry.hash) == new.map(|entry| &entry.hash) {
            continue;
        }
        let path = path.to_string_lossy();
        let result = scope.record(
            &format!("{call_id}:{path}"),
            &path,
            &path,
            FileChangeContent {
                hash: old.map(|entry| entry.hash.as_str()),
                bytes: old.and_then(|entry| entry.content.as_deref()),
            },
            FileChangeContent {
                hash: new.map(|entry| entry.hash.as_str()),
                bytes: new.and_then(|entry| entry.content.as_deref()),
            },
        );
        if let Err(error) = result {
            tracing::warn!(%error, "Native shell file change could not be recorded");
            scope.mark_partial(call_id);
        }
    }
}

#[derive(Debug)]
pub(super) struct FileChangeSet {
    pub(super) artifact: Value,
    pub(super) summary: String,
}

/// The owned task survives a dropped tool waiter. Its tree permit covers the
/// non-cancellable native operation and the final persisted snapshot together.
pub(super) async fn execute_tracked_native<R: Send + 'static>(
    paths: Vec<PathBuf>,
    cwd: PathBuf,
    scope: Option<crate::turn_file_changes::FileChangeScope>,
    call_id: String,
    parent_cancel: tokio_util::sync::CancellationToken,
    work: impl std::future::Future<Output = Result<R, crate::error::CoreError>> + Send + 'static,
) -> Result<(R, Option<FileChangeSet>), crate::error::CoreError> {
    use crate::error::CoreError;
    let cancel = parent_cancel.child_token();
    let _cancel_waiting_on_drop = cancel.clone().drop_guard();
    tokio::spawn(async move {
        let _mutation = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(CoreError::InvalidInput("Native file mutation cancelled before starting".into())),
            guard = crate::file_mutation::lock_native_tree_mutation() => guard,
        };
        let pending = scope.as_ref().map(|scope| scope.begin_pending(&call_id));
        let before_paths = paths.clone();
        let before = tokio::task::spawn_blocking(move || capture_file_snapshot(&before_paths)).await
            .map_err(|error| CoreError::Internal(error.to_string()))?;
        if cancel.is_cancelled() {
            if let Some(pending) = pending { pending.finish(false); }
            return Err(CoreError::InvalidInput("Native file mutation cancelled before starting".into()));
        }
        // Once admitted, finish recording even if the caller times out/stops.
        let result = work.await;
        let changes = tokio::task::spawn_blocking(move || {
            let after = capture_file_snapshot(&paths);
            if let Some(scope) = &scope { persist_file_changes(scope, &call_id, &before, &after); }
            build_run_shell_file_changes(&cwd, &before, &after)
        }).await.map_err(|error| CoreError::Internal(error.to_string()))?;
        if let Some(pending) = pending { pending.finish(result.is_err()); }
        result.map(|value| (value, changes))
    }).await.map_err(|error| CoreError::Internal(error.to_string()))?
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn read_snapshot_entry(path: &Path) -> Result<Option<FileSnapshotEntry>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot stat '{}': {err}", path.display())),
    };
    if !metadata.is_file() {
        return Ok(None);
    }

    let mut file = std::fs::File::open(path)
        .map_err(|err| format!("cannot open '{}': {err}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut content = if metadata.len() <= MAX_FILE_TRACK_BYTES {
        Some(Vec::with_capacity(metadata.len() as usize))
    } else {
        None
    };
    let mut buffer = [0u8; 8192];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("cannot read '{}': {err}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        if let Some(bytes) = content.as_mut() {
            bytes.extend_from_slice(&buffer[..read]);
        }
    }

    Ok(Some(FileSnapshotEntry {
        bytes: metadata.len(),
        hash: hasher.finalize().to_hex().to_string(),
        content,
    }))
}

/// Snapshot only the paths whose native cp/mv operation can change them.
/// Arbitrary shell commands have no trustworthy path set and do not use this.
pub(super) fn capture_file_snapshot(roots: &[PathBuf]) -> FileSnapshot {
    let mut files = BTreeMap::new();
    let mut truncated = false;
    let mut unreadable_count = 0;
    'roots: for root in roots {
        if !root.exists() {
            continue;
        }
        for entry in WalkDir::new(root).follow_links(false) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    unreadable_count += 1;
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let identity = match crate::file_mutation::canonical_file_identity(entry.path()) {
                Ok(path) => path,
                Err(_) => {
                    unreadable_count += 1;
                    continue;
                }
            };
            if files.contains_key(&identity) {
                continue;
            }
            if files.len() >= MAX_FILE_TRACK_FILES {
                truncated = true;
                break 'roots;
            }
            match read_snapshot_entry(entry.path()) {
                Ok(Some(snapshot)) => {
                    files.insert(identity, snapshot);
                }
                Ok(None) => {}
                Err(_) => unreadable_count += 1,
            }
        }
    }
    FileSnapshot {
        files,
        truncated,
        unreadable_count,
    }
}

fn snapshot_path_label(root: &Path, path: &Path) -> String {
    let display = path
        .strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path);
    display.to_string_lossy().replace('\\', "/")
}

fn absolute_path_label(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    #[cfg(windows)]
    {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return rest.to_string();
        }
    }
    text
}

fn utf8_content(entry: Option<&FileSnapshotEntry>) -> Option<String> {
    let bytes = entry?.content.as_ref()?;
    std::str::from_utf8(bytes)
        .ok()
        .map(std::string::ToString::to_string)
}

fn diff_number(diff: &Value, key: &str) -> usize {
    diff.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

fn diff_hunk_count(diff: &Value) -> usize {
    diff.get("hunks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

pub(super) fn build_run_shell_file_changes(
    root: &Path,
    before: &FileSnapshot,
    after: &FileSnapshot,
) -> Option<FileChangeSet> {
    let mut all_paths = BTreeSet::new();
    all_paths.extend(before.files.keys().cloned());
    all_paths.extend(after.files.keys().cloned());

    let mut changes = Vec::new();
    let mut diffs = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut hunk_count = 0usize;
    let mut changed_paths = Vec::new();

    for path in all_paths {
        let old = before.files.get(&path);
        let new = after.files.get(&path);
        let operation = match (old, new) {
            (None, Some(_)) => "create",
            (Some(_), None) => "delete",
            (Some(old), Some(new)) if old.hash != new.hash || old.bytes != new.bytes => "modify",
            _ => continue,
        };

        let label = snapshot_path_label(root, &path);
        let absolute_path = absolute_path_label(&path);
        let old_text = utf8_content(old);
        let new_text = utf8_content(new);
        let mut has_text_diff = false;

        if diffs.len() < MAX_FILE_TRACK_DIFFS {
            let maybe_diff = match operation {
                "create" => new_text
                    .as_deref()
                    .map(|content| text_diff_artifact(&label, "create", "", content)),
                "delete" => old_text
                    .as_deref()
                    .map(|content| text_diff_artifact(&label, "delete", content, "")),
                _ => match (old_text.as_deref(), new_text.as_deref()) {
                    (Some(old_content), Some(new_content)) => Some(text_diff_artifact(
                        &label,
                        "run_shell",
                        old_content,
                        new_content,
                    )),
                    _ => None,
                },
            };

            if let Some(mut diff) = maybe_diff {
                if let Some(object) = diff.as_object_mut() {
                    object.insert(
                        "absolutePath".to_string(),
                        Value::String(absolute_path.clone()),
                    );
                }
                additions += diff_number(&diff, "additions");
                deletions += diff_number(&diff, "deletions");
                hunk_count += diff_hunk_count(&diff);
                diffs.push(diff);
                has_text_diff = true;
            }
        }

        changed_paths.push(label.clone());
        changes.push(json!({
            "path": label,
            "absolutePath": absolute_path,
            "operation": operation,
            "bytesBefore": old.map(|entry| entry.bytes),
            "bytesAfter": new.map(|entry| entry.bytes),
            "textDiff": has_text_diff,
        }));
    }

    if changes.is_empty() {
        return None;
    }

    let text_diff_count = diffs.len();
    let mut artifact = json!({
        "kind": "fileChangeSet",
        "source": "run_shell",
        "root": root.display().to_string(),
        "tracking": {
            "scope": "nativeMutationPaths",
            "maxFiles": MAX_FILE_TRACK_FILES,
            "maxBytesPerFile": MAX_FILE_TRACK_BYTES,
            "truncated": before.truncated || after.truncated,
            "unreadableCount": before.unreadable_count + after.unreadable_count,
        },
        "fileChanges": changes,
        "diffs": diffs,
        "diffStats": {
            "kind": "diffStats",
            "filesChanged": changed_paths.len(),
            "additions": additions,
            "deletions": deletions,
            "hunks": hunk_count,
            "operation": "run_shell",
            "paths": changed_paths,
        }
    });

    if let Some(first_diff) = artifact
        .get("diffs")
        .and_then(Value::as_array)
        .and_then(|items| (items.len() == 1).then(|| items[0].clone()))
    {
        artifact["diff"] = first_diff;
    }

    let paths = artifact["diffStats"]["paths"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .take(6)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let more = artifact["diffStats"]["filesChanged"]
        .as_u64()
        .map(|count| {
            if count > 6 {
                format!(" and {} more", count - 6)
            } else {
                String::new()
            }
        })
        .unwrap_or_default();
    let summary = format!(
        "File changes: {} file(s), +{}, -{}, {} text diff(s): {}{}",
        artifact["diffStats"]["filesChanged"].as_u64().unwrap_or(0),
        additions,
        deletions,
        text_diff_count,
        paths,
        more
    );

    Some(FileChangeSet { artifact, summary })
}

#[cfg(test)]
mod mutation_lifecycle_tests {
    use super::*;
    #[tokio::test]
    async fn dropping_native_waiter_keeps_the_permit_until_recording_finishes() {
        let db = crate::db::Database::open_memory().unwrap();
        let conversation = db
            .create_conversation(
                &serde_json::from_value(json!({ "provider": "open_ai", "model": "test" })).unwrap(),
            )
            .unwrap();
        let scope = crate::turn_file_changes::FileChangeScope::from_context(
            &crate::tools::ToolExecutionContext::new("copy", "{}", &db, &[])
                .with_conversation_id(Some(&conversation.id))
                .with_turn_id(Some("turn-copy")),
        )
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("copied.txt");
        std::fs::write(&path, "before\n").unwrap();
        let work_path = path.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(execute_tracked_native(
            vec![path.clone()],
            directory.path().into(),
            Some(scope),
            "copy".into(),
            tokio_util::sync::CancellationToken::new(),
            async move {
                started_tx.send(()).unwrap();
                release_rx.await.unwrap();
                std::fs::write(work_path, "after\n")?;
                Ok(())
            },
        ));
        started_rx.await.unwrap();
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert!(db.conversation_file_changes(&conversation.id).unwrap()[0].pending);
        let (admitted_tx, mut admitted_rx) = tokio::sync::oneshot::channel();
        let writer = tokio::task::spawn_blocking(move || {
            let _guard = crate::file_mutation::lock_file_mutation(&path, None).unwrap();
            admitted_tx.send(()).unwrap();
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), &mut admitted_rx)
                .await
                .is_err()
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), &mut admitted_rx)
            .await
            .unwrap()
            .unwrap();
        writer.await.unwrap();
        let changes = db.conversation_file_changes(&conversation.id).unwrap();
        assert!(!changes[0].pending && !changes[0].partial);
        assert_eq!(
            (
                changes[0].files.len(),
                changes[0].additions,
                changes[0].deletions
            ),
            (1, 1, 1)
        );
    }
}
