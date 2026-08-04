use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use walkdir::{DirEntry, WalkDir};

use super::super::diff_stats::text_diff_artifact;

const MAX_FILE_TRACK_FILES: usize = 5_000;
const MAX_FILE_TRACK_BYTES: u64 = 1024 * 1024;
const MAX_FILE_TRACK_DIFFS: usize = 30;

const FILE_TRACK_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];

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

#[derive(Debug)]
pub(super) struct FileChangeSet {
    pub(super) artifact: Value,
    pub(super) summary: String,
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn should_track_entry(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }
    let name = entry.file_name().to_string_lossy();
    !FILE_TRACK_SKIP_DIRS
        .iter()
        .any(|skip| name.eq_ignore_ascii_case(skip))
}

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

pub(super) fn capture_file_snapshot(root: &Path) -> FileSnapshot {
    if let Some(paths) = git_snapshot_paths(root) {
        return capture_known_paths(paths);
    }

    let mut files = BTreeMap::new();
    let mut truncated = false;
    let mut unreadable_count = 0usize;

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(should_track_entry)
    {
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
        if files.len() >= MAX_FILE_TRACK_FILES {
            truncated = true;
            break;
        }
        let path = entry.path().to_path_buf();
        match read_snapshot_entry(&path) {
            Ok(Some(snapshot)) => {
                files.insert(path, snapshot);
            }
            Ok(None) => {}
            Err(_) => {
                unreadable_count += 1;
            }
        }
    }

    FileSnapshot {
        files,
        truncated,
        unreadable_count,
    }
}

fn git_snapshot_paths(root: &Path) -> Option<Vec<PathBuf>> {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(root).args([
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ]);
    crate::background_process::configure_std_background(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .take(MAX_FILE_TRACK_FILES + 1)
            .map(|path| root.join(String::from_utf8_lossy(path).as_ref()))
            .collect(),
    )
}

fn capture_known_paths(paths: Vec<PathBuf>) -> FileSnapshot {
    let mut files = BTreeMap::new();
    let truncated = paths.len() > MAX_FILE_TRACK_FILES;
    let mut unreadable_count = 0;
    for path in paths.into_iter().take(MAX_FILE_TRACK_FILES) {
        match read_snapshot_entry(&path) {
            Ok(Some(snapshot)) => {
                files.insert(path, snapshot);
            }
            Ok(None) => {}
            Err(_) => unreadable_count += 1,
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
