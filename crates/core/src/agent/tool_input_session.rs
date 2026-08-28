//! Bounded projection policy for streamed tool-call input.
//!
//! Providers own the byte-by-byte argument stream. Consumers should observe
//! coarse progress snapshots instead of reparsing and rebuilding a cumulative
//! tool preview for every fragment. This session is the single policy seam
//! between lossless provider assembly and best-effort UI projection.

use std::collections::HashMap;

const TOOL_INPUT_PREVIEW_BUCKET_BYTES: usize = 2 * 1024;

#[derive(Debug, Default)]
pub(super) struct ToolInputSession {
    projected_buckets: HashMap<String, usize>,
}

impl ToolInputSession {
    /// Return `true` for the first observation and whenever cumulative input
    /// crosses another fixed-size bucket. Provider assembly remains lossless;
    /// only the diagnostic preview is sampled.
    pub(super) fn should_project(&mut self, call_id: &str, received_bytes: usize) -> bool {
        let bucket = received_bytes / TOOL_INPUT_PREVIEW_BUCKET_BYTES;
        match self.projected_buckets.get_mut(call_id) {
            Some(last_bucket) if bucket <= *last_bucket => false,
            Some(last_bucket) => {
                *last_bucket = bucket;
                true
            }
            None => {
                self.projected_buckets.insert(call_id.to_string(), bucket);
                true
            }
        }
    }

    pub(super) fn reset(&mut self) {
        self.projected_buckets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cumulative_fragments_project_once_per_progress_bucket() {
        let mut session = ToolInputSession::default();
        let call_id = "call-large-file";
        let mut projections = 0;

        for received_bytes in (16..=80_000).step_by(16) {
            projections += usize::from(session.should_project(call_id, received_bytes));
        }

        assert!(
            projections <= 41,
            "unexpected projection count: {projections}"
        );
        assert!(
            projections >= 39,
            "progress buckets were skipped: {projections}"
        );
    }

    #[test]
    fn reset_isolates_retried_model_samples() {
        let mut session = ToolInputSession::default();
        assert!(session.should_project("call-1", 10));
        assert!(!session.should_project("call-1", 20));

        session.reset();

        assert!(session.should_project("call-1", 20));
    }
}
