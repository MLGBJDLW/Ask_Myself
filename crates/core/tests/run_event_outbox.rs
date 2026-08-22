use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nexa_core::agent::{AgentEvent, StreamBlockChannel};
use nexa_core::agent_run::{AgentRunEvent, AgentRunPhase};
use nexa_core::conversation::{AgentTaskRun, ConversationMessage, CreateConversationInput};
use nexa_core::db::Database;
use nexa_core::db_executor::DatabaseExecutor;
use nexa_core::interaction::{
    CreateInteractionRequest, InteractionKind, InteractionQuestion, InteractionQuestionKind,
    SubmitInteractionResponse,
};
use nexa_core::llm::Role;
use nexa_core::run_event_outbox::{
    AgentRunEventDelivery, AgentRunEventOutboxFailure, AgentRunEventOutboxes,
    AgentRunEventSubmitError,
};

struct CaptureDelivery {
    database: Database,
    delivered: Mutex<Vec<(u64, bool)>>,
    events: Mutex<Vec<AgentRunEvent>>,
    pause_boundaries: Mutex<Vec<bool>>,
    terminal_task_statuses: Mutex<Vec<String>>,
    notification: tokio::sync::Notify,
}

impl CaptureDelivery {
    fn new(database: Database) -> Self {
        Self {
            database,
            delivered: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            pause_boundaries: Mutex::new(Vec::new()),
            terminal_task_statuses: Mutex::new(Vec::new()),
            notification: tokio::sync::Notify::new(),
        }
    }
}

impl AgentRunEventDelivery for CaptureDelivery {
    fn deliver_run_event(&self, _conversation_id: &str, event: &AgentRunEvent) {
        let committed = self
            .database
            .list_agent_run_events(&event.run_id)
            .expect("delivery can inspect the durable ledger")
            .iter()
            .any(|stored| stored.event_seq == event.event_seq);
        self.delivered
            .lock()
            .expect("capture lock")
            .push((event.event_seq, committed));
        self.events
            .lock()
            .expect("capture lock")
            .push(event.clone());
        if event.phase == AgentRunPhase::Paused {
            let checkpoint_committed = event
                .payload
                .get("checkpointId")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|checkpoint_id| {
                    self.database
                        .get_task_resume_checkpoint(checkpoint_id)
                        .is_ok()
                });
            let task_paused = self
                .database
                .get_agent_task_run(&event.run_id)
                .is_ok_and(|run| run.status == "paused" && run.phase == "paused");
            let turn_paused = self
                .database
                .get_conversation_turn(&event.turn_id)
                .is_ok_and(|turn| turn.status == "paused" && turn.finished_at.is_none());
            self.pause_boundaries
                .lock()
                .expect("capture lock")
                .push(committed && checkpoint_committed && task_paused && turn_paused);
        }
        if event.is_terminal() && event.is_durable() {
            self.terminal_task_statuses
                .lock()
                .expect("capture lock")
                .push(
                    self.database
                        .get_agent_task_run(&event.run_id)
                        .expect("terminal delivery can inspect the task projection")
                        .status,
                );
        }
        self.notification.notify_one();
    }

    fn deliver_task_run_snapshot(&self, _conversation_id: &str, _snapshot: AgentTaskRun) {}
}

fn create_started_run(database: &Database) -> (String, String, String) {
    let conversation = database
        .create_conversation(&CreateConversationInput {
            provider: "test".to_string(),
            model: "test-model".to_string(),
            system_prompt: None,
            collection_context: None,
            project_id: None,
            persona_id: None,
        })
        .expect("conversation");
    let message = ConversationMessage {
        id: "message-1".to_string(),
        conversation_id: conversation.id.clone(),
        role: Role::User,
        content: "Run the task".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 3,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    database.add_message(&message).expect("user message");
    let turn = database
        .create_conversation_turn(&conversation.id, &message.id, None)
        .expect("conversation turn");
    let run = database
        .create_agent_task_run(
            &conversation.id,
            &turn.id,
            &message.id,
            "Run the task",
            Some("test"),
            Some("test-model"),
        )
        .expect("task run");
    database
        .mark_agent_task_run_started(&run.id, "responding")
        .expect("started task run");
    (conversation.id, turn.id, run.id)
}

#[tokio::test(start_paused = true)]
async fn durable_run_events_are_committed_before_delivery() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery.clone());
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");

    outbox
        .submit(AgentRunEvent::output_delta(
            &run_id,
            Some(&turn_id),
            0,
            "answer-1",
            StreamBlockChannel::Answer,
            0,
            "hello",
        ))
        .expect("queued output delta");

    assert!(delivery.delivered.lock().expect("capture lock").is_empty());
    tokio::time::advance(Duration::from_millis(100)).await;
    delivery.notification.notified().await;

    assert_eq!(
        *delivery.delivered.lock().expect("capture lock"),
        vec![(1, true)]
    );
}

