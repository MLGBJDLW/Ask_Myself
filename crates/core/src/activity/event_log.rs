use std::collections::VecDeque;

use super::{ActivityEvent, ActivityRecord};

pub(crate) const DEFAULT_MAX_EVENTS_PER_ACTIVITY: usize = 2_048;

pub(crate) struct ActivityEntry {
    pub(crate) record: ActivityRecord,
    pub(crate) events: VecDeque<ActivityEvent>,
}

impl ActivityEntry {
    pub(crate) fn new(record: ActivityRecord) -> Self {
        Self {
            record,
            events: VecDeque::new(),
        }
    }

    pub(crate) fn push(&mut self, event: ActivityEvent, max_events: usize) {
        self.record.last_event_seq = event.seq;
        self.record.updated_at = event.timestamp;
        self.events.push_back(event);
        while self.events.len() > max_events.max(1) {
            self.events.pop_front();
        }
    }

    pub(crate) fn events_after(&self, after_seq: u64) -> Vec<ActivityEvent> {
        self.events
            .iter()
            .filter(|event| event.seq > after_seq)
            .cloned()
            .collect()
    }
}
