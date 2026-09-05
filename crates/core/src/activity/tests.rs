use std::time::Duration;

use super::{ActivityEventKind, ActivityRuntime, ActivitySpec, ActivityState, ActivitySurface};
use crate::db::Database;

#[test]
fn action_receipts_can_distinguish_persistent_from_ephemeral_runtimes() {
    assert!(!ActivityRuntime::new().is_persistent());
    let persistent = ActivityRuntime::with_database(Database::open_memory().unwrap()).unwrap();
    assert!(persistent.is_persistent());
}

#[tokio::test]
async fn malformed_history_is_quarantined_without_disabling_new_durable_activities() {
    let db = Database::open_memory().unwrap();
    let runtime = ActivityRuntime::with_database(db.clone()).unwrap();
    for id in ["healthy", "broken-record", "broken-event"] {
        runtime
            .start(ActivitySpec::new(ActivitySurface::Process, "run_shell").with_activity_id(id))
            .unwrap();
    }
    runtime
        .transition("healthy", ActivityState::Completed, serde_json::json!({}))
        .unwrap();
    drop(runtime);
    db.conn().execute("UPDATE activity_records SET record_json = '{broken' WHERE activity_id = 'broken-record'",[]).unwrap();
    db.conn()
        .execute(
            "UPDATE activity_events SET event_json = '{broken' WHERE activity_id = 'broken-event'",
            [],
        )
        .unwrap();
    let recovered = ActivityRuntime::with_database(db.clone()).unwrap();
    assert!(recovered.is_persistent());
    assert_eq!(
        recovered.get("healthy").unwrap().state,
        ActivityState::Completed
    );
    for id in ["broken-record", "broken-event"] {
        assert!(recovered
            .observe(id, 0, Duration::ZERO)
            .await
            .unwrap_err()
            .to_string()
            .contains("unreadable"));
        assert!(recovered
            .start(ActivitySpec::new(ActivitySurface::Process, "run_shell").with_activity_id(id))
            .is_err());
    }
    recovered
        .start(
            ActivitySpec::new(ActivitySurface::Process, "run_shell").with_activity_id("new-chat"),
        )
        .unwrap();
    recovered
        .transition(
            "new-chat",
            ActivityState::Completed,
            serde_json::json!({"receipt":"durable"}),
        )
        .unwrap();
    drop(recovered);
    let next = ActivityRuntime::with_database(db.clone()).unwrap();
    assert_eq!(
        next.get("new-chat").unwrap().state,
        ActivityState::Completed
    );
    assert_eq!(
        db.conn()
            .query_row(
                "SELECT record_json FROM activity_records WHERE activity_id = 'broken-record'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "{broken"
    );
    assert_eq!(
        db.conn()
            .query_row(
                "SELECT event_json FROM activity_events WHERE activity_id = 'broken-event'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "{broken"
    );
}

#[test]
fn mismatched_journal_identity_is_not_replayed_under_another_activity() {
    let db = Database::open_memory().unwrap();
    let runtime = ActivityRuntime::with_database(db.clone()).unwrap();
    for id in ["source", "target"] {
        runtime
            .start(ActivitySpec::new(ActivitySurface::Process, "run_shell").with_activity_id(id))
            .unwrap();
    }
    drop(runtime);
    db.conn().execute("UPDATE activity_events SET event_json = json_set(event_json, '$.activityId', 'target') WHERE activity_id = 'source'",[]).unwrap();
    let recovered = ActivityRuntime::with_database(db).unwrap();
    assert!(recovered.get("source").is_none());
    assert_eq!(
        recovered.get("target").unwrap().state,
        ActivityState::Orphaned
    );
    assert!(recovered
        .start(ActivitySpec::new(ActivitySurface::Process, "run_shell").with_activity_id("source"))
        .is_err());
}

#[test]
fn storage_failures_still_prevent_an_ephemeral_runtime_from_being_reported_as_durable() {
    let db = Database::open_memory().unwrap();
    db.conn().execute("DROP TABLE activity_events", []).unwrap();
    assert!(ActivityRuntime::with_database(db).is_err());
}

#[tokio::test]
async fn activity_journal_uses_strictly_increasing_incremental_cursors() {
    let runtime = ActivityRuntime::new();
    let activity = runtime
        .start(
            ActivitySpec::new(ActivitySurface::Process, "run_shell")
                .with_conversation_id("conversation-1")
                .with_task_run_id("run-1"),
        )
        .expect("start activity");

    runtime
        .append(
            &activity.activity_id,
            ActivityEventKind::StdoutChunk,
            serde_json::json!({ "data": "first" }),
        )
        .expect("append first chunk");
    let first = runtime
        .observe(&activity.activity_id, 0, Duration::ZERO)
        .await
        .expect("observe first delta");
    assert_eq!(
        first
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    runtime
        .append(
            &activity.activity_id,
            ActivityEventKind::StderrChunk,
            serde_json::json!({ "data": "second" }),
        )
        .expect("append second chunk");
    let second = runtime
        .observe(&activity.activity_id, first.cursor, Duration::ZERO)
        .await
        .expect("observe second delta");
    assert_eq!(second.events.len(), 1);
    assert_eq!(second.events[0].seq, 3);
    assert_eq!(second.cursor, 3);
}

#[tokio::test]
async fn activity_observe_wakes_when_a_decision_worthy_event_arrives() {
    let runtime = ActivityRuntime::new();
    let activity = runtime
        .start(ActivitySpec::new(ActivitySurface::Process, "run_shell"))
        .expect("start activity");
    let cursor = activity.last_event_seq;
    let observer = runtime.clone();
    let activity_id = activity.activity_id.clone();
    let waiting = tokio::spawn(async move {
        observer
            .observe(&activity_id, cursor, Duration::from_secs(10))
            .await
            .expect("observe activity")
    });

    tokio::task::yield_now().await;
    runtime
        .transition(
            &activity.activity_id,
            ActivityState::Completed,
            serde_json::json!({ "exitCode": 0 }),
        )
        .expect("complete activity");

    let observed = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .expect("observer should wake promptly")
        .expect("observer task");
    assert_eq!(observed.record.state, ActivityState::Completed);
    assert!(observed
        .events
        .iter()
        .any(|event| event.kind == ActivityEventKind::Completed));
}

#[tokio::test]
async fn activity_observe_clamps_long_waits_to_a_short_quantum() {
    let runtime = ActivityRuntime::new();
    let activity = runtime
        .start(ActivitySpec::new(
            ActivitySurface::Browser,
            "browser_session",
        ))
        .expect("start activity");
    let started = std::time::Instant::now();

    let observed = runtime
        .observe(
            &activity.activity_id,
            activity.last_event_seq,
            Duration::from_secs(30),
        )
        .await
        .expect("observe activity");

    assert_eq!(observed.record.state, ActivityState::Running);
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn persisted_unfinished_activities_recover_as_orphaned() {
    let db = Database::open_memory().expect("database");
    let runtime = ActivityRuntime::with_database(db.clone()).expect("activity runtime");
    let activity = runtime
        .start(
            ActivitySpec::new(ActivitySurface::Terminal, "terminal_session")
                .with_session_id("terminal-1"),
        )
        .expect("start activity");
    drop(runtime);

    let recovered = ActivityRuntime::with_database(db).expect("recover activity runtime");
    let observed = recovered
        .observe(&activity.activity_id, 0, Duration::ZERO)
        .await
        .expect("observe recovered activity");

    assert_eq!(observed.record.state, ActivityState::Orphaned);
    assert!(observed
        .events
        .iter()
        .any(|event| event.kind == ActivityEventKind::StateChanged));
}

#[tokio::test]
async fn file_backed_runtime_is_shared_while_live_activities_are_owned() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::new(dir.path().join("activity.db")).expect("database");
    let owner = ActivityRuntime::with_database(db.clone()).expect("activity runtime");
    let activity = owner
        .start(ActivitySpec::new(ActivitySurface::Process, "run_shell"))
        .expect("start activity");

    let next_turn = ActivityRuntime::with_database(db).expect("reuse activity runtime");
    let observed = next_turn
        .observe(&activity.activity_id, 0, Duration::ZERO)
        .await
        .expect("observe live activity");

    assert_eq!(observed.record.state, ActivityState::Running);
}

#[tokio::test]
async fn subscribers_receive_activity_events_as_they_are_appended() {
    let runtime = ActivityRuntime::new();
    let mut events = runtime.subscribe();
    let record = runtime
        .start(ActivitySpec::new(ActivitySurface::Process, "run_shell"))
        .unwrap();
    runtime
        .append(
            &record.activity_id,
            ActivityEventKind::StdoutChunk,
            serde_json::json!({ "data": "hello" }),
        )
        .unwrap();

    let started = events.recv().await.unwrap();
    let stdout = events.recv().await.unwrap();
    assert_eq!(started.seq, 1);
    assert_eq!(stdout.seq, 2);
    assert_eq!(stdout.kind, ActivityEventKind::StdoutChunk);
}

#[tokio::test]
async fn activity_journal_bounds_replay_without_resetting_the_cursor() {
    let runtime = ActivityRuntime::new();
    let record = runtime
        .start(ActivitySpec::new(ActivitySurface::Process, "run_shell"))
        .expect("start activity");
    for index in 0..2_055 {
        runtime
            .append(
                &record.activity_id,
                ActivityEventKind::StdoutChunk,
                serde_json::json!({ "data": index.to_string() }),
            )
            .expect("append output");
    }

    let observed = runtime
        .observe(&record.activity_id, 0, Duration::ZERO)
        .await
        .expect("observe bounded replay");
    assert_eq!(observed.events.len(), 2_048);
    assert_eq!(observed.cursor, 2_056);
    assert_eq!(observed.events.first().map(|event| event.seq), Some(9));
}
