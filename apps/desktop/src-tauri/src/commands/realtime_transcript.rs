//! Ordered transcript authority for a microphone session with multiple utterances.

#[derive(Default)]
pub(super) struct RealtimeTranscript {
    items: Vec<TranscriptItem>,
}

struct TranscriptItem {
    id: String,
    text: String,
    finalized: bool,
}

impl RealtimeTranscript {
    pub(super) fn update(&mut self, id: Option<&str>, text: &str, append: bool, finalized: bool) {
        let id = id.unwrap_or("session");
        let index = self
            .items
            .iter()
            .position(|item| item.id == id)
            .unwrap_or_else(|| {
                self.items.push(TranscriptItem {
                    id: id.to_string(),
                    text: String::new(),
                    finalized: false,
                });
                self.items.len() - 1
            });
        let item = &mut self.items[index];
        if item.finalized && !finalized {
            return;
        }
        if append {
            item.text.push_str(text);
        } else {
            item.text = text.to_string();
        }
        item.finalized = finalized;
    }

    pub(super) fn snapshot(&self) -> String {
        self.items
            .iter()
            .map(|item| item.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(super) fn finish(&self) -> Result<String, String> {
        if self
            .items
            .iter()
            .any(|item| !item.finalized && !item.text.trim().is_empty())
        {
            return Err(
                "Realtime transcription finished before all utterances were finalized".to_string(),
            );
        }
        Ok(self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_preserves_earlier_utterances_and_corrects_each_snapshot() {
        let mut transcript = RealtimeTranscript::default();
        transcript.update(Some("a"), "第一具", false, false);
        transcript.update(Some("a"), "第一句。", false, true);
        transcript.update(Some("b"), "第二", false, false);
        assert_eq!(transcript.snapshot(), "第一句。 第二");
        transcript.update(Some("b"), "第二句。", false, true);
        transcript.update(Some("a"), "第一句。", false, true);
        assert_eq!(transcript.finish().unwrap(), "第一句。 第二句。");
    }

    #[test]
    fn finished_session_cannot_truncate_unfinalized_last_utterance() {
        let mut transcript = RealtimeTranscript::default();
        transcript.update(Some("a"), "first", false, true);
        transcript.update(Some("b"), "last", false, false);
        assert!(transcript.finish().is_err());
        transcript.update(Some("b"), "", false, true);
        assert_eq!(transcript.finish().unwrap(), "first");
    }

    #[test]
    fn late_interim_cannot_replace_final_text_and_silence_is_empty() {
        let mut transcript = RealtimeTranscript::default();
        assert_eq!(transcript.finish().unwrap(), "");
        transcript.update(None, "hello", true, false);
        transcript.update(None, " world", true, false);
        assert_eq!(transcript.snapshot(), "hello world");
        transcript.update(None, "Hello world.", false, true);
        transcript.update(None, "bad late text", false, false);
        assert_eq!(transcript.finish().unwrap(), "Hello world.");
    }
}
