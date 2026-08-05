//! Bounded execution lanes for synchronous SQLite work.
//!
//! `rusqlite` is intentionally synchronous. Async hosts must enqueue work on
//! these dedicated threads instead of blocking Tokio workers on the global
//! connection mutex.

use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::db::Database;
use crate::error::CoreError;

type DatabaseJob = Box<dyn FnOnce(&Database) + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseExecutionMetrics {
    pub queue_wait: Duration,
    pub execution: Duration,
}

#[derive(Debug)]
pub struct DatabaseExecution<T> {
    pub value: T,
    pub metrics: DatabaseExecutionMetrics,
}

#[derive(Clone)]
pub struct DatabaseExecutor {
    reader: SyncSender<DatabaseJob>,
    writer: SyncSender<DatabaseJob>,
}

impl DatabaseExecutor {
    pub fn new(database: Database, capacity: usize) -> Result<Self, CoreError> {
        if capacity == 0 {
            return Err(CoreError::InvalidInput(
                "Database executor capacity must be greater than zero".to_string(),
            ));
        }

        let reader_database = database.read_only_lane()?;
        let reader = spawn_lane("nexa-db-reader", reader_database, capacity)?;
        let writer = spawn_lane("nexa-db-writer", database, capacity)?;
        Ok(Self { reader, writer })
    }

    pub async fn read<T, F>(&self, operation: F) -> Result<DatabaseExecution<T>, CoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Database) -> Result<T, CoreError> + Send + 'static,
    {
        execute(&self.reader, "reader", operation).await
    }

    pub async fn write<T, F>(&self, operation: F) -> Result<DatabaseExecution<T>, CoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Database) -> Result<T, CoreError> + Send + 'static,
    {
        execute(&self.writer, "writer", operation).await
    }
}

fn spawn_lane(
    name: &str,
    database: Database,
    capacity: usize,
) -> Result<SyncSender<DatabaseJob>, CoreError> {
    let (sender, receiver) = mpsc::sync_channel::<DatabaseJob>(capacity);
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || {
            while let Ok(job) = receiver.recv() {
                job(&database);
            }
        })
        .map_err(|error| CoreError::Internal(format!("Failed to start {name}: {error}")))?;
    Ok(sender)
}

async fn execute<T, F>(
    lane: &SyncSender<DatabaseJob>,
    lane_name: &str,
    operation: F,
) -> Result<DatabaseExecution<T>, CoreError>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> Result<T, CoreError> + Send + 'static,
{
    let queued_at = Instant::now();
    let (result_tx, result_rx) = oneshot::channel();
    let job = Box::new(move |database: &Database| {
        let started_at = Instant::now();
        let queue_wait = started_at.saturating_duration_since(queued_at);
        let value = operation(database);
        let execution = started_at.elapsed();
        let _ = result_tx.send((value, queue_wait, execution));
    });

    match lane.try_send(job) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            return Err(CoreError::InvalidInput(format!(
                "Database {lane_name} queue is full; retry after current work completes"
            )))
        }
        Err(TrySendError::Disconnected(_)) => {
            return Err(CoreError::Internal(format!(
                "Database {lane_name} executor is unavailable"
            )))
        }
    }

    let (value, queue_wait, execution) = result_rx.await.map_err(|_| {
        CoreError::Internal(format!(
            "Database {lane_name} executor stopped before returning a result"
        ))
    })?;
    Ok(DatabaseExecution {
        value: value?,
        metrics: DatabaseExecutionMetrics {
            queue_wait,
            execution,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn executes_reads_and_writes_on_dedicated_threads() {
        let database = Database::open_memory().unwrap();
        let executor = DatabaseExecutor::new(database, 4).unwrap();

        let writer = executor
            .write(|database| {
                let thread = std::thread::current()
                    .name()
                    .unwrap_or_default()
                    .to_string();
                database.load_app_config()?;
                Ok(thread)
            })
            .await
            .unwrap();
        let reader = executor
            .read(|database| {
                let thread = std::thread::current()
                    .name()
                    .unwrap_or_default()
                    .to_string();
                database.get_conversation_stats()?;
                Ok(thread)
            })
            .await
            .unwrap();

        assert_eq!(writer.value, "nexa-db-writer");
        assert_eq!(reader.value, "nexa-db-reader");
    }
}
