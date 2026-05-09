use serde_json::{json, Value};

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
}
