//! Coordinate native checkpoint/write/record transactions across agent workers.
//! Ordinary file writes can run in parallel; native directory cp/mv temporarily
//! excludes writers because its complete destination set can grow during copy.
use crate::error::CoreError;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock,
};

type PathLocks = Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>;
static PATH_LOCKS: OnceLock<PathLocks> = OnceLock::new();
static TREE_GATE: OnceLock<Arc<RwLock<()>>> = OnceLock::new();

pub struct FileMutationGuard {
    _path: OwnedMutexGuard<()>,
    _tree: OwnedRwLockReadGuard<()>,
}

pub fn canonical_file_identity(path: &Path) -> Result<PathBuf, CoreError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or_else(|| {
            CoreError::InvalidInput(format!("Cannot resolve file identity: {}", path.display()))
        })?;
        missing.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| CoreError::InvalidInput("File has no existing ancestor".into()))?;
    }
    let mut canonical = std::fs::canonicalize(ancestor)?;
    for name in missing.into_iter().rev() {
        canonical.push(name);
    }
    Ok(canonical)
}

/// Called from native blocking mutation closures, before reading the before
/// state. The guard must outlive both the disk write and its durable receipt.
pub fn lock_file_mutation(
    path: &Path,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<FileMutationGuard, CoreError> {
    let acquire = async {
        let tree = TREE_GATE
            .get_or_init(|| Arc::new(RwLock::new(())))
            .clone()
            .read_owned()
            .await;
        let identity = canonical_file_identity(path)?;
        let lock = {
            let mut locks = PATH_LOCKS
                .get_or_init(Mutex::default)
                .lock()
                .map_err(|_| CoreError::Internal("File mutation registry poisoned".into()))?;
            locks.retain(|_, value| value.strong_count() > 0);
            if let Some(lock) = locks.get(&identity).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                locks.insert(identity, Arc::downgrade(&lock));
                lock
            }
        };
        let path = lock.lock_owned().await;
        Ok(FileMutationGuard {
            _path: path,
            _tree: tree,
        })
    };
    futures::executor::block_on(async {
        if let Some(cancel) = cancel {
            tokio::select! { biased;
                _ = cancel.cancelled() => Err(CoreError::InvalidInput("File mutation cancelled before writing".into())),
                result = acquire => result,
            }
        } else {
            acquire.await
        }
    })
}

pub async fn lock_native_tree_mutation() -> OwnedRwLockWriteGuard<()> {
    TREE_GATE
        .get_or_init(|| Arc::new(RwLock::new(())))
        .clone()
        .write_owned()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_share_one_guard_and_waiting_cancellation_prevents_a_late_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.txt");
        std::fs::write(&path, "original").unwrap();
        std::fs::create_dir(directory.path().join("sub")).unwrap();
        let alias = directory.path().join("sub/../file.txt");
        assert_eq!(
            canonical_file_identity(&alias).unwrap(),
            std::fs::canonicalize(&path).unwrap()
        );
        let guard = lock_file_mutation(&path, None).unwrap();
        let other_guard = lock_file_mutation(&directory.path().join("other.txt"), None).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = tokio_util::sync::CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let acquired = lock_file_mutation(&alias, Some(&worker_cancel));
            if acquired.is_ok() {
                std::fs::write(&alias, "late write").unwrap();
            }
            tx.send(acquired.is_ok()).unwrap();
        });
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(30))
            .is_err());
        cancel.cancel();
        assert!(!rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap());
        drop((guard, other_guard));
        worker.join().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
    }
}
