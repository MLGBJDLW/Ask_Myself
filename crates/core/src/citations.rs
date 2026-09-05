//! Citation references shared by evidence verification and workflow requirements.

/// Extract `[cite:CHUNK_ID]` references from an answer text.
pub fn extract_citations(text: &str) -> Vec<String> {
    let mut citations = Vec::new();
    let mut pos = 0;
    while let Some(start) = text[pos..].find("[cite:") {
        let abs_start = pos + start + 6; // skip "[cite:"
        if abs_start >= text.len() {
            break;
        }
        if let Some(end) = text[abs_start..].find(']') {
            let inner = &text[abs_start..abs_start + end];
            // inner might be "UUID" or "UUID|description"
            let uuid_part = inner.split('|').next().unwrap_or(inner).trim();
            if !uuid_part.is_empty() && !citations.contains(&uuid_part.to_string()) {
                citations.push(uuid_part.to_string());
            }
            pos = abs_start + end + 1;
        } else {
            break;
        }
    }
    citations
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_extract_citations() {
        let text = "Based on [cite:abc-123] and [cite:def-456|some desc], the answer is...";
        let cites = extract_citations(text);
        assert_eq!(cites, vec!["abc-123", "def-456"]);
    }

    #[test]
    fn test_extract_citations_dedup() {
        let text = "See [cite:abc-123] and again [cite:abc-123].";
        let cites = extract_citations(text);
        assert_eq!(cites, vec!["abc-123"]);
    }

    #[test]
    fn test_extract_citations_empty() {
        let text = "No citations here.";
        let cites = extract_citations(text);
        assert!(cites.is_empty());
    }
}
