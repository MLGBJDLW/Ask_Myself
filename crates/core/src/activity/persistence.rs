use std::collections::{HashMap, VecDeque};

use rusqlite::params;

use super::event_log::{ActivityEntry, DEFAULT_MAX_EVENTS_PER_ACTIVITY};
use super::{ActivityEvent, ActivityRecord};
use crate::db::Database;
use crate::error::CoreError;

pub(crate) fn persist_record(db: &Database, record: &ActivityRecord) -> Result<(), CoreError> {
    let record_json = serde_json::to_string(record)?;
    db.conn().execute(
        "INSERT INTO activity_records (
            activity_id, state, conversation_id, task_run_id, updated_at, record_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(activity_id) DO UPDATE SET
            state = excluded.state,
            conversation_id = excluded.conversation_id,
            task_run_id = excluded.task_run_id,
            updated_at = excluded.updated_at,
            record_json = excluded.record_json",
        params![
            record.activity_id,
            format!("{:?}", record.state).to_ascii_lowercase(),
            record.conversation_id,
            record.task_run_id,
            record.updated_at.to_rfc3339(),
            record_json,
        ],
    )?;
    Ok(())
}

pub(crate) fn persist_event(db: &Database, event: &ActivityEvent) -> Result<(), CoreError> {
    let event_json = serde_json::to_string(event)?;
    let conn = db.conn();
    conn.execute(
        "INSERT OR IGNORE INTO activity_events (
            activity_id, seq, kind, timestamp, event_json
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            event.activity_id,
            event.seq as i64,
            format!("{:?}", event.kind).to_ascii_lowercase(),
            event.timestamp.to_rfc3339(),
            event_json,
        ],
    )?;
    let oldest_retained_seq = event
        .seq
        .saturating_sub(DEFAULT_MAX_EVENTS_PER_ACTIVITY as u64);
    if oldest_retained_seq > 0 {
        conn.execute(
            "DELETE FROM activity_events WHERE activity_id = ?1 AND seq <= ?2",
            params![event.activity_id, oldest_retained_seq as i64],
        )?;
    }
    Ok(())
}

pub(crate) fn load_entries(db: &Database) -> Result<HashMap<String, ActivityEntry>, CoreError> {
    let conn = db.conn();
    let mut records_stmt =
        conn.prepare("SELECT record_json FROM activity_records ORDER BY updated_at, activity_id")?;
    let records = records_stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut entries = HashMap::new();
    for record_json in records {
        let record: ActivityRecord = serde_json::from_str(&record_json)?;
        entries.insert(record.activity_id.clone(), ActivityEntry::new(record));
    }

    let mut events_stmt =
        conn.prepare("SELECT event_json FROM activity_events ORDER BY activity_id, seq")?;
    let events = events_stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for event_json in events {
        let event: ActivityEvent = serde_json::from_str(&event_json)?;
        if let Some(entry) = entries.get_mut(&event.activity_id) {
            entry.events.push_back(event);
        }
    }
    for entry in entries.values_mut() {
        let expected = entry.record.last_event_seq;
        entry.events = std::mem::take(&mut entry.events)
            .into_iter()
            .filter(|event| event.seq <= expected)
            .rev()
            .take(DEFAULT_MAX_EVENTS_PER_ACTIVITY)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<VecDeque<_>>();
    }
    Ok(entries)
}
