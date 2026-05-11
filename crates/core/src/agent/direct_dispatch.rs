use crate::db::Database;

/// Internal result of a direct-dispatch pattern match.
pub(super) struct DirectDispatch {
    pub(super) tool_name: String,
    pub(super) arguments: String,
}

/// Match user query text against known direct-dispatch patterns.
///
/// Only matches CLEAR, unambiguous commands. Anything vague or conversational
/// falls through to the LLM.
pub(super) fn match_direct_pattern(user_text: &str, db: &Database) -> Option<DirectDispatch> {
    let q = user_text.trim().to_lowercase();
    let q = q
        .trim_end_matches(|c: char| ".?!\u{3002}\u{ff1f}\u{ff01}".contains(c))
        .trim();

    // Strip common polite prefixes/suffixes.
    let q = q.strip_prefix("please ").unwrap_or(q);
    let q = q.strip_prefix("can you ").unwrap_or(q);
    let q = q.strip_prefix("could you ").unwrap_or(q);
    let q = q.strip_suffix(" please").unwrap_or(q);
    let q = q.strip_prefix('\u{8BF7}').unwrap_or(q); // 请

    // --- List sources (no arguments) ------------------------------------
    const LIST_SOURCES: &[&str] = &[
        "list sources",
        "list my sources",
        "show sources",
        "show my sources",
        "show all sources",
        "what sources do i have",
        "what are my sources",
        "\u{663E}\u{793A}\u{6570}\u{636E}\u{6E90}", // 显示数据源
        "\u{5217}\u{51FA}\u{6570}\u{636E}\u{6E90}", // 列出数据源
        "\u{67E5}\u{770B}\u{6570}\u{636E}\u{6E90}", // 查看数据源
        "\u{6570}\u{636E}\u{6E90}\u{5217}\u{8868}", // 数据源列表
        "\u{30BD}\u{30FC}\u{30B9}\u{4E00}\u{89A7}", // ソース一覧
        "\u{30BD}\u{30FC}\u{30B9}\u{3092}\u{8868}\u{793A}", // ソースを表示
    ];
    if LIST_SOURCES.contains(&q) {
        return Some(DirectDispatch {
            tool_name: "list_sources".into(),
            arguments: "{}".into(),
        });
    }

    // --- List playbooks (action: list) ----------------------------------
    const LIST_PLAYBOOKS: &[&str] = &[
        "list playbooks",
        "list my playbooks",
        "show playbooks",
        "show my playbooks",
        "what playbooks do i have",
        "what are my playbooks",
        "\u{663E}\u{793A}\u{5267}\u{672C}", // 显示剧本
        "\u{5217}\u{51FA}\u{5267}\u{672C}", // 列出剧本
        "\u{67E5}\u{770B}\u{5267}\u{672C}", // 查看剧本
        "\u{5267}\u{672C}\u{5217}\u{8868}", // 剧本列表
        "\u{30D7}\u{30EC}\u{30A4}\u{30D6}\u{30C3}\u{30AF}\u{4E00}\u{89A7}", // プレイブック一覧
    ];
    if LIST_PLAYBOOKS.contains(&q) {
        return Some(DirectDispatch {
            tool_name: "manage_playbook".into(),
            arguments: r#"{"action":"list"}"#.into(),
        });
    }

    // --- Browse directory (extract path) --------------------------------
    let path = None
        .or_else(|| q.strip_prefix("ls "))
        .or_else(|| q.strip_prefix("dir "))
        .or_else(|| q.strip_prefix("browse "))
        .or_else(|| q.strip_prefix("list directory "))
        .or_else(|| q.strip_prefix("list dir "));

    if let Some(raw_path) = path {
        let raw_path = raw_path.trim().trim_matches('"').trim_matches('\'');
        if !raw_path.is_empty() {
            let escaped =
                serde_json::to_string(raw_path).unwrap_or_else(|_| format!("\"{}\"", raw_path));
            return Some(DirectDispatch {
                tool_name: "list_dir".into(),
                arguments: format!(r#"{{"path":{}}}"#, escaped),
            });
        }
    }

    // --- List documents in source (resolve source name -> ID) ------------
    let source_phrase = None
        .or_else(|| q.strip_prefix("list files in "))
        .or_else(|| q.strip_prefix("show files in "))
        .or_else(|| q.strip_prefix("list documents in "))
        .or_else(|| q.strip_prefix("show documents in "))
        .or_else(|| q.strip_suffix("\u{91CC}\u{7684}\u{6587}\u{4EF6}")) // 里的文件
        .or_else(|| q.strip_suffix("\u{306E}\u{30D5}\u{30A1}\u{30A4}\u{30EB}")); // のファイル

    if let Some(source_name) = source_phrase {
        let source_name = source_name.trim().trim_matches('"').trim_matches('\'');
        if !source_name.is_empty() {
            if let Ok(sources) = db.list_sources() {
                let name_lower = source_name.to_lowercase();
                let matches: Vec<_> = sources
                    .iter()
                    .filter(|s| {
                        let root_lower = s.root_path.to_lowercase();
                        s.id == source_name
                            || root_lower.ends_with(&name_lower)
                            || root_lower.contains(&name_lower)
                    })
                    .collect();

                if matches.len() == 1 {
                    let source_id = serde_json::to_string(&matches[0].id).unwrap_or_default();
                    return Some(DirectDispatch {
                        tool_name: "list_documents".into(),
                        arguments: format!(r#"{{"source_id":{}}}"#, source_id),
                    });
                }
                // 0 or >1 matches -> ambiguous, fall through to LLM.
            }
        }
    }

    None
}
