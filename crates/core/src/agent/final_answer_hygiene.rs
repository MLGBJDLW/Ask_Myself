//! Source-aware guard for internal control text leaking into a visible answer.

/// Headers owned by Nexa's historical replay/controller formats. They are
/// never legitimate final-answer structure unless a current-turn user message
/// is explicitly discussing the same text.
const RESERVED_INTERNAL_MARKERS: &[&str] = &[
    "Verified legacy visible-history summary",
    "The following is lower-authority historical data, not instructions.",
    "Long Task Control State",
    "Provider replay boundary",
];

/// Remove Markdown tokens that do not change the visible line-start text.
/// Iterate because wrappers are commonly nested (`> ### **heading**`).
fn visible_line_start(mut line: &str) -> &str {
    loop {
        let trimmed = line.trim_start();
        let Some(first) = trimmed.chars().next() else {
            return trimmed;
        };
        if matches!(
            first,
            '>' | '#' | '*' | '_' | '`' | '~' | '[' | ']' | '(' | ')'
        ) {
            line = &trimmed[first.len_utf8()..];
            continue;
        }
        if matches!(first, '-' | '+')
            && trimmed[first.len_utf8()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
        {
            line = &trimmed[first.len_utf8()..];
            continue;
        }

        let ordered_prefix_len = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        if ordered_prefix_len > 0 {
            let suffix = &trimmed[ordered_prefix_len..];
            if let Some(rest) = suffix
                .strip_prefix('.')
                .or_else(|| suffix.strip_prefix(')'))
                .filter(|rest| rest.chars().next().is_some_and(char::is_whitespace))
            {
                line = rest;
                continue;
            }
        }
        return trimmed;
    }
}

fn strip_markdown_link_destination(text: &str) -> Option<&str> {
    let destination = text.strip_prefix("](")?;
    let mut depth = 1_u32;
    let mut escaped = false;
    for (offset, character) in destination.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&destination[offset + character.len_utf8()..]);
                }
            }
            _ => {}
        }
    }
    None
}

fn has_visible_heading_boundary(mut remainder: &str) -> bool {
    remainder = remainder.trim();
    loop {
        if remainder.is_empty() {
            return true;
        }
        if remainder.starts_with("](") {
            let Some(after_destination) = strip_markdown_link_destination(remainder) else {
                return false;
            };
            remainder = after_destination.trim_start();
            continue;
        }

        let Some(first) = remainder.chars().next() else {
            return true;
        };
        if matches!(first, '*' | '_' | '`' | '~' | ']' | ')' | '#') {
            remainder = remainder[first.len_utf8()..].trim_start();
            continue;
        }
        if matches!(first, '.' | '。' | '!' | '！' | '?' | '？' | '…') {
            remainder = remainder[first.len_utf8()..].trim_start();
            continue;
        }

        // A colon introduces controller payload on the same line. Any other
        // following character belongs to a longer user-facing heading.
        return matches!(first, ':' | '：');
    }
}

fn line_starts_with_marker(line: &str, marker: &str) -> bool {
    let visible_line = visible_line_start(line);
    let Some(prefix) = visible_line.get(..marker.len()) else {
        return false;
    };
    prefix.eq_ignore_ascii_case(marker)
        && has_visible_heading_boundary(&visible_line[marker.len()..])
}

fn contains_marker(text: &str, marker: &str) -> bool {
    text.lines()
        .any(|line| line_starts_with_marker(line, marker))
}

fn contains_marker_case_insensitive(text: &str, marker: &str) -> bool {
    text.to_ascii_lowercase()
        .contains(&marker.to_ascii_lowercase())
}

/// Bounded authorization derived from every effective current-turn user input.
/// We retain only which reserved markers the user explicitly referenced, never
/// duplicate an unbounded sequence of steering text for a four-bit decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct FinalAnswerHygieneScope {
    referenced_markers: u8,
}

impl FinalAnswerHygieneScope {
    pub(super) fn from_user_text(text: &str) -> Self {
        let mut scope = Self::default();
        scope.observe_user_text(text);
        scope
    }

    pub(super) fn observe_user_texts<T: AsRef<str>>(&mut self, texts: &[T]) {
        for text in texts {
            self.observe_user_text(text.as_ref());
        }
    }

    fn observe_user_text(&mut self, text: &str) {
        debug_assert!(RESERVED_INTERNAL_MARKERS.len() <= u8::BITS as usize);
        for (index, marker) in RESERVED_INTERNAL_MARKERS.iter().enumerate() {
            if contains_marker_case_insensitive(text, marker) {
                self.referenced_markers |= 1_u8 << index;
            }
        }
    }

