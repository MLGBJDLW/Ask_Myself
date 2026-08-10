//! Cooperative scheduling for resource-intensive desktop background work.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use nexa_core::embedding_job::{EmbeddingJobControl, ScanProgress};
use nexa_core::error::CoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChangeJob {
    pub source_id: String,
    pub changed_paths: Vec<PathBuf>,
    pub removed_paths: Vec<PathBuf>,
}

#[derive(Clone)]
pub struct BackgroundWorkGovernor {
    inner: Arc<Inner>,
}

pub struct BackgroundWorkReceiver {
    inner: Arc<Inner>,
}

pub struct BackgroundWorkPermit {
    inner: Arc<Inner>,
    job: SourceChangeJob,
    cancelled: Arc<AtomicBool>,
}

pub struct ForegroundWorkLease {
    inner: Arc<Inner>,
    active: bool,
}

#[derive(Clone)]
pub struct CooperativeEmbeddingControl {
    inner: Arc<Inner>,
    progress: Option<Arc<dyn Fn(ScanProgress) + Send + Sync>>,
}

struct Inner {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    foreground_leases: usize,
    pending: BTreeMap<String, PendingSourceChanges>,
    running: Option<RunningJob>,
    stopped: bool,
}

#[derive(Default)]
struct PendingSourceChanges {
    changed_paths: HashSet<PathBuf>,
    removed_paths: HashSet<PathBuf>,
}

struct RunningJob {
    source_id: String,
    cancelled: Arc<AtomicBool>,
}

impl BackgroundWorkGovernor {
    pub fn new() -> (Self, BackgroundWorkReceiver) {
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        (
            Self {
                inner: Arc::clone(&inner),
            },
            BackgroundWorkReceiver { inner },
        )
    }

    /// Merge a watcher generation into the pending source job. Removed paths
    /// win over changed paths, and a newer generation cancels active embedding
    /// work for the same source.
    pub fn submit_source_changes(
        &self,
        source_id: String,
        changed_paths: impl IntoIterator<Item = PathBuf>,
        removed_paths: impl IntoIterator<Item = PathBuf>,
    ) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            return;
        }

        if let Some(running) = state
            .running
            .as_ref()
            .filter(|running| running.source_id == source_id)
        {
            running.cancelled.store(true, Ordering::Release);
        }

        let pending = state.pending.entry(source_id).or_default();
        for path in changed_paths {
            if !pending.removed_paths.contains(&path) {
                pending.changed_paths.insert(path);
            }
        }
        for path in removed_paths {
            pending.changed_paths.remove(&path);
            pending.removed_paths.insert(path);
        }
        self.inner.changed.notify_all();
    }

    /// Mark an interactive agent turn as foreground work. Background jobs
    /// pause at their next bounded checkpoint until all foreground leases end.
    pub fn foreground_lease(&self) -> ForegroundWorkLease {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.foreground_leases = state.foreground_leases.saturating_add(1);
        ForegroundWorkLease {
            inner: Arc::clone(&self.inner),
            active: true,
        }
    }

    /// Control adapter for an explicit user-started embedding operation. It
    /// yields to agent turns but is not cancelled as an obsolete watcher job.
    pub fn cooperative_embedding_control(
        &self,
        progress: Option<Arc<dyn Fn(ScanProgress) + Send + Sync>>,
    ) -> CooperativeEmbeddingControl {
        CooperativeEmbeddingControl {
            inner: Arc::clone(&self.inner),
            progress,
        }
    }
}