#[tokio::test]
async fn pause_checkpoint_event_and_projections_commit_as_one_boundary() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery.clone());
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");

    let checkpoint = outbox
        .pause_with_checkpoint(&turn_id, "user_pause")
        .await
        .expect("atomic pause checkpoint");

    assert_eq!(checkpoint.run_id, run_id);
    assert_eq!(checkpoint.reason, "user_pause");
    assert_eq!(
        database
            .list_task_resume_checkpoints(&run_id)
            .expect("durable checkpoints")
            .len(),
        1
    );
    let events = database
        .list_agent_run_events(&run_id)
        .expect("durable run ledger");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].phase, AgentRunPhase::Paused);
    assert_eq!(events[0].payload["checkpointId"], checkpoint.id);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("paused task projection")
            .status,
        "paused"
    );
    assert_eq!(
        database
            .get_conversation_turn(&turn_id)
            .expect("paused turn projection")
            .status,
        "paused"
    );
    assert_eq!(
        delivery
            .pause_boundaries
            .lock()
            .expect("capture lock")
            .as_slice(),
        &[true],
        "delivery must observe the checkpoint, event, task, and turn after commit"
    );

    assert_eq!(
        outbox.submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Responding,
            "late producer output",
            Some("running"),
            None,
        )),
        Err(nexa_core::run_event_outbox::AgentRunEventSubmitError::Suspended),
        "the old producer must stay fenced after the checkpoint commits"
    );
    outbox
        .resume_submissions()
        .expect("checkpoint relaunch reopens submissions");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Responding,
            "Resumed from checkpoint",
            Some("running"),
            None,
        ))
        .expect("new producer can submit after resume");
    assert_eq!(outbox.flush().await.expect("resume committed"), 2);
}

#[tokio::test]
async fn terminal_completion_follows_the_committed_task_projection() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery.clone());
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");

    outbox
        .submit(AgentRunEvent::terminal_status(
            &run_id,
            Some(&turn_id),
            0,
            "Final answer produced",
            "completed",
            None,
        ))
        .expect("queued terminal event");

    let completion = outbox
        .wait_for_terminal_commit()
        .await
        .expect("terminal completion");

    assert_eq!(completion.event_seq, 1);
    assert_eq!(
        *delivery
            .terminal_task_statuses
            .lock()
            .expect("capture lock"),
        vec!["completed".to_string()]
    );
}

#[tokio::test]
async fn resumable_pause_keeps_the_same_run_outbox_open() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery.clone());
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");

    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Paused,
            "Paused with a resumable checkpoint",
            Some("paused"),
            Some(&serde_json::json!({ "checkpointId": "checkpoint-1" })),
        ))
        .expect("queued resumable pause");
    assert_eq!(outbox.flush().await.expect("paused status committed"), 1);

    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("paused task projection")
            .status,
        "paused"
    );
    assert!(!outbox.is_closed_for_submission());
    let reopened = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("same run outbox");
    assert!(Arc::ptr_eq(&outbox, &reopened));

    reopened
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Responding,
            "Resumed from checkpoint",
            Some("running"),
            None,
        ))
        .expect("queued resume status");
    assert_eq!(reopened.flush().await.expect("resume status committed"), 2);

    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("resumed task projection")
            .status,
        "running"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_terminal_candidate_wins_with_a_named_closed_result() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery);
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    let barrier = Arc::new(std::sync::Barrier::new(3));

    let completed = {
        let outbox = Arc::clone(&outbox);
        let barrier = Arc::clone(&barrier);
        let run_id = run_id.clone();
        let turn_id = turn_id.clone();
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            outbox.submit(AgentRunEvent::terminal_status(
                &run_id,
                Some(&turn_id),
                0,
                "Completed",
                "completed",
                None,
            ))
        })
    };
    let timed_out = {
        let outbox = Arc::clone(&outbox);
        let barrier = Arc::clone(&barrier);
        let run_id = run_id.clone();
        let turn_id = turn_id.clone();
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            outbox.submit(AgentRunEvent::terminal_error(
                &run_id,
                Some(&turn_id),
                0,
                "Timed out",
                "timed_out",
                None,
            ))
        })
    };
    barrier.wait();

    let results = [
        completed.await.expect("completed candidate"),
        timed_out.await.expect("timeout candidate"),
    ];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(AgentRunEventSubmitError::AlreadyClosed)))
            .count(),
        1
    );

    outbox
        .wait_for_terminal_commit()
        .await
        .expect("winning terminal committed");
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run events")
            .iter()
            .filter(|event| event.is_terminal())
            .count(),
        1
    );
}

