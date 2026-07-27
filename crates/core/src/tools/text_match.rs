use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextMatchKind {
    Exact,
    LineEndingNormalized,
    IndentationNormalized,
    VisualNormalized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextMatch {
    pub start: usize,
    pub len: usize,
    pub kind: TextMatchKind,
    indentation_prefix: Option<String>,
}

impl TextMatch {
    pub(crate) fn replacement_text(&self, original: &str, replacement: &str) -> String {
        let replacement = match self.indentation_prefix.as_deref() {
            Some(prefix) => replacement
                .split_inclusive('\n')
                .map(|line| {
                    let body = line.trim_end_matches(['\r', '\n']);
                    if body.trim().is_empty() {
                        line.to_string()
                    } else {
                        format!("{prefix}{line}")
                    }
                })
                .collect(),
            None => replacement.to_string(),
        };

        if matches!(
            self.kind,
            TextMatchKind::LineEndingNormalized
                | TextMatchKind::IndentationNormalized
                | TextMatchKind::VisualNormalized
        ) && original.contains("\r\n")
            && replacement.contains('\n')
        {
            replacement.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            replacement
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NormalizationMode {
    LineEndings,
    Visual,
}

pub(crate) fn find_text_matches(haystack: &str, needle: &str) -> Vec<TextMatch> {
    if needle.is_empty() {
        return Vec::new();
    }

    let exact: Vec<TextMatch> = haystack
        .match_indices(needle)
        .map(|(start, matched)| TextMatch {
            start,
            len: matched.len(),
            kind: TextMatchKind::Exact,
            indentation_prefix: None,
        })
        .collect();
    if !exact.is_empty() {
        return exact;
    }

    let line_endings = find_normalized_matches(
        haystack,
        needle,
        NormalizationMode::LineEndings,
        TextMatchKind::LineEndingNormalized,
    );
    if !line_endings.is_empty() {
        return line_endings;
    }

    let indentation = find_uniform_indentation_matches(haystack, needle);
    if !indentation.is_empty() {
        return indentation;
    }

    find_normalized_matches(
        haystack,
        needle,
        NormalizationMode::Visual,
        TextMatchKind::VisualNormalized,
    )
}

fn find_normalized_matches(
    haystack: &str,
    needle: &str,
    mode: NormalizationMode,
    kind: TextMatchKind,
) -> Vec<TextMatch> {
    let (normalized_haystack, haystack_map) = normalize_with_map(haystack, mode);
    let (normalized_needle, _) = normalize_with_map(needle, mode);
    if normalized_needle.is_empty() {
        return Vec::new();
    }
    if normalized_haystack == haystack && normalized_needle == needle {
        return Vec::new();
    }

    let mut matches = Vec::new();
    for (start, matched) in normalized_haystack.match_indices(&normalized_needle) {
        let end = start + matched.len();
        let Some(original_start) = haystack_map.get(start).copied() else {
            continue;
        };
        let original_end = haystack_map.get(end).copied().unwrap_or(haystack.len());
        if original_end <= original_start {
            continue;
        }
        let item = TextMatch {
            start: original_start,
            len: original_end - original_start,
            kind,
            indentation_prefix: None,
        };
        if !matches
            .iter()
            .any(|existing: &TextMatch| existing.start == item.start && existing.len == item.len)
        {
            matches.push(item);
        }
    }

    matches
}

fn line_ranges(input: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for (index, byte) in input.bytes().enumerate() {
        if byte == b'\n' {
            ranges.push((start, index + 1));
            start = index + 1;
        }
    }
    if start < input.len() {
        ranges.push((start, input.len()));
    }
    ranges
}

fn leading_ascii_whitespace(value: &str) -> &str {
    let count = value
        .as_bytes()
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    &value[..count]
}

fn line_body(value: &str) -> &str {
    value.trim_end_matches(['\r', '\n'])
}

fn find_uniform_indentation_matches(haystack: &str, needle: &str) -> Vec<TextMatch> {
    let haystack_ranges = line_ranges(haystack);
    let needle_ranges = line_ranges(needle);
    if needle_ranges.is_empty() || needle_ranges.len() > haystack_ranges.len() {
        return Vec::new();
    }

    let needle_lines = needle_ranges
        .iter()
        .map(|(start, end)| &needle[*start..*end])
        .collect::<Vec<_>>();
    let mut matches = Vec::new();
    for window_start in 0..=haystack_ranges.len() - needle_ranges.len() {
        let mut shared_prefix: Option<&str> = None;
        let mut valid = true;
        for (offset, needle_line) in needle_lines.iter().enumerate() {
            let (start, end) = haystack_ranges[window_start + offset];
            let haystack_line = &haystack[start..end];
            if haystack_line.ends_with('\n') != needle_line.ends_with('\n') {
                valid = false;
                break;
            }
            let haystack_body = line_body(haystack_line);
            let needle_body = line_body(needle_line);
            let haystack_indent = leading_ascii_whitespace(haystack_body);
            let needle_indent = leading_ascii_whitespace(needle_body);
            if haystack_body[haystack_indent.len()..] != needle_body[needle_indent.len()..] {
                valid = false;
                break;
            }
            if haystack_body.trim().is_empty() {
                continue;
            }
            let Some(prefix) = haystack_indent.strip_suffix(needle_indent) else {
                valid = false;
                break;
            };
            match shared_prefix {
                Some(existing) if existing != prefix => {
                    valid = false;
                    break;
                }
                None => shared_prefix = Some(prefix),
                _ => {}
            }
        }
        let Some(prefix) = shared_prefix.filter(|prefix| !prefix.is_empty()) else {
            continue;
        };
        if valid {
            let start = haystack_ranges[window_start].0;
            let end = haystack_ranges[window_start + needle_ranges.len() - 1].1;
            matches.push(TextMatch {
                start,
                len: end - start,
                kind: TextMatchKind::IndentationNormalized,
                indentation_prefix: Some(prefix.to_string()),
            });
        }
    }
    matches
}

fn normalize_with_map(input: &str, mode: NormalizationMode) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(input.len());
    let mut byte_map = Vec::with_capacity(input.len() + 1);
    let mut chars = input.char_indices().peekable();

    while let Some((idx, ch)) = chars.next() {
        if ch == '\r' {
            if chars.peek().is_some_and(|(_, next)| *next == '\n') {
                chars.next();
            }
            push_mapped_char(&mut normalized, &mut byte_map, '\n', idx);
            continue;
        }

        match mode {
            NormalizationMode::LineEndings => {
                push_mapped_char(&mut normalized, &mut byte_map, ch, idx);
            }
            NormalizationMode::Visual => {
                push_visual_equivalent(&mut normalized, &mut byte_map, ch, idx);
            }
        }
    }

    byte_map.push(input.len());
    (normalized, byte_map)
}

fn push_mapped_char(
    normalized: &mut String,
    byte_map: &mut Vec<usize>,
    ch: char,
    original_idx: usize,
) {
    for _ in 0..ch.len_utf8() {
        byte_map.push(original_idx);
    }
    normalized.push(ch);
}

fn push_mapped_str(
    normalized: &mut String,
    byte_map: &mut Vec<usize>,
    text: &str,
    original_idx: usize,
) {
    for ch in text.chars() {
        push_mapped_char(normalized, byte_map, ch, original_idx);
    }
}

fn push_visual_equivalent(
    normalized: &mut String,
    byte_map: &mut Vec<usize>,
    ch: char,
    original_idx: usize,
) {
    let mut buffer = [0u8; 4];
    for decomposed in ch.encode_utf8(&mut buffer).nfkd() {
        match visual_fold(decomposed) {
            VisualFold::Skip => {}
            VisualFold::Char(folded) => {
                push_mapped_char(normalized, byte_map, folded, original_idx);
            }
            VisualFold::Str(folded) => {
                push_mapped_str(normalized, byte_map, folded, original_idx);
            }
        }
    }
}

enum VisualFold {
    Skip,
    Char(char),
    Str(&'static str),
}

fn visual_fold(ch: char) -> VisualFold {
    if ch != '\n' && ch.is_whitespace() {
        return VisualFold::Char(' ');
    }
    if is_ignorable_format_char(ch) {
        return VisualFold::Skip;
    }

    match ch {
        '“' | '”' | '„' | '‟' | '«' | '»' | '〝' | '〞' | '〟' | '「' | '」' | '『' | '』'
        | '﹁' | '﹂' | '﹃' | '﹄' => VisualFold::Char('"'),
        '‘' | '’' | '‚' | '‛' | '‹' | '›' | '′' | '＇' | '`' | '´' | 'ʼ' => {
            VisualFold::Char('\'')
        }
        '‐' | '‑' | '‒' | '–' | '—' | '―' | '−' | '﹘' | '﹣' => {
            VisualFold::Char('-')
        }
        '…' | '⋯' => VisualFold::Str("..."),
        '。' | '｡' => VisualFold::Char('.'),
        '、' | '､' | '،' => VisualFold::Char(','),
        '；' | '؛' => VisualFold::Char(';'),
        '：' => VisualFold::Char(':'),
        '？' | '؟' => VisualFold::Char('?'),
        '！' => VisualFold::Char('!'),
        _ => VisualFold::Char(ch),
    }
}

fn is_ignorable_format_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{E0100}'..='\u{E01EF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_line_ending_match_after_non_ascii_text() {
        let haystack = "第一行\r\n第二行\r\n第三行\r\n";
        let needle = "第二行\n第三行";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::LineEndingNormalized);
        assert_eq!(
            &haystack[matches[0].start..matches[0].start + matches[0].len],
            "第二行\r\n第三行"
        );
    }

    #[test]
    fn finds_visual_quote_equivalent_match() {
        let haystack = "她说：“我要出门了。”\n";
        let needle = "她说:\"我要出门了。\"";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::VisualNormalized);
        assert_eq!(
            &haystack[matches[0].start..matches[0].start + matches[0].len],
            "她说：“我要出门了。”"
        );
    }

    #[test]
    fn finds_canonical_equivalent_match_for_combining_marks() {
        let haystack = "Cafe\u{301} noir\n";
        let needle = "Café";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::VisualNormalized);
        assert_eq!(
            &haystack[matches[0].start..matches[0].start + matches[0].len],
            "Cafe\u{301}"
        );
    }

    #[test]
    fn finds_compatibility_match_for_japanese_halfwidth_katakana() {
        let haystack = "ﾊﾟﾝを買う\n";
        let needle = "パンを買う";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::VisualNormalized);
        assert_eq!(
            &haystack[matches[0].start..matches[0].start + matches[0].len],
            "ﾊﾟﾝを買う"
        );
    }

    #[test]
    fn finds_canonical_equivalent_match_for_decomposed_hangul() {
        let haystack = "\u{1112}\u{1161}\u{11AB}글\n";
        let needle = "한글";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::VisualNormalized);
        assert_eq!(
            &haystack[matches[0].start..matches[0].start + matches[0].len],
            "\u{1112}\u{1161}\u{11AB}글"
        );
    }

    #[test]
    fn finds_visual_quote_equivalent_match_in_arabic_text() {
        let haystack = "قال: «مرحبا»\n";
        let needle = "قال: \"مرحبا\"";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::VisualNormalized);
        assert_eq!(
            &haystack[matches[0].start..matches[0].start + matches[0].len],
            "قال: «مرحبا»"
        );
    }

    #[test]
    fn finds_and_reindents_uniformly_outdented_block() {
        let haystack = "fn main() {\n    if ready {\n        run();\n    }\n}\n";
        let needle = "if ready {\n    run();\n}\n";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::IndentationNormalized);
        assert_eq!(
            matches[0].replacement_text(
                &haystack[matches[0].start..matches[0].start + matches[0].len],
                "if ready {\n    finish();\n}\n"
            ),
            "    if ready {\n        finish();\n    }\n"
        );
    }

    #[test]
    fn recovered_replacement_preserves_crlf_without_doubling_carriage_returns() {
        let haystack = "fn main() {\r\n    if ready {\r\n        run();\r\n    }\r\n}\r\n";
        let needle = "if ready {\n    run();\n}\n";
        let matches = find_text_matches(haystack, needle);
        let matched = &matches[0];
        let original = &haystack[matched.start..matched.start + matched.len];

        assert_eq!(
            matched.replacement_text(original, "if ready {\r\n    finish();\r\n}\r\n"),
            "    if ready {\r\n        finish();\r\n    }\r\n"
        );
    }

    #[test]
    fn finds_unicode_space_equivalent_match_in_cyrillic_text() {
        let haystack = "добрый\u{00A0}день\n";
        let needle = "добрый день";

        let matches = find_text_matches(haystack, needle);

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].kind, TextMatchKind::VisualNormalized);
        assert_eq!(
            &haystack[matches[0].start..matches[0].start + matches[0].len],
            "добрый\u{00A0}день"
        );
    }
}