impl BackgroundWorkReceiver {
    pub fn recv(&self) -> Option<BackgroundWorkPermit> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if state.stopped {
                return None;
            }
            if state.foreground_leases == 0 && state.running.is_none() {
                if let Some(source_id) = state.pending.keys().next().cloned() {
                    let pending = state.pending.remove(&source_id).expect("pending job");
                    let cancelled = Arc::new(AtomicBool::new(false));
                    state.running = Some(RunningJob {
                        source_id: source_id.clone(),
                        cancelled: Arc::clone(&cancelled),
                    });
                    let mut changed_paths: Vec<_> = pending.changed_paths.into_iter().collect();
                    let mut removed_paths: Vec<_> = pending.removed_paths.into_iter().collect();
                    changed_paths.sort();
                    removed_paths.sort();
                    return Some(BackgroundWorkPermit {
                        inner: Arc::clone(&self.inner),
                        job: SourceChangeJob {
                            source_id,
                            changed_paths,
                            removed_paths,
                        },
                        cancelled,
                    });
                }
            }
            state = self
                .inner
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl BackgroundWorkPermit {
    pub fn job(&self) -> &SourceChangeJob {
        &self.job
    }

    /// Yield while foreground work is active without abandoning ingestion for
    /// the already accepted watcher generation.
    pub fn wait_for_foreground(&self) -> Result<(), CoreError> {
        wait_for_foreground(&self.inner)
    }
}

impl EmbeddingJobControl for BackgroundWorkPermit {
    fn checkpoint(&self) -> Result<(), CoreError> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(CoreError::Cancelled(
                "obsolete background embedding generation".to_string(),
            ));
        }
        wait_for_foreground(&self.inner)?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(CoreError::Cancelled(
                "obsolete background embedding generation".to_string(),
            ));
        }
        Ok(())
    }
}

impl Drop for BackgroundWorkPermit {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state
            .running
            .as_ref()
            .is_some_and(|running| Arc::ptr_eq(&running.cancelled, &self.cancelled))
        {
            state.running = None;
        }
        self.inner.changed.notify_all();
    }
}

impl Drop for ForegroundWorkLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.foreground_leases = state.foreground_leases.saturating_sub(1);
        self.active = false;
        self.inner.changed.notify_all();
    }
}

impl EmbeddingJobControl for CooperativeEmbeddingControl {
    fn checkpoint(&self) -> Result<(), CoreError> {
        wait_for_foreground(&self.inner)
    }

    fn on_progress(&self, progress: ScanProgress) {
        if let Some(callback) = &self.progress {
            callback(progress);
        }
    }
}

fn wait_for_foreground(inner: &Inner) -> Result<(), CoreError> {
    let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
    while state.foreground_leases > 0 && !state.stopped {
        state = inner.changed.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    if state.stopped {
        Err(CoreError::Cancelled(
            "background work governor stopped".to_string(),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn merges_paths_and_removal_wins() {
        let (governor, receiver) = BackgroundWorkGovernor::new();
        governor.submit_source_changes(
            "source".to_string(),
            [PathBuf::from("a.md"), PathBuf::from("b.md")],
            [],
        );
        governor.submit_source_changes(
            "source".to_string(),
            [PathBuf::from("c.md"), PathBuf::from("a.md")],
            [PathBuf::from("a.md")],
        );

        let permit = receiver.recv().expect("merged job");
        assert_eq!(
            permit.job().changed_paths,
            vec![PathBuf::from("b.md"), PathBuf::from("c.md")]
        );
        assert_eq!(permit.job().removed_paths, vec![PathBuf::from("a.md")]);
    }

    #[test]
    fn foreground_lease_defers_pending_background_work() {
        let (governor, receiver) = BackgroundWorkGovernor::new();
        let lease = governor.foreground_lease();
        governor.submit_source_changes("source".to_string(), [PathBuf::from("a.md")], []);
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || tx.send(receiver.recv().is_some()).unwrap());

        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(lease);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), true);
        worker.join().unwrap();
    }

    #[test]
    fn newer_source_generation_cancels_active_embedding_only() {
        let (governor, receiver) = BackgroundWorkGovernor::new();
        governor.submit_source_changes("source".to_string(), [PathBuf::from("a.md")], []);
        let active = receiver.recv().expect("active job");

        governor.submit_source_changes("source".to_string(), [PathBuf::from("b.md")], []);
        assert!(matches!(
            EmbeddingJobControl::checkpoint(&active),
            Err(CoreError::Cancelled(_))
        ));
        drop(active);

        let next = receiver.recv().expect("replacement job");
        assert_eq!(next.job().changed_paths, vec![PathBuf::from("b.md")]);
    }
}
