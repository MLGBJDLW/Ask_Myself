use serde_json::{json, Value};

use crate::file_checkpoint::{checkpoint_artifact, FileCheckpoint};

const DIFF_CONTEXT_LINES: usize = 3;
const MAX_DIFF_LINES: usize = 400;

pub(crate) fn changed_line_count(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let count = text.split('\n').count();
    if text.ends_with('\n') {
        count.saturating_sub(1)
    } else {
        count
    }
}

fn text_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.lines().collect()
    }
}

pub(crate) fn diff_stats_artifact(
    path: &str,
    operation: &str,
    additions: usize,
    deletions: usize,
    hunks: usize,
    replacements: Option<usize>,
) -> Value {
    let mut stats = json!({
        "kind": "diffStats",
        "filesChanged": usize::from(!path.is_empty()),
        "additions": additions,
        "deletions": deletions,
        "hunks": hunks,
        "operation": operation,
        "paths": if path.is_empty() { Vec::<String>::new() } else { vec![path.to_string()] },
    });

    if let Some(replacements) = replacements {
        if let Some(object) = stats.as_object_mut() {
            object.insert("replacements".to_string(), json!(replacements));
        }
    }

    stats
}

pub(crate) fn checkpoint_artifact_with_diff(
    checkpoint: &FileCheckpoint,
    bytes_after: Option<u64>,
    diff: Value,
    replacements: Option<usize>,
) -> Value {
    let mut artifact = checkpoint_artifact(checkpoint, bytes_after);
    if let Some(object) = artifact.as_object_mut() {
        object.insert(
            "diffStats".to_string(),
            diff_stats_from_diff(&diff, replacements),
        );
        object.insert("diff".to_string(), diff);
    }
    artifact
}

pub(crate) fn create_file_diff_artifact(path: &str, file_content: &str) -> Value {
    let all_lines = text_lines(file_content);
    let displayed_count = all_lines.len().min(MAX_DIFF_LINES);
    let lines: Vec<Value> = all_lines
        .iter()
        .take(displayed_count)
        .enumerate()
        .map(|(idx, content)| {
            json!({
                "type": "addition",
                "oldLine": null,
                "newLine": idx + 1,
                "content": content,
            })
        })
        .collect();

    json!({
        "path": path,
        "operation": "create",
        "additions": all_lines.len(),
        "deletions": 0,
        "truncated": displayed_count < all_lines.len(),
        "omittedLineCount": all_lines.len().saturating_sub(displayed_count),
        "hunks": [{
            "oldStart": 0,
            "newStart": 1,
            "oldLines": 0,
            "newLines": all_lines.len(),
            "lines": lines,
        }]
    })
}

pub(crate) fn text_diff_artifact(
    path: &str,
    operation: &str,
    old_content: &str,
    new_content: &str,
) -> Value {
    use similar::{Algorithm, ChangeTag, TextDiff};
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(80);
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .timeout(budget)
        .diff_lines(old_content, new_content);
    let stats_exact = started.elapsed() < budget;
    let mut additions = 0;
    let mut deletions = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => deletions += 1,
            ChangeTag::Equal => {}
        }
    }
    let mut hunks = Vec::new();
    let mut included = 0;
    let mut omitted = 0;
    let mut clipped = false;
    for group in diff.grouped_ops(DIFF_CONTEXT_LINES) {
        let mut lines = Vec::new();
        let first = group.first().unwrap();
        let last = group.last().unwrap();
        for op in &group {
            for change in diff.iter_changes(op) {
                if included >= MAX_DIFF_LINES {
                    omitted += 1;
                    continue;
                }
                included += 1;
                let raw = change.value().trim_end_matches(['\r', '\n']);
                let content: String = raw.chars().take(2048).collect();
                clipped |= content.len() < raw.len();
                lines.push(json!({
                    "type": match change.tag() { ChangeTag::Equal => "context", ChangeTag::Insert => "addition", ChangeTag::Delete => "deletion" },
                    "oldLine": change.old_index().map(|index| index + 1),
                    "newLine": change.new_index().map(|index| index + 1),
                    "content": content,
                }));
            }
        }
        if !lines.is_empty() {
            hunks.push(json!({ "oldStart": first.old_range().start + 1, "newStart": first.new_range().start + 1,
                "oldLines": last.old_range().end - first.old_range().start,
                "newLines": last.new_range().end - first.new_range().start, "lines": lines }));
        }
    }
    json!({ "path": path, "operation": operation, "additions": additions, "deletions": deletions,
        "statsExact": stats_exact, "truncated": omitted > 0 || clipped, "omittedLineCount": omitted, "hunks": hunks })
}

pub(crate) fn diff_stats_from_diff(diff: &Value, replacements: Option<usize>) -> Value {
    let path = diff.get("path").and_then(Value::as_str).unwrap_or("");
    let operation = diff
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("edit");
    let additions = diff.get("additions").and_then(Value::as_u64).unwrap_or(0) as usize;
    let deletions = diff.get("deletions").and_then(Value::as_u64).unwrap_or(0) as usize;
    let hunks = diff
        .get("hunks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    diff_stats_artifact(path, operation, additions, deletions, hunks, replacements)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_line_count_ignores_trailing_newline() {
        assert_eq!(changed_line_count(""), 0);
        assert_eq!(changed_line_count("one"), 1);
        assert_eq!(changed_line_count("one\n"), 1);
        assert_eq!(changed_line_count("one\ntwo\n"), 2);
    }

    #[test]
    fn diff_stats_from_diff_extracts_counts() {
        let diff = json!({
            "path": "notes.md",
            "operation": "str_replace",
            "additions": 2,
            "deletions": 1,
            "hunks": [{ "lines": [] }]
        });

        let stats = diff_stats_from_diff(&diff, Some(1));
        assert_eq!(stats["kind"], "diffStats");
        assert_eq!(stats["filesChanged"], 1);
        assert_eq!(stats["additions"], 2);
        assert_eq!(stats["deletions"], 1);
        assert_eq!(stats["hunks"], 1);
        assert_eq!(stats["replacements"], 1);
        assert_eq!(stats["paths"][0], "notes.md");
    }

    #[test]
    fn create_file_diff_artifact_records_added_lines() {
        let diff = create_file_diff_artifact("notes.md", "one\ntwo\n");
        assert_eq!(diff["operation"], "create");
        assert_eq!(diff["additions"], 2);
        assert_eq!(diff["deletions"], 0);
        assert_eq!(diff["hunks"][0]["lines"][0]["type"], "addition");
    }

    #[test]
    fn text_diff_artifact_records_replacement_lines() {
        let diff = text_diff_artifact("notes.md", "overwrite", "one\ntwo\n", "one\nthree\n");
        assert_eq!(diff["operation"], "overwrite");
        assert_eq!(diff["additions"], 1);
        assert_eq!(diff["deletions"], 1);
        assert!(diff["hunks"][0]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["type"] == "deletion" && line["content"] == "two"));
        assert!(diff["hunks"][0]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["type"] == "addition" && line["content"] == "three"));
    }
}