#[tokio::test]
async fn reopening_a_terminal_run_preserves_its_closed_outbox() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database));
    let outboxes = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    outbox
        .submit(AgentRunEvent::terminal_status(
            &run_id,
            Some(&turn_id),
            0,
            "Completed",
            "completed",
            None,
        ))
        .expect("queued terminal event");
    outbox
        .wait_for_terminal_commit()
        .await
        .expect("terminal completion");
    drop(outbox);
    drop(outboxes);

    let restarted = AgentRunEventOutboxes::new(executor, delivery);
    let reopened = restarted
        .open(&conversation_id, &run_id)
        .await
        .expect("restored terminal outbox");

    assert!(reopened.is_closed_for_submission());
    assert_eq!(
        reopened.submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Responding,
            "late",
            Some("running"),
            None,
        )),
        Err(AgentRunEventSubmitError::AlreadyClosed)
    );
}

#[tokio::test]
async fn historical_error_with_paused_status_still_closes_the_run() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    database
        .save_agent_run_event(&AgentRunEvent::terminal_error(
            &run_id,
            Some(&turn_id),
            1,
            "Historical failure mislabeled as paused",
            "paused",
            None,
        ))
        .expect("historical error event");
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery);

    let reopened = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("restored historical error outbox");

    assert!(reopened.is_closed_for_submission());
    assert_eq!(
        reopened.submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Responding,
            "must stay closed",
            Some("running"),
            None,
        )),
        Err(AgentRunEventSubmitError::AlreadyClosed)
    );
}

#[tokio::test]
async fn registry_retains_an_open_actor_but_releases_it_after_true_terminal() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery);
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    let lifetime = Arc::downgrade(&outbox);
    drop(outbox);

    let retained = lifetime
        .upgrade()
        .expect("the actor retains an open outbox");
    retained
        .submit(AgentRunEvent::terminal_status(
            &run_id,
            Some(&turn_id),
            0,
            "Completed",
            "completed",
            None,
        ))
        .expect("terminal event");
    retained
        .wait_for_terminal_commit()
        .await
        .expect("terminal completion");
    drop(retained);
    tokio::time::timeout(Duration::from_secs(1), async {
        while lifetime.strong_count() > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal actor releases its outbox");
    assert!(lifetime.upgrade().is_none());

    let reopened = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("reopened terminal outbox");
    assert!(reopened.is_closed_for_submission());
}

#[tokio::test]
async fn competing_outbox_actors_cannot_replace_an_existing_sequence() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let first_delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let second_delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let first_registry = AgentRunEventOutboxes::new(executor.clone(), first_delivery.clone());
    let second_registry = AgentRunEventOutboxes::new(executor, second_delivery.clone());
    let first = first_registry
        .open(&conversation_id, &run_id)
        .await
        .expect("first outbox actor");
    let second = second_registry
        .open(&conversation_id, &run_id)
        .await
        .expect("competing outbox actor");

    first
        .submit(AgentRunEvent::terminal_status(
            &run_id,
            Some(&turn_id),
            0,
            "Completed",
            "completed",
            None,
        ))
        .expect("first terminal candidate");
    second
        .submit(AgentRunEvent::terminal_error(
            &run_id,
            Some(&turn_id),
            0,
            "Timed out",
            "timed_out",
            None,
        ))
        .expect("second terminal candidate");

    let (first_completion, second_completion) = tokio::join!(
        first.wait_for_terminal_commit(),
        second.wait_for_terminal_commit()
    );
    assert!(first_completion.is_ok());
    assert!(second_completion.is_ok());
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run events")
            .len(),
        1
    );
    let delivered = [
        first_registry
            .open(&conversation_id, &run_id)
            .await
            .expect("first registry remains closed"),
        second_registry
            .open(&conversation_id, &run_id)
            .await
            .expect("second registry observes durable winner"),
    ];
    assert!(delivered
        .iter()
        .all(|outbox| outbox.is_closed_for_submission()));
    assert!(matches!(
        database
            .get_agent_task_run(&run_id)
            .expect("winning task projection")
            .status
            .as_str(),
        "completed" | "timed_out"
    ));
    let delivered = first_delivery.events.lock().expect("capture lock").len()
        + second_delivery.events.lock().expect("capture lock").len();
    assert_eq!(
        delivered, 1,
        "the stale actor must not emit a live failure terminal"
    );
}

