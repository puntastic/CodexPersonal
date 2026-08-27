use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use codex_protocol::ThreadId;
use tracing::warn;

use crate::ThreadStoreError;
use crate::ThreadStoreResult;

const WRITER_LOCK_DIR: &str = "thread-writer-locks";
const COORDINATION_LOCK_FILE: &str = ".coordination.lock";
const LIFECYCLE_LOCK_SUFFIX: &str = ".lifecycle.lock";

pub(super) struct WriterLockCoordinator {
    directory: PathBuf,
    cleanup_attempted: AtomicBool,
}

pub(super) struct WriterLockGuard {
    coordinator: Arc<WriterLockCoordinator>,
    path: PathBuf,
    file: Option<File>,
}

/// Cross-process counterpart to the process-local lifecycle reservation.
///
/// Lifecycle files deliberately remain on disk after the guard is released. Shared locks make
/// unlink-on-drop unsafe: another process may still hold the old file while a new caller opens a
/// replacement inode. A stable zero-byte file per thread keeps every process on the same lock.
#[derive(Debug)]
pub(super) struct LifecycleLockGuard {
    _file: File,
}

#[derive(Clone, Copy)]
enum LifecycleLockMode {
    Shared,
    Exclusive,
}

impl WriterLockCoordinator {
    pub(super) fn new(codex_home: &Path) -> Self {
        Self {
            directory: codex_home.join(WRITER_LOCK_DIR),
            cleanup_attempted: AtomicBool::new(false),
        }
    }

    pub(super) fn acquire(
        self: &Arc<Self>,
        thread_id: ThreadId,
    ) -> ThreadStoreResult<WriterLockGuard> {
        let coordination_lock = self.lock_coordination()?;
        if !self.cleanup_attempted.swap(true, Ordering::Relaxed)
            && let Err(err) = self.remove_stale_thread_locks()
        {
            warn!("failed to clean up stale thread writer locks: {err}");
        }

        let path = self.directory.join(format!("{thread_id}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| ThreadStoreError::Internal {
                message: format!(
                    "failed to open thread writer lock {}: {err}",
                    path.display()
                ),
            })?;

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(ThreadStoreError::Conflict {
                    message: format!("thread {thread_id} already has an active writer"),
                });
            }
            Err(std::fs::TryLockError::Error(err)) => {
                return Err(ThreadStoreError::Internal {
                    message: format!(
                        "failed to acquire thread writer lock {}: {err}",
                        path.display()
                    ),
                });
            }
        }

        drop(coordination_lock);
        Ok(WriterLockGuard {
            coordinator: Arc::clone(self),
            path,
            file: Some(file),
        })
    }

    /// Reserve a thread's lifecycle while a fork prepares and persists its child reference.
    pub(super) async fn reserve_lifecycle(
        self: &Arc<Self>,
        thread_id: ThreadId,
    ) -> ThreadStoreResult<LifecycleLockGuard> {
        self.acquire_lifecycle(thread_id, LifecycleLockMode::Shared)
            .await
    }

    /// Exclude cross-process fork reservations while a destructive reference scan and delete run.
    pub(super) async fn lock_lifecycle(
        self: &Arc<Self>,
        thread_id: ThreadId,
    ) -> ThreadStoreResult<LifecycleLockGuard> {
        self.acquire_lifecycle(thread_id, LifecycleLockMode::Exclusive)
            .await
    }

    async fn acquire_lifecycle(
        self: &Arc<Self>,
        thread_id: ThreadId,
        mode: LifecycleLockMode,
    ) -> ThreadStoreResult<LifecycleLockGuard> {
        let coordinator = Arc::clone(self);
        tokio::task::spawn_blocking(move || coordinator.acquire_lifecycle_blocking(thread_id, mode))
            .await
            .map_err(|err| ThreadStoreError::Internal {
                message: format!(
                    "failed to join thread {thread_id} lifecycle lock acquisition: {err}"
                ),
            })?
    }

    fn acquire_lifecycle_blocking(
        &self,
        thread_id: ThreadId,
        mode: LifecycleLockMode,
    ) -> ThreadStoreResult<LifecycleLockGuard> {
        fs::create_dir_all(&self.directory).map_err(|err| ThreadStoreError::Internal {
            message: format!(
                "failed to create thread writer lock directory {}: {err}",
                self.directory.display()
            ),
        })?;
        let path = self
            .directory
            .join(format!("{thread_id}{LIFECYCLE_LOCK_SUFFIX}"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| ThreadStoreError::Internal {
                message: format!(
                    "failed to open thread lifecycle lock {}: {err}",
                    path.display()
                ),
            })?;
        let result = match mode {
            LifecycleLockMode::Shared => file.lock_shared(),
            LifecycleLockMode::Exclusive => file.lock(),
        };
        result.map_err(|err| ThreadStoreError::Internal {
            message: format!(
                "failed to acquire thread lifecycle lock {}: {err}",
                path.display()
            ),
        })?;
        Ok(LifecycleLockGuard { _file: file })
    }

    fn lock_coordination(&self) -> ThreadStoreResult<File> {
        fs::create_dir_all(&self.directory).map_err(|err| ThreadStoreError::Internal {
            message: format!(
                "failed to create thread writer lock directory {}: {err}",
                self.directory.display()
            ),
        })?;
        let path = self.directory.join(COORDINATION_LOCK_FILE);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|err| ThreadStoreError::Internal {
                message: format!(
                    "failed to open thread writer coordination lock {}: {err}",
                    path.display()
                ),
            })?;
        file.lock().map_err(|err| ThreadStoreError::Internal {
            message: format!(
                "failed to acquire thread writer coordination lock {}: {err}",
                path.display()
            ),
        })?;
        Ok(file)
    }

    fn remove_stale_thread_locks(&self) -> io::Result<()> {
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            // Lifecycle leases are stable rendezvous files, not stale exclusive-writer markers.
            if file_name.ends_with(LIFECYCLE_LOCK_SUFFIX) {
                continue;
            }
            let Some(thread_id) = file_name.strip_suffix(".lock") else {
                continue;
            };
            if ThreadId::from_string(thread_id).is_err() {
                continue;
            }

            let path = entry.path();
            let file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => file,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => {
                    warn!(
                        "failed to inspect thread writer lock {}: {err}",
                        path.display()
                    );
                    continue;
                }
            };
            match file.try_lock() {
                Ok(()) => {
                    drop(file);
                    if let Err(err) = fs::remove_file(&path)
                        && err.kind() != io::ErrorKind::NotFound
                    {
                        warn!(
                            "failed to remove stale thread writer lock {}: {err}",
                            path.display()
                        );
                    }
                }
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(std::fs::TryLockError::Error(err)) => {
                    warn!(
                        "failed to inspect thread writer lock {}: {err}",
                        path.display()
                    );
                }
            }
        }
        Ok(())
    }
}

impl Drop for WriterLockGuard {
    fn drop(&mut self) {
        let coordination_lock = match self.coordinator.lock_coordination() {
            Ok(lock) => lock,
            Err(err) => {
                warn!("failed to coordinate thread writer lock cleanup: {err}");
                return;
            }
        };

        // Close the writer lock before deleting it so cleanup works on Windows too.
        drop(self.file.take());
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!(
                "failed to remove thread writer lock {}: {err}",
                self.path.display()
            );
        }
        drop(coordination_lock);
    }
}

#[cfg(test)]
#[path = "writer_lock_tests.rs"]
mod tests;
