//! File watcher module — monitors source directories for changes.
//!
//! Uses the `notify` crate to watch source directories recursively and
//! emit debounced events when files are created, modified, or removed.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{info, warn};

use crate::error::CoreError;

/// An event emitted when a watched file changes.
#[derive(Debug, Clone)]
pub struct WatcherEvent {
    pub path: PathBuf,
    pub kind: WatcherEventKind,
}

/// The kind of file change detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatcherEventKind {
    Created,
    Modified,
    Removed,
}

/// Watches directories for file system changes.
pub struct FileWatcher {
    watcher: RecommendedWatcher,
}

impl FileWatcher {
    /// Create a new file watcher.
    ///
    /// Returns the watcher and a receiver that emits [`WatcherEvent`]s.
    /// Events are debounced with a 2-second delay to batch rapid changes.
    pub fn new() -> Result<(Self, mpsc::Receiver<WatcherEvent>), CoreError> {
        let (tx, rx) = mpsc::channel::<WatcherEvent>();

        let watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| match res {
                Ok(event) => {
                    let kind = match event.kind {
                        EventKind::Create(_) => Some(WatcherEventKind::Created),
                        EventKind::Modify(_) => Some(WatcherEventKind::Modified),
                        EventKind::Remove(_) => Some(WatcherEventKind::Removed),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        for path in event.paths {
                            let evt = WatcherEvent {
                                path: path.clone(),
                                kind: kind.clone(),
                            };
                            if tx.send(evt).is_err() {
                                warn!(
                                    "Watcher channel closed, dropping event for {}",
                                    path.display()
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("File watcher error: {e}");
                }
            },
            Config::default().with_poll_interval(Duration::from_secs(2)),
        )
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;

        info!("File watcher initialized");
        Ok((Self { watcher }, rx))
    }

    /// Start watching a directory recursively.
    pub fn watch(&mut self, path: &Path) -> Result<(), CoreError> {
        self.watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;
        info!("Started watching: {}", path.display());
        Ok(())
    }

    /// Stop watching a directory.
    pub fn unwatch(&mut self, path: &Path) -> Result<(), CoreError> {
        self.watcher
            .unwatch(path)
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;
        info!("Stopped watching: {}", path.display());
        Ok(())
    }
}