#[tokio::test]
async fn producers_cannot_encode_resumable_states_as_terminal_events() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery);
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");

    for status in ["paused", "awaiting_user_input"] {
        assert!(matches!(
            outbox.submit(AgentRunEvent::terminal_status(
                &run_id,
                Some(&turn_id),
                0,
                "resumable state",
                status,
                None,
            )),
            Err(AgentRunEventSubmitError::InvalidEvent { .. })
        ));
    }
    assert!(!outbox.is_closed_for_submission());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_saturation_fails_closed_after_preserving_the_accepted_prefix() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let outboxes = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    let cancellation = outbox.turn_cancellation_token();

    let (writer_started_tx, writer_started_rx) = std::sync::mpsc::channel();
    let release_writer = Arc::new(std::sync::Barrier::new(2));
    let blocked_writer = {
        let executor = executor.clone();
        let release_writer = Arc::clone(&release_writer);
        tokio::spawn(async move {
            executor
                .write(move |_database| {
                    writer_started_tx.send(()).expect("writer started signal");
                    release_writer.wait();
                    Ok(())
                })
                .await
                .expect("blocked writer")
        })
    };
    writer_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("writer lane is blocked");

    let mut accepted = 0usize;
    let saturation = loop {
        let result = outbox.submit(AgentRunEvent::output_delta(
            &run_id,
            Some(&turn_id),
            0,
            "answer-1",
            StreamBlockChannel::Answer,
            accepted,
            "x",
        ));
        match result {
            Ok(()) => accepted += 1,
            Err(error) => break error,
        }
        assert!(accepted < 2_000, "bounded queue must eventually saturate");
    };
    assert_eq!(saturation, AgentRunEventSubmitError::QueueFull);
    release_writer.wait();
    blocked_writer.await.expect("blocked writer task");

    let settlement =
        tokio::time::timeout(Duration::from_secs(5), outbox.wait_for_terminal_commit())
            .await
            .expect("queue failure settles the outbox");
    assert_eq!(settlement, Err(AgentRunEventOutboxFailure::QueueFull));
    assert!(cancellation.is_cancelled());

    let durable = database
        .list_agent_run_events(&run_id)
        .expect("accepted durable prefix");
    assert_eq!(durable.len(), accepted);
    assert_eq!(
        durable
            .iter()
            .map(|event| event.event_seq)
            .collect::<Vec<_>>(),
        (1..=accepted as u64).collect::<Vec<_>>()
    );
    let delivered = delivery.events.lock().expect("capture lock").clone();
    assert_eq!(delivered.len(), accepted + 1);
    assert_eq!(
        delivered.last().expect("failure terminal").event_seq,
        accepted as u64 + 1
    );
    assert_eq!(
        delivered.last().expect("failure terminal").persistence,
        nexa_core::agent_run::AgentRunEventPersistence::Ephemeral
    );
}