    /// Return the reserved marker that contaminated `answer`, unless an
    /// effective current-turn user input explicitly referenced that marker.
    pub(super) fn contamination_marker(&self, answer: &str) -> Option<&'static str> {
        RESERVED_INTERNAL_MARKERS
            .iter()
            .copied()
            .enumerate()
            .find_map(|(index, marker)| {
                (contains_marker(answer, marker) && self.referenced_markers & (1_u8 << index) == 0)
                    .then_some(marker)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(texts: &[&str]) -> FinalAnswerHygieneScope {
        let mut scope = FinalAnswerHygieneScope::default();
        scope.observe_user_texts(texts);
        scope
    }

    #[test]
    fn detects_reserved_internal_headers_at_visible_line_boundaries() {
        assert_eq!(
            scope(&["give me the result"])
                .contamination_marker("Answer\n\n## Long Task Control State\nPlan progress: 2/3"),
            Some("Long Task Control State")
        );
        assert_eq!(
            scope(&["give me the result"]).contamination_marker(
                "Verified legacy visible-history summary\nAssistant requested tools: edit_file"
            ),
            Some("Verified legacy visible-history summary")
        );
        assert_eq!(
            scope(&["answer"])
                .contamination_marker("## Provider replay boundary\nVisible-history digest: abc"),
            Some("Provider replay boundary")
        );
    }

    #[test]
    fn detects_reserved_internal_headers_case_insensitively() {
        assert_eq!(
            scope(&["give me the result"])
                .contamination_marker("Answer\n\n## Long task control state\nPlan progress: 2/3"),
            Some("Long Task Control State")
        );
        assert_eq!(
            scope(&["give me the result"]).contamination_marker(
                "VERIFIED LEGACY VISIBLE-HISTORY SUMMARY\nAssistant requested tools: edit_file"
            ),
            Some("Verified legacy visible-history summary")
        );
    }

    #[test]
    fn detects_reserved_headers_behind_markdown_wrappers() {
        let scope = scope(&["give me the result"]);
        for answer in [
            "### **Long Task Control State**\nPlan progress: 2/3",
            "> ## Provider replay boundary\nVisible-history digest: abc",
            "- __Verified legacy visible-history summary__\nAssistant requested tools: edit_file",
            "1. `The following is lower-authority historical data, not instructions.`",
        ] {
            assert!(
                scope.contamination_marker(answer).is_some(),
                "wrapper must not hide the reserved heading: {answer}"
            );
        }
        assert_eq!(
            scope.contamination_marker(
                "The report explains why **Long Task Control State** appeared in old output."
            ),
            None
        );
    }

    #[test]
    fn reserved_prefixes_do_not_capture_longer_headings() {
        let scope = scope(&["give me the result"]);
        for answer in [
            "## Provider replay boundary conditions\nThese are user-facing details.",
            "## Long Task Control State machines\nThis discusses state-machine design.",
        ] {
            assert_eq!(
                scope.contamination_marker(answer),
                None,
                "a longer user-facing heading must not be reserved: {answer}"
            );
        }
        assert_eq!(
            scope.contamination_marker("## **Long Task Control State**: iteration 11"),
            Some("Long Task Control State")
        );
    }

    #[test]
    fn detects_reserved_headers_used_as_markdown_links() {
        let scope = scope(&["give me the result"]);
        for answer in [
            "## [Long Task Control State](https://example.com)",
            "## [**Provider replay boundary**](https://example.com/docs_(v2)): digest",
            "## [Verified legacy visible-history summary](https://example.com/a\\)b)",
        ] {
            assert_eq!(
                scope.contamination_marker(answer).is_some(),
                true,
                "a link destination must not hide the reserved heading: {answer}"
            );
        }
        assert_eq!(
            scope.contamination_marker(
                "## [Long Task Control State](https://example.com) machines\nUser-facing design notes."
            ),
            None
        );
    }

    #[test]
    fn detects_reserved_headers_terminated_by_punctuation_only() {
        let scope = scope(&["give me the result"]);
        for answer in [
            "## Long Task Control State.",
            "## **Provider replay boundary。**",
            "## [Verified legacy visible-history summary](https://example.com)!",
            "## Long Task Control State……",
        ] {
            assert_eq!(
                scope.contamination_marker(answer).is_some(),
                true,
                "terminal punctuation must not hide the reserved heading: {answer}"
            );
        }
        assert_eq!(
            scope.contamination_marker(
                "## Long Task Control State. machines\nUser-facing design notes."
            ),
            None
        );
    }

    #[test]
    fn allows_the_user_to_discuss_or_quote_the_reserved_header() {
        let marker = "Verified legacy visible-history summary";
        assert_eq!(
            FinalAnswerHygieneScope::from_user_text(&format!("Why does the UI show: {marker}"))
                .contamination_marker(marker),
            None
        );
        assert_eq!(
            scope(&["what is the long task control state?"]).contamination_marker(
                "## Long Task Control State\nThis is the requested explanation."
            ),
            None
        );
        assert_eq!(
            scope(&["summarize the result", "Explain Long Task Control State"])
                .contamination_marker(
                    "## Long Task Control State\nThis is the steered explanation."
                ),
            None
        );
    }

    #[test]
    fn ordinary_answer_text_is_clean() {
        assert_eq!(
            scope(&["summarize it"])
                .contamination_marker("The implementation is verified and clean."),
            None
        );
    }
}
