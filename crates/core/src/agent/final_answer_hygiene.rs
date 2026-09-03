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

fn line_starts_with_marker(line: &str, marker: &str) -> bool {
    let visible_line = line.trim_start().trim_start_matches('#').trim_start();
    visible_line
        .get(..marker.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(marker))
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