#[tokio::test(start_paused = true)]
async fn restart_continues_after_the_maximum_durable_sequence_without_renumbering_gaps() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let first = AgentRunEvent::output_delta(
        &run_id,
        Some(&turn_id),
        1,
        "answer-1",
        StreamBlockChannel::Answer,
        0,
        "a",
    );
    let fourth = AgentRunEvent::output_delta(
        &run_id,
        Some(&turn_id),
        4,
        "answer-1",
        StreamBlockChannel::Answer,
        1,
        "b",
    );
    database
        .save_agent_run_events(&[first, fourth])
        .expect("durable events with a historical gap");
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery);
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("restored outbox");
    tokio::task::yield_now().await;

    outbox
        .submit(AgentRunEvent::output_delta(
            &run_id,
            Some(&turn_id),
            0,
            "answer-1",
            StreamBlockChannel::Answer,
            2,
            "c",
        ))
        .expect("queued continuation");
    tokio::time::advance(Duration::from_millis(100)).await;
    assert_eq!(outbox.flush().await.expect("continuation committed"), 5);

    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run events")
            .iter()
            .map(|event| event.event_seq)
            .collect::<Vec<_>>(),
        vec![1, 4, 5]
    );
}

#[tokio::test(start_paused = true)]
async fn semantic_status_flushes_the_pending_delta_without_waiting_for_the_deadline() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let outboxes = AgentRunEventOutboxes::new(executor, delivery.clone());
    let outbox = outboxes
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");

    outbox
        .submit(AgentRunEvent::output_delta(
            &run_id,
            Some(&turn_id),
            0,
            "answer-1",
            StreamBlockChannel::Answer,
            0,
            "hello",
        ))
        .expect("queued output delta");
    assert!(delivery.events.lock().expect("capture lock").is_empty());
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Routing,
            "Route selected",
            Some("running"),
            None,
        ))
        .expect("queued semantic status");

    assert_eq!(outbox.flush().await.expect("semantic batch committed"), 2);
    assert_eq!(
        delivery
            .events
            .lock()
            .expect("capture lock")
            .iter()
            .map(|event| event.event_seq)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[tokio::test]
async fn restart_restores_checkpoint_response_when_relaunch_never_started() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let first_registry = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
    let outbox = first_registry
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Routing,
            "Agent started",
            Some("running"),
            None,
        ))
        .expect("initial started marker");
    outbox.flush().await.expect("started marker committed");

    let checkpoint = database
        .create_task_resume_checkpoint(&run_id, "user_pause")
        .expect("resume checkpoint");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Paused,
            "Paused with a resumable checkpoint",
            Some("paused"),
            Some(&serde_json::json!({
                "checkpointId": checkpoint.id,
                "resumePrompt": checkpoint.resume_prompt,
            })),
        ))
        .expect("pause boundary");
    outbox.flush().await.expect("pause boundary committed");

    let response = ConversationMessage {
        id: "checkpoint-response-1".to_string(),
        conversation_id: conversation_id.clone(),
        role: Role::User,
        content: checkpoint.resume_prompt.clone(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 3,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    let first_launch = database
        .resume_agent_turn_from_checkpoint(
            &response,
            None,
            None,
            "checkpoint-restart-key",
            &checkpoint.id,
        )
        .expect("first checkpoint launch");
    assert_eq!(first_launch.status, "queued");
    assert!(!first_launch.reused);
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Routing,
            "Task queued",
            Some("queued"),
            None,
        ))
        .expect("queued marker");
    outbox.flush().await.expect("queued marker committed");
    drop(outbox);
    drop(first_registry);

    let restarted = AgentRunEventOutboxes::new(executor, delivery);
    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");
    assert_eq!(recovery.restored_suspensions, 1);
    assert_eq!(recovery.repaired_terminals, 0);
    assert_eq!(recovery.cancelled_runs, 0);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("restored task")
            .status,
        "paused"
    );
    assert_eq!(
        database
            .get_conversation_turn(&turn_id)
            .expect("restored turn")
            .status,
        "paused"
    );
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run ledger")
            .iter()
            .filter(|event| event.closes_run())
            .count(),
        0
    );
    let message_count = database
        .get_messages(&conversation_id)
        .expect("conversation messages")
        .len();

    let retry = ConversationMessage {
        id: "checkpoint-response-retry".to_string(),
        ..response.clone()
    };
    let recovered_launch = database
        .resume_agent_turn_from_checkpoint(
            &retry,
            None,
            None,
            "checkpoint-restart-key",
            &checkpoint.id,
        )
        .expect("replay restored response");
    assert_eq!(recovered_launch.status, "queued");
    assert!(!recovered_launch.reused);
    assert_eq!(recovered_launch.user_message_id, response.id);
    assert_eq!(
        database
            .get_messages(&conversation_id)
            .expect("conversation messages")
            .len(),
        message_count
    );
    let idempotent_retry = database
        .resume_agent_turn_from_checkpoint(
            &retry,
            None,
            None,
            "checkpoint-restart-key",
            &checkpoint.id,
        )
        .expect("idempotent response replay");
    assert!(idempotent_retry.reused);
    assert_eq!(idempotent_retry.user_message_id, response.id);
}

#[tokio::test]
async fn restart_restores_historical_awaiting_response_before_relaunch_started() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let first_registry = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
    let outbox = first_registry
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Routing,
            "Agent started",
            Some("running"),
            None,
        ))
        .expect("initial started marker");
    outbox.flush().await.expect("started marker committed");

    let created = database
        .create_interaction_request(&CreateInteractionRequest {
            conversation_id: conversation_id.clone(),
            turn_id: turn_id.clone(),
            tool_call_id: Some("call-scope".to_string()),
            idempotency_key: "request-scope".to_string(),
            kind: InteractionKind::UserInput,
            title: "Choose a scope".to_string(),
            description: None,
            questions: vec![InteractionQuestion {
                id: "scope".to_string(),
                header: "Scope".to_string(),
                question: "Which scope should be used?".to_string(),
                kind: InteractionQuestionKind::Short,
                options: Vec::new(),
                placeholder: None,
                why: None,
            }],
            required: true,
            expires_at: None,
        })
        .expect("interaction request");
    database
        .suspend_agent_turn_for_interaction(&created.request.interaction_id)
        .expect("durable interaction suspension");
    database
        .mark_interaction_presented(&created.request.interaction_id)
        .expect("presented interaction");

    // Older producers kept the presentation tone in `status`. Recovery must
    // continue to recognize that durable awaiting-user-input boundary.
    let mut historical_awaiting = AgentRunEvent::from_agent_event(&AgentEvent::ControllerStatus {
        code: "awaiting_user_input".to_string(),
        content: "Waiting for your response".to_string(),
        tone: Some("attention".to_string()),
    })
    .with_context(Some(&run_id), Some(&turn_id), None);
    historical_awaiting.status = Some("attention".to_string());
    outbox
        .submit(historical_awaiting)
        .expect("historical awaiting boundary");
    outbox.flush().await.expect("awaiting boundary committed");

    let response = ConversationMessage {
        id: "interaction-response-1".to_string(),
        conversation_id: conversation_id.clone(),
        role: Role::User,
        content: "Use the local scope".to_string(),
        tool_call_id: None,
        tool_calls: Vec::new(),
        artifacts: None,
        token_count: 4,
        created_at: String::new(),
        sort_order: 0,
        thinking: None,
        image_attachments: None,
    };
    let mut answers = BTreeMap::new();
    answers.insert("scope".to_string(), vec!["local".to_string()]);
    let response_input = SubmitInteractionResponse {
        interaction_id: created.request.interaction_id.clone(),
        resume_token: created.request.resume_token.clone(),
        answers,
    };
    let first_launch = database
        .resume_agent_turn_with_interaction_response(
            &response,
            None,
            None,
            "interaction-restart-key",
            &response_input,
        )
        .expect("first interaction continuation");
    assert_eq!(first_launch.status, "queued");
    assert!(!first_launch.reused);
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Routing,
            "Task queued",
            Some("queued"),
            None,
        ))
        .expect("queued marker");
    outbox.flush().await.expect("queued marker committed");
    let message_count = database
        .get_messages(&conversation_id)
        .expect("conversation messages")
        .len();
    drop(outbox);
    drop(first_registry);

    let restarted = AgentRunEventOutboxes::new(executor, delivery);
    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");
    assert_eq!(recovery.restored_suspensions, 1);
    assert_eq!(recovery.repaired_terminals, 0);
    assert_eq!(recovery.cancelled_runs, 0);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("restored task")
            .status,
        "awaiting_user_input"
    );
    assert_eq!(
        database
            .get_conversation_turn(&turn_id)
            .expect("restored turn")
            .status,
        "awaiting_user_input"
    );
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run ledger")
            .iter()
            .filter(|event| event.closes_run())
            .count(),
        0
    );

    let retry = ConversationMessage {
        id: "interaction-response-retry".to_string(),
        ..response.clone()
    };
    let recovered_launch = database
        .resume_agent_turn_with_interaction_response(
            &retry,
            None,
            None,
            "interaction-restart-key",
            &response_input,
        )
        .expect("replay restored interaction response");
    assert_eq!(recovered_launch.status, "queued");
    assert!(!recovered_launch.reused);
    assert_eq!(recovered_launch.user_message_id, response.id);
    assert_eq!(
        database
            .get_messages(&conversation_id)
            .expect("conversation messages")
            .len(),
        message_count
    );
}

#[tokio::test]
async fn restart_cancels_a_relaunch_after_its_started_marker() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let first_registry = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
    let outbox = first_registry
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Paused,
            "Paused with a resumable checkpoint",
            Some("paused"),
            None,
        ))
        .expect("pause boundary");
    outbox.flush().await.expect("pause boundary committed");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Routing,
            "Task queued",
            Some("queued"),
            None,
        ))
        .expect("queued marker");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::Routing,
            "Agent started",
            Some("running"),
            None,
        ))
        .expect("started marker");
    outbox.flush().await.expect("relaunch markers committed");
    drop(outbox);
    drop(first_registry);

    let restarted = AgentRunEventOutboxes::new(executor, delivery);
    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");
    assert_eq!(recovery.restored_suspensions, 0);
    assert_eq!(recovery.cancelled_runs, 1);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("cancelled task")
            .status,
        "cancelled"
    );
    assert_eq!(
        database
            .get_conversation_turn(&turn_id)
            .expect("cancelled turn")
            .status,
        "cancelled"
    );
    let events = database.list_agent_run_events(&run_id).expect("run ledger");
    assert_eq!(events.iter().filter(|event| event.closes_run()).count(), 1);
    assert_eq!(
        events.last().and_then(|event| event.status.as_deref()),
        Some("cancelled")
    );
}

#[tokio::test]
async fn restart_preserves_cancelling_intent_after_an_awaiting_boundary() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let first_registry = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
    let outbox = first_registry
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::AwaitingUserInput,
            "Waiting for your response",
            Some("awaiting_user_input"),
            None,
        ))
        .expect("awaiting boundary");
    outbox.flush().await.expect("awaiting boundary committed");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::AwaitingUserInput,
            "Cancelling while waiting for user input",
            Some("cancelling"),
            None,
        ))
        .expect("cancelling intent");
    outbox.flush().await.expect("cancelling intent committed");
    drop(outbox);
    drop(first_registry);

    let restarted = AgentRunEventOutboxes::new(executor, delivery);
    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");
    assert_eq!(recovery.restored_suspensions, 0);
    assert_eq!(recovery.cancelled_runs, 1);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("cancelled task")
            .status,
        "cancelled"
    );
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run ledger")
            .iter()
            .filter(|event| event.closes_run())
            .count(),
        1
    );
}

#[tokio::test]
async fn restart_preserves_legacy_task_only_cancelling_intent() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let first_registry = AgentRunEventOutboxes::new(executor.clone(), delivery.clone());
    let outbox = first_registry
        .open(&conversation_id, &run_id)
        .await
        .expect("run outbox");
    outbox
        .submit(AgentRunEvent::status_update(
            &run_id,
            Some(&turn_id),
            0,
            AgentRunPhase::AwaitingUserInput,
            "Waiting for your response",
            Some("attention"),
            None,
        ))
        .expect("historical awaiting boundary");
    outbox.flush().await.expect("awaiting boundary committed");

    // Legacy stop persisted only the task projection before emitting a
    // frontend-only status. Recovery must not let the earlier suspension
    // boundary erase that durable intent.
    database
        .update_agent_task_run_progress(
            &run_id,
            Some("cancelling"),
            Some("awaiting_user_input"),
            None,
            Some("Cancelling while waiting for user input"),
            None,
            None,
        )
        .expect("legacy task-only cancelling intent");
    drop(outbox);
    drop(first_registry);

    let restarted = AgentRunEventOutboxes::new(executor, delivery);
    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");

    assert_eq!(recovery.restored_suspensions, 0);
    assert_eq!(recovery.cancelled_runs, 1);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("cancelled task")
            .status,
        "cancelled"
    );
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run ledger")
            .iter()
            .filter(|event| event.closes_run())
            .count(),
        1
    );
}

#[tokio::test]
async fn restart_repairs_an_active_projection_from_the_existing_true_terminal() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    database
        .save_agent_run_event(&AgentRunEvent::terminal_status(
            &run_id,
            Some(&turn_id),
            1,
            "Completed before projection commit",
            "completed",
            None,
        ))
        .expect("historical terminal ledger row");
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let restarted = AgentRunEventOutboxes::new(executor, delivery);

    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");
    assert_eq!(recovery.repaired_terminals, 1);
    assert_eq!(recovery.cancelled_runs, 0);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("repaired task")
            .status,
        "completed"
    );
    assert_eq!(
        database
            .get_conversation_turn(&turn_id)
            .expect("repaired turn")
            .status,
        "success"
    );
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run ledger")
            .len(),
        1
    );
    let reopened = restarted
        .open(&conversation_id, &run_id)
        .await
        .expect("repaired terminal outbox");
    assert!(reopened.is_closed_for_submission());
}

#[tokio::test]
async fn restart_terminal_arbitration_overrides_a_materialized_pause_projection() {
    let database = Database::open_memory().expect("in-memory database");
    let (_conversation_id, turn_id, run_id) = create_started_run(&database);
    database
        .update_agent_task_run_progress(
            &run_id,
            Some("paused"),
            Some("paused"),
            None,
            Some("Paused with a resumable checkpoint"),
            None,
            None,
        )
        .expect("paused task projection");
    database
        .finalize_conversation_turn(&turn_id, "paused", None, None)
        .expect("paused turn projection");
    database
        .save_agent_run_event(&AgentRunEvent::terminal_status(
            &run_id,
            Some(&turn_id),
            1,
            "Completed before projection commit",
            "completed",
            None,
        ))
        .expect("historical terminal ledger row");
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let restarted = AgentRunEventOutboxes::new(executor, delivery);

    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");
    assert_eq!(recovery.repaired_terminals, 1);
    assert_eq!(recovery.restored_suspensions, 0);
    assert_eq!(recovery.cancelled_runs, 0);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("terminal task projection")
            .status,
        "completed"
    );
    assert_eq!(
        database
            .get_conversation_turn(&turn_id)
            .expect("terminal turn projection")
            .status,
        "success"
    );
    assert_eq!(
        database
            .list_agent_run_events(&run_id)
            .expect("run ledger")
            .len(),
        1
    );
    assert_eq!(
        restarted
            .recover_after_restart()
            .await
            .expect("idempotent startup recovery"),
        Default::default()
    );
}

#[tokio::test]
async fn restart_keeps_legacy_done_paused_as_a_resumable_boundary() {
    let database = Database::open_memory().expect("in-memory database");
    let (conversation_id, turn_id, run_id) = create_started_run(&database);
    database
        .save_agent_run_event(&AgentRunEvent::terminal_status(
            &run_id,
            Some(&turn_id),
            1,
            "Paused by an older build",
            "paused",
            None,
        ))
        .expect("legacy pause ledger row");
    let executor = DatabaseExecutor::new(database.clone(), 8).expect("database executor");
    let delivery = Arc::new(CaptureDelivery::new(database.clone()));
    let restarted = AgentRunEventOutboxes::new(executor, delivery);

    let recovery = restarted
        .recover_after_restart()
        .await
        .expect("startup recovery");
    assert_eq!(recovery.restored_suspensions, 1);
    assert_eq!(recovery.cancelled_runs, 0);
    assert_eq!(
        database
            .get_agent_task_run(&run_id)
            .expect("legacy paused task")
            .status,
        "paused"
    );
    assert_eq!(
        database
            .get_conversation_turn(&turn_id)
            .expect("legacy paused turn")
            .status,
        "paused"
    );
    let reopened = restarted
        .open(&conversation_id, &run_id)
        .await
        .expect("legacy resumable outbox");
    assert!(!reopened.is_closed_for_submission());
}
