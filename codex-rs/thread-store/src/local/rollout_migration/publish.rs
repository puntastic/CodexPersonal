//! Filesystem helpers for safely publishing migrated rollouts.
//!
//! This module owns temporary rollout paths, durable `.pending` journals, compressed rollout
//! staging, cleanup, and parent-directory syncs. The migration orchestrator decides when to call
//! these helpers; this module keeps the low-level file operations in one place.
//!
//! A migration can crash between publishing JSONL and finishing SQLite metadata. The journal is
//! the durable handoff between those steps, so cleanup must only remove it once the paginated
//! rollout and SQLite state are both complete.

use std::collections::HashSet;
use std::io;
use std::io::BufRead as StdBufRead;
use std::io::Read as StdRead;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

#[cfg(unix)]
use std::fs::Permissions;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use codex_protocol::ThreadId;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutLine;
use serde::Deserialize;
use serde::Serialize;
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::io::BufWriter;

use super::migration_error;
use crate::ThreadStoreError;
use crate::ThreadStoreResult;

const MIGRATION_JOURNAL_DIRECTORY: &str = "rollout-migrations";
const MIGRATION_JOURNAL_VERSION: u8 = 1;

/// Durable recovery handle written before conversion begins.
///
/// The preserved path is deterministic and intentionally survives successful publication. It
/// pins the legacy inode so an older writer that already has the file open cannot append into an
/// unlinked file after the paginated replacement becomes visible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct MigrationRecoveryHandle {
    version: u8,
    pub(super) rollout_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_rollout_path: Option<PathBuf>,
    pub(super) preserved_source_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) publication_fingerprint: Option<PreservedSourceFingerprint>,
    #[serde(default, skip_serializing_if = "is_false")]
    retirement_started: bool,
    /// Set durably when an older writer changes the preserved legacy generation after cutover.
    /// Once set, normal resume/write/retirement must stop and preserve both generations for an
    /// explicit reconciliation.
    #[serde(default, skip_serializing_if = "is_false")]
    split_generation_detected: bool,
}

/// Stable metadata captured after the legacy inode is pinned and before publication.
///
/// Legacy writers are append-only, so length catches their important cross-version mutation. The
/// modification timestamp also refuses retirement after an unexpected in-place rewrite that kept
/// the same length.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct PreservedSourceFingerprint {
    size_bytes: u64,
    modified_at_ns: i64,
}

/// Recovery artifacts qualified for removal by an authorized hard thread delete.
///
/// Preparation is deliberately read-only so a batch delete can validate every thread before
/// removing any recovery source. Callers must hold the rollout-maintenance lock and the thread's
/// writer lock from preparation through removal.
#[derive(Debug)]
pub(in crate::local) struct PreparedThreadDeleteMigrationArtifacts {
    thread_id: ThreadId,
    pending_journal_path: PathBuf,
    pending_staged_path: PathBuf,
    retained_journal_path: PathBuf,
    retained_journal_present: bool,
    retained_staged_path: PathBuf,
    recovery: Option<MigrationRecoveryHandle>,
}

// `serde(skip_serializing_if)` requires a predicate over `&T`, including copy types.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

impl MigrationRecoveryHandle {
    #[cfg(test)]
    pub(super) fn new(rollout_path: PathBuf, preserved_source_path: PathBuf) -> Self {
        Self {
            version: MIGRATION_JOURNAL_VERSION,
            rollout_path,
            published_rollout_path: None,
            preserved_source_path,
            publication_fingerprint: None,
            retirement_started: false,
            split_generation_detected: false,
        }
    }

    pub(super) fn new_with_published_path(
        rollout_path: PathBuf,
        published_rollout_path: PathBuf,
        preserved_source_path: PathBuf,
    ) -> Self {
        let published_rollout_path =
            (published_rollout_path != rollout_path).then_some(published_rollout_path);
        Self {
            version: MIGRATION_JOURNAL_VERSION,
            rollout_path,
            published_rollout_path,
            preserved_source_path,
            publication_fingerprint: None,
            retirement_started: false,
            split_generation_detected: false,
        }
    }

    pub(super) fn published_rollout_path(&self) -> &Path {
        self.published_rollout_path
            .as_deref()
            .unwrap_or(&self.rollout_path)
    }

    fn with_publication_fingerprint(mut self, fingerprint: PreservedSourceFingerprint) -> Self {
        self.publication_fingerprint = Some(fingerprint);
        self
    }

    fn begin_retirement(mut self) -> Self {
        self.retirement_started = true;
        self
    }

    fn mark_split_generation(mut self) -> Self {
        self.split_generation_detected = true;
        self
    }

    fn same_source_identity(&self, other: &Self) -> bool {
        self.version == other.version
            && self.rollout_path == other.rollout_path
            && self.published_rollout_path == other.published_rollout_path
            && self.preserved_source_path == other.preserved_source_path
    }
}

/// Validate a journal recovery handle before any of its paths are used.
///
/// Recovery journals are durable input, not authority. In particular, an absolute-path journal
/// copied with a Codex home must never reach back into the original home. Every path therefore has
/// to be the deterministic path for this migration, contained by this invocation's rollout roots,
/// and owned by the expected thread where a source is present.
pub(super) async fn validate_migration_recovery(
    codex_home: &Path,
    thread_id: ThreadId,
    journal_path: &Path,
    recovery: &MigrationRecoveryHandle,
    selected_published_path: Option<&Path>,
    require_publication_fingerprint: bool,
) -> ThreadStoreResult<()> {
    let expected_pending = migration_journal_path(codex_home, thread_id);
    let expected_retained = retained_migration_recovery_path(codex_home, thread_id);
    if journal_path != expected_pending && journal_path != expected_retained {
        return Err(recovery_conflict(
            thread_id,
            format!(
                "journal is outside this Codex home or names another thread: {}",
                journal_path.display()
            ),
        ));
    }
    let canonical_home = std::fs::canonicalize(codex_home).map_err(migration_error)?;
    let canonical_journal_root =
        std::fs::canonicalize(codex_home.join(MIGRATION_JOURNAL_DIRECTORY))
            .map_err(migration_error)?;
    let canonical_journal = std::fs::canonicalize(journal_path).map_err(migration_error)?;
    if !canonical_journal_root.starts_with(&canonical_home)
        || !canonical_journal.starts_with(&canonical_journal_root)
    {
        return Err(recovery_conflict(
            thread_id,
            format!(
                "journal resolves outside this Codex home: {}",
                journal_path.display()
            ),
        ));
    }
    if recovery.version != MIGRATION_JOURNAL_VERSION {
        return Err(recovery_conflict(
            thread_id,
            format!("journal uses unsupported version {}", recovery.version),
        ));
    }
    if recovery.retirement_started && recovery.split_generation_detected {
        return Err(recovery_conflict(
            thread_id,
            "journal simultaneously records retirement and a split generation".to_string(),
        ));
    }

    qualify_recovery_rollout_path(codex_home, thread_id, &recovery.rollout_path)?;
    let published_path = recovery.published_rollout_path();
    qualify_recovery_rollout_path(codex_home, thread_id, published_path)?;
    let expected_published_path = codex_rollout::plain_rollout_path(&recovery.rollout_path);
    if published_path != expected_published_path {
        return Err(recovery_conflict(
            thread_id,
            format!(
                "journal names a non-deterministic publication target: {}",
                published_path.display()
            ),
        ));
    }
    if let Some(selected_published_path) = selected_published_path {
        qualify_recovery_rollout_path(codex_home, thread_id, selected_published_path)?;
        let recorded_still_exists = published_path.exists();
        let selected_matches_recorded = codex_utils_path::paths_match_after_normalization(
            selected_published_path,
            published_path,
        );
        let same_generation_filename = codex_rollout::plain_rollout_path(selected_published_path)
            .file_name()
            == codex_rollout::plain_rollout_path(published_path).file_name();
        if !selected_matches_recorded && (recorded_still_exists || !same_generation_filename) {
            return Err(recovery_conflict(
                thread_id,
                format!(
                    "journal publication target does not match the selected rollout generation: {}",
                    published_path.display()
                ),
            ));
        }
        let selected_metadata = codex_rollout::read_session_meta_line(selected_published_path)
            .await
            .map_err(migration_error)?;
        if selected_metadata.meta.id != thread_id
            || selected_metadata.meta.history_mode
                != codex_protocol::protocol::ThreadHistoryMode::PaginatedRefsV1
        {
            return Err(recovery_conflict(
                thread_id,
                format!(
                    "selected rollout is not this thread's paginated_refs_v1 generation: {}",
                    selected_published_path.display()
                ),
            ));
        }
    }
    let expected_preserved_path = preserved_legacy_rollout_path(&recovery.rollout_path)?;
    if recovery.preserved_source_path != expected_preserved_path {
        return Err(recovery_conflict(
            thread_id,
            format!(
                "journal names a non-deterministic preserved source: {}",
                recovery.preserved_source_path.display()
            ),
        ));
    }
    qualify_recovery_rollout_path(codex_home, thread_id, &recovery.preserved_source_path)?;
    if require_publication_fingerprint && recovery.publication_fingerprint.is_none() {
        return Err(recovery_conflict(
            thread_id,
            "journal has no publication fingerprint".to_string(),
        ));
    }

    if tokio::fs::try_exists(&recovery.preserved_source_path)
        .await
        .map_err(migration_error)?
    {
        let preserved_thread_id = preserved_source_thread_id(recovery).await?;
        if preserved_thread_id != thread_id {
            return Err(recovery_conflict(
                thread_id,
                format!(
                    "preserved source belongs to {preserved_thread_id}: {}",
                    recovery.preserved_source_path.display()
                ),
            ));
        }
    }
    if tokio::fs::try_exists(published_path)
        .await
        .map_err(migration_error)?
    {
        let metadata = codex_rollout::read_session_meta_line(published_path)
            .await
            .map_err(migration_error)?;
        if metadata.meta.id != thread_id {
            return Err(recovery_conflict(
                thread_id,
                format!(
                    "published rollout belongs to {}: {}",
                    metadata.meta.id,
                    published_path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn recovery_conflict(thread_id: ThreadId, message: String) -> ThreadStoreError {
    ThreadStoreError::Conflict {
        message: format!("thread {thread_id} rollout migration recovery refused: {message}"),
    }
}

fn qualify_recovery_rollout_path(
    codex_home: &Path,
    thread_id: ThreadId,
    path: &Path,
) -> ThreadStoreResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(recovery_conflict(
            thread_id,
            format!(
                "journal path is not a normalized absolute path: {}",
                path.display()
            ),
        ));
    }
    let roots = [
        codex_home.join(codex_rollout::SESSIONS_SUBDIR),
        codex_home.join(codex_rollout::ARCHIVED_SESSIONS_SUBDIR),
    ];
    let containment_target = if path.exists() {
        std::fs::canonicalize(path).map_err(migration_error)?
    } else {
        let parent = path.parent().ok_or_else(|| {
            recovery_conflict(
                thread_id,
                format!("journal path has no parent: {}", path.display()),
            )
        })?;
        std::fs::canonicalize(parent).map_err(migration_error)?
    };
    for root in roots {
        match std::fs::canonicalize(root) {
            Ok(canonical_root) if containment_target.starts_with(&canonical_root) => return Ok(()),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(migration_error(error)),
        }
    }
    Err(recovery_conflict(
        thread_id,
        format!(
            "journal path resolves outside this Codex home: {}",
            path.display()
        ),
    ))
}

/// Fail closed when a migrated thread has an unfinished publication or a diverged legacy
/// generation. This is called while the thread's writer lock is held before resume and around
/// every durable write boundary.
pub(in crate::local) async fn ensure_migrated_generation_is_writable(
    codex_home: &Path,
    thread_id: ThreadId,
    selected_published_path: &Path,
) -> ThreadStoreResult<()> {
    let pending_path = migration_journal_path(codex_home, thread_id);
    if tokio::fs::try_exists(&pending_path)
        .await
        .map_err(migration_error)?
    {
        verify_migration_source_unchanged(
            codex_home,
            thread_id,
            &pending_path,
            selected_published_path,
        )
        .await?;
        return Err(recovery_conflict(
            thread_id,
            "publication recovery is still pending; finish recovery before resuming or writing"
                .to_string(),
        ));
    }
    let retained_path = retained_migration_recovery_path(codex_home, thread_id);
    verify_migration_source_unchanged(
        codex_home,
        thread_id,
        &retained_path,
        selected_published_path,
    )
    .await
}

/// Compare the preserved legacy inode with its cutover fingerprint and durably mark divergence.
/// The marker is monotonic: once either generation advances after cutover, neither branch may be
/// silently discarded or resumed as if it were complete.
pub(super) async fn verify_migration_source_unchanged(
    codex_home: &Path,
    thread_id: ThreadId,
    journal_path: &Path,
    selected_published_path: &Path,
) -> ThreadStoreResult<()> {
    let Some(recovery) = read_migration_journal_recovery(journal_path).await? else {
        return Ok(());
    };
    validate_migration_recovery(
        codex_home,
        thread_id,
        journal_path,
        &recovery,
        Some(selected_published_path),
        /*require_publication_fingerprint*/ true,
    )
    .await?;
    if recovery.split_generation_detected {
        return Err(recovery_conflict(
            thread_id,
            format!(
                "legacy and paginated generations have diverged; both were preserved for explicit reconciliation ({})",
                recovery.preserved_source_path.display()
            ),
        ));
    }
    if recovery.retirement_started {
        return Err(recovery_conflict(
            thread_id,
            "preserved source retirement is incomplete; finish recovery before writing".to_string(),
        ));
    }
    let Some(expected_fingerprint) = recovery.publication_fingerprint.as_ref() else {
        return Err(recovery_conflict(
            thread_id,
            "journal has no publication fingerprint".to_string(),
        ));
    };
    let current_fingerprint = preserved_source_fingerprint(&recovery.preserved_source_path).await?;
    if current_fingerprint == *expected_fingerprint {
        return Ok(());
    }

    let split = recovery.clone().mark_split_generation();
    write_migration_journal_with_recovery(journal_path, &split).await?;
    Err(recovery_conflict(
        thread_id,
        format!(
            "an older writer changed the preserved legacy generation after paginated cutover; both generations were preserved for explicit reconciliation ({})",
            recovery.preserved_source_path.display()
        ),
    ))
}

/// Read-only startup probe for retained generations. The actual migration pass takes the
/// maintenance and writer locks, persists the split marker, and emits the operator-visible
/// outcome.
pub(in crate::local) async fn retained_migration_requires_reconciliation(
    codex_home: &Path,
    thread_id: ThreadId,
    selected_published_path: &Path,
) -> ThreadStoreResult<bool> {
    let retained_path = retained_migration_recovery_path(codex_home, thread_id);
    let Some(recovery) = read_migration_journal_recovery(&retained_path).await? else {
        return Ok(false);
    };
    validate_migration_recovery(
        codex_home,
        thread_id,
        &retained_path,
        &recovery,
        Some(selected_published_path),
        /*require_publication_fingerprint*/ true,
    )
    .await?;
    if recovery.split_generation_detected || recovery.retirement_started {
        return Ok(true);
    }
    let Some(expected_fingerprint) = recovery.publication_fingerprint.as_ref() else {
        return Err(recovery_conflict(
            thread_id,
            "journal has no publication fingerprint".to_string(),
        ));
    };
    Ok(
        preserved_source_fingerprint(&recovery.preserved_source_path).await?
            != *expected_fingerprint,
    )
}

impl PreservedSourceFingerprint {
    pub(super) fn from_metadata(metadata: &std::fs::Metadata) -> ThreadStoreResult<Self> {
        let modified_at_ns = metadata
            .modified()
            .map_err(migration_error)?
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(migration_error)?
            .as_nanos();
        let modified_at_ns = i64::try_from(modified_at_ns).map_err(migration_error)?;
        Ok(Self {
            size_bytes: metadata.len(),
            modified_at_ns,
        })
    }
}

pub(super) fn migration_journal_path(codex_home: &Path, thread_id: ThreadId) -> PathBuf {
    codex_home
        .join(MIGRATION_JOURNAL_DIRECTORY)
        .join(format!("{thread_id}.pending"))
}

pub(super) fn retained_migration_recovery_path(codex_home: &Path, thread_id: ThreadId) -> PathBuf {
    codex_home
        .join(MIGRATION_JOURNAL_DIRECTORY)
        .join(format!("{thread_id}.retained"))
}

pub(super) async fn pending_migration_thread_ids(
    codex_home: &Path,
) -> ThreadStoreResult<HashSet<ThreadId>> {
    migration_thread_ids_with_suffix(codex_home, ".pending").await
}

pub(super) async fn retained_migration_thread_ids(
    codex_home: &Path,
) -> ThreadStoreResult<HashSet<ThreadId>> {
    migration_thread_ids_with_suffix(codex_home, ".retained").await
}

async fn migration_thread_ids_with_suffix(
    codex_home: &Path,
    suffix: &str,
) -> ThreadStoreResult<HashSet<ThreadId>> {
    let mut entries = match tokio::fs::read_dir(codex_home.join(MIGRATION_JOURNAL_DIRECTORY)).await
    {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(error) => return Err(migration_error(error)),
    };
    let mut thread_ids = HashSet::new();
    while let Some(entry) = entries.next_entry().await.map_err(migration_error)? {
        if !entry.file_type().await.map_err(migration_error)?.is_file() {
            continue;
        }
        let filename = entry.file_name();
        let Some(thread_id) = filename
            .to_str()
            .and_then(|filename| filename.strip_suffix(suffix))
            .and_then(|thread_id| ThreadId::from_string(thread_id).ok())
        else {
            continue;
        };
        thread_ids.insert(thread_id);
    }
    Ok(thread_ids)
}

pub(super) fn staged_rollout_path(rollout_path: &Path) -> ThreadStoreResult<PathBuf> {
    staged_path(rollout_path, "paginated")
}

pub(super) fn preserved_legacy_rollout_path(rollout_path: &Path) -> ThreadStoreResult<PathBuf> {
    let filename = rollout_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .ok_or_else(|| migration_error("rollout path has no valid filename"))?;
    Ok(rollout_path.with_file_name(format!(".{filename}.legacy-before-pagination")))
}

pub(super) fn decompressed_staged_rollout_path(rollout_path: &Path) -> ThreadStoreResult<PathBuf> {
    staged_path(rollout_path, "decompressed")
}

pub(super) fn compressed_staged_rollout_path(rollout_path: &Path) -> ThreadStoreResult<PathBuf> {
    staged_path(rollout_path, "paginated.zst")
}

fn staged_path(rollout_path: &Path, suffix: &str) -> ThreadStoreResult<PathBuf> {
    let filename = rollout_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .ok_or_else(|| migration_error("rollout path has no valid filename"))?;
    Ok(rollout_path.with_file_name(format!(".{filename}.{suffix}.tmp")))
}

fn migration_journal_staged_path(journal_path: &Path) -> ThreadStoreResult<PathBuf> {
    let filename = journal_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .ok_or_else(|| migration_error("rollout migration journal has no valid filename"))?;
    Ok(journal_path.with_file_name(format!(".{filename}.tmp")))
}

/// Qualifies the exact migration artifacts owned by one thread for hard deletion.
///
/// A pending or staged journal means publication or retirement may have been interrupted, so
/// hard-delete must fail before touching the visible rollout. A completed retained journal is
/// accepted only when its deterministic hidden source is inside this Codex home's rollout roots
/// and its SessionMeta belongs to `thread_id`.
pub(in crate::local) async fn prepare_thread_delete_migration_artifacts(
    codex_home: &Path,
    thread_id: ThreadId,
) -> ThreadStoreResult<PreparedThreadDeleteMigrationArtifacts> {
    let pending_journal_path = migration_journal_path(codex_home, thread_id);
    let pending_staged_path = migration_journal_staged_path(&pending_journal_path)?;
    if tokio::fs::try_exists(&pending_journal_path)
        .await
        .map_err(migration_error)?
        || tokio::fs::try_exists(&pending_staged_path)
            .await
            .map_err(migration_error)?
    {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "thread {thread_id} has a pending rollout migration; recover or clean up that migration before deleting the thread"
            ),
        });
    }

    let retained_journal_path = retained_migration_recovery_path(codex_home, thread_id);
    let retained_staged_path = migration_journal_staged_path(&retained_journal_path)?;
    if tokio::fs::try_exists(&retained_staged_path)
        .await
        .map_err(migration_error)?
    {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "thread {thread_id} has an incomplete retained rollout recovery update; recover it before deleting the thread"
            ),
        });
    }
    if !tokio::fs::try_exists(&retained_journal_path)
        .await
        .map_err(migration_error)?
    {
        return Ok(PreparedThreadDeleteMigrationArtifacts {
            thread_id,
            pending_journal_path,
            pending_staged_path,
            retained_journal_path,
            retained_journal_present: false,
            retained_staged_path,
            recovery: None,
        });
    }

    let recovery = read_migration_journal_recovery(&retained_journal_path).await?;
    let Some(recovery) = recovery else {
        // Empty journals came from the pre-recovery migration implementation and do not own a
        // preserved source. Removing the exact retained journal is sufficient.
        return Ok(PreparedThreadDeleteMigrationArtifacts {
            thread_id,
            pending_journal_path,
            pending_staged_path,
            retained_journal_path,
            retained_journal_present: true,
            retained_staged_path,
            recovery: None,
        });
    };
    validate_migration_recovery(
        codex_home,
        thread_id,
        &retained_journal_path,
        &recovery,
        /*selected_published_path*/ None,
        /*require_publication_fingerprint*/ true,
    )
    .await?;

    let preserved_exists = tokio::fs::try_exists(&recovery.preserved_source_path)
        .await
        .map_err(migration_error)?;
    if !preserved_exists && !recovery.retirement_started {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "thread {thread_id} retained rollout recovery source is missing before deletion: {}",
                recovery.preserved_source_path.display()
            ),
        });
    }

    Ok(PreparedThreadDeleteMigrationArtifacts {
        thread_id,
        pending_journal_path,
        pending_staged_path,
        retained_journal_path,
        retained_journal_present: true,
        retained_staged_path,
        recovery: Some(recovery),
    })
}

/// Removes a previously qualified retained source and its journal.
///
/// Marking retirement first makes a crash between source deletion and journal cleanup retryable.
/// Unlike ordinary retirement, hard-delete intentionally removes a changed late-writer source:
/// the caller has authorized deletion of the whole thread, not merely release of an unchanged
/// migration backup.
pub(in crate::local) async fn remove_thread_delete_migration_artifacts(
    prepared: PreparedThreadDeleteMigrationArtifacts,
) -> ThreadStoreResult<bool> {
    if tokio::fs::try_exists(&prepared.pending_journal_path)
        .await
        .map_err(migration_error)?
        || tokio::fs::try_exists(&prepared.pending_staged_path)
            .await
            .map_err(migration_error)?
        || tokio::fs::try_exists(&prepared.retained_staged_path)
            .await
            .map_err(migration_error)?
    {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "thread {} migration state changed while hard-delete was being prepared",
                prepared.thread_id
            ),
        });
    }
    let retained_exists = tokio::fs::try_exists(&prepared.retained_journal_path)
        .await
        .map_err(migration_error)?;
    if retained_exists != prepared.retained_journal_present {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "thread {} retained migration journal changed while hard-delete was being prepared",
                prepared.thread_id
            ),
        });
    }
    if !retained_exists {
        return Ok(false);
    }
    let retained_journal_path = prepared.retained_journal_path;
    let current_recovery = read_migration_journal_recovery(&retained_journal_path).await?;
    if current_recovery != prepared.recovery {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "thread {} retained migration recovery changed while hard-delete was being prepared",
                prepared.thread_id
            ),
        });
    }
    let thread_id = prepared.thread_id;
    if let Some(recovery) = current_recovery {
        let retiring = recovery.clone().begin_retirement();
        if !recovery.retirement_started {
            write_migration_journal_with_recovery(&retained_journal_path, &retiring).await?;
        }
        if tokio::fs::try_exists(&recovery.preserved_source_path)
            .await
            .map_err(migration_error)?
        {
            let preserved_thread_id = preserved_source_thread_id(&recovery).await?;
            if preserved_thread_id != thread_id {
                return Err(ThreadStoreError::Conflict {
                    message: format!(
                        "thread {thread_id} retained rollout recovery source changed ownership before deletion: {}",
                        recovery.preserved_source_path.display()
                    ),
                });
            }
            tokio::fs::remove_file(&recovery.preserved_source_path)
                .await
                .map_err(migration_error)?;
            sync_parent_directory(&recovery.preserved_source_path).await?;
        }
    }
    remove_migration_journal(&retained_journal_path).await?;
    Ok(true)
}

async fn preserved_source_thread_id(
    recovery: &MigrationRecoveryHandle,
) -> ThreadStoreResult<ThreadId> {
    let path = recovery.preserved_source_path.clone();
    let compressed = recovery
        .rollout_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".jsonl.zst"));
    tokio::task::spawn_blocking(move || -> ThreadStoreResult<ThreadId> {
        let file = std::fs::File::open(&path).map_err(migration_error)?;
        let source: Box<dyn StdRead> = if compressed {
            Box::new(zstd::stream::read::Decoder::new(file).map_err(migration_error)?)
        } else {
            Box::new(file)
        };
        let reader = std::io::BufReader::new(source);
        for line in reader.lines() {
            let line = line.map_err(migration_error)?;
            if line.trim().is_empty() {
                continue;
            }
            let value =
                serde_json::from_str::<serde_json::Value>(&line).map_err(migration_error)?;
            if value.get("type").and_then(serde_json::Value::as_str) != Some("session_meta") {
                continue;
            }
            let thread_id = value
                .get("payload")
                .and_then(|payload| payload.get("id"))
                .and_then(serde_json::Value::as_str)
                .and_then(|thread_id| ThreadId::from_string(thread_id).ok())
                .ok_or_else(|| migration_error("preserved rollout SessionMeta has no valid id"))?;
            return Ok(thread_id);
        }
        Err(migration_error(format!(
            "preserved rollout has no SessionMeta: {}",
            path.display()
        )))
    })
    .await
    .map_err(migration_error)?
}

fn retained_path_from_pending_journal(journal_path: &Path) -> ThreadStoreResult<PathBuf> {
    let filename = journal_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .and_then(|filename| filename.strip_suffix(".pending"))
        .ok_or_else(|| migration_error("rollout migration journal is not a pending journal"))?;
    Ok(journal_path.with_file_name(format!("{filename}.retained")))
}

pub(super) async fn decompress_rollout_to_path(
    compressed_path: &Path,
    plain_path: &Path,
) -> ThreadStoreResult<()> {
    let compressed_path = compressed_path.to_path_buf();
    let plain_path = plain_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> io::Result<()> {
        let input = std::fs::File::open(compressed_path)?;
        let mut decoder = zstd::stream::read::Decoder::new(input)?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut output = options.open(plain_path)?;
        #[cfg(unix)]
        output.set_permissions(Permissions::from_mode(0o600))?;
        io::copy(&mut decoder, &mut output)?;
        output.flush()
    })
    .await
    .map_err(migration_error)?
    .map_err(migration_error)
}

pub(super) fn rewritten_staged_rollout_path(staged_path: &Path) -> ThreadStoreResult<PathBuf> {
    let filename = staged_path
        .file_name()
        .and_then(|filename| filename.to_str())
        .ok_or_else(|| migration_error("staged rollout path has no valid filename"))?;
    Ok(staged_path.with_file_name(format!("{filename}.head.tmp")))
}

pub(super) async fn rewrite_subagent_history_boundary(
    staged_path: &Path,
    boundary: u64,
) -> ThreadStoreResult<()> {
    let rewritten_path = rewritten_staged_rollout_path(staged_path)?;
    let permissions = tokio::fs::metadata(staged_path)
        .await
        .map_err(migration_error)?
        .permissions();
    let source = File::open(staged_path).await.map_err(migration_error)?;
    let mut source = BufReader::new(source);
    let mut head_bytes = Vec::new();
    source
        .read_until(b'\n', &mut head_bytes)
        .await
        .map_err(migration_error)?;
    let mut head = serde_json::from_slice::<RolloutLine>(&head_bytes).map_err(migration_error)?;
    let RolloutItem::SessionMeta(session_meta) = &mut head.item else {
        return Err(migration_error(
            "staged rollout head is not session metadata",
        ));
    };
    session_meta.meta.subagent_history_start_ordinal = Some(boundary);

    let rewritten = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&rewritten_path)
        .await
        .map_err(migration_error)?;
    rewritten
        .set_permissions(permissions)
        .await
        .map_err(migration_error)?;
    let mut rewritten = BufWriter::new(rewritten);
    rewritten
        .write_all(
            serde_json::to_string(&head)
                .map_err(migration_error)?
                .as_bytes(),
        )
        .await
        .map_err(migration_error)?;
    rewritten.write_all(b"\n").await.map_err(migration_error)?;
    tokio::io::copy(&mut source, &mut rewritten)
        .await
        .map_err(migration_error)?;
    rewritten.flush().await.map_err(migration_error)?;
    rewritten
        .get_ref()
        .sync_all()
        .await
        .map_err(migration_error)?;
    drop(rewritten);
    tokio::fs::rename(rewritten_path, staged_path)
        .await
        .map_err(migration_error)
}

/// Pins the current source inode at a deterministic recovery path.
///
/// Reusing the same hard link after a crash is safe and idempotent. A distinct file already at the
/// recovery path is never overwritten: doing so could erase the only copy receiving writes from
/// an older process.
pub(super) async fn preserve_rollout_source(
    rollout_path: &Path,
    preserved_source_path: &Path,
) -> ThreadStoreResult<()> {
    let rollout_path = rollout_path.to_path_buf();
    let preserved_source_path = preserved_source_path.to_path_buf();
    let preserved_source_sync_path = preserved_source_path.clone();
    let created = tokio::task::spawn_blocking(move || -> ThreadStoreResult<bool> {
        match std::fs::hard_link(&rollout_path, &preserved_source_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if !same_file::is_same_file(&rollout_path, &preserved_source_path)
                    .map_err(migration_error)?
                {
                    return Err(ThreadStoreError::Conflict {
                        message: format!(
                            "rollout migration recovery path already preserves a different file: {}",
                            preserved_source_path.display()
                        ),
                    });
                }
                return Ok(false);
            }
            Err(error) => return Err(migration_error(error)),
        }

        if !same_file::is_same_file(&rollout_path, &preserved_source_path)
            .map_err(migration_error)?
        {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "rollout changed while its recovery hard link was being established: {}",
                    preserved_source_path.display()
                ),
            });
        }
        Ok(true)
    })
    .await
    .map_err(migration_error)??;
    if created {
        sync_parent_directory(&preserved_source_sync_path).await?;
    }
    Ok(())
}

/// Removes an unpublished recovery alias only when the visible legacy path still names the same
/// inode. A distinct recovery file is never treated as cleanup debris.
pub(super) async fn remove_preserved_source_alias_if_present(
    rollout_path: &Path,
    preserved_source_path: &Path,
) -> ThreadStoreResult<()> {
    if !tokio::fs::try_exists(preserved_source_path)
        .await
        .map_err(migration_error)?
    {
        return Ok(());
    }
    if !same_file::is_same_file(rollout_path, preserved_source_path).map_err(migration_error)? {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "unpublished migration recovery path preserves a different inode: {}",
                preserved_source_path.display()
            ),
        });
    }
    tokio::fs::remove_file(preserved_source_path)
        .await
        .map_err(migration_error)?;
    sync_parent_directory(preserved_source_path).await
}

pub(super) async fn preserved_source_fingerprint(
    path: &Path,
) -> ThreadStoreResult<PreservedSourceFingerprint> {
    let metadata = tokio::fs::metadata(path).await.map_err(migration_error)?;
    PreservedSourceFingerprint::from_metadata(&metadata)
}

/// Publishes staged bytes only after the legacy inode has a durable, deterministic name and its
/// retirement fingerprint has reached the recovery journal.
pub(super) async fn publish_staged_rollout(
    rollout_path: &Path,
    published_rollout_path: &Path,
    staged_path: &Path,
    preserved_source_path: &Path,
    journal_path: &Path,
    expected_source_fingerprint: &PreservedSourceFingerprint,
) -> ThreadStoreResult<MigrationRecoveryHandle> {
    if published_rollout_path != rollout_path
        && tokio::fs::try_exists(published_rollout_path)
            .await
            .map_err(migration_error)?
    {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "plain rollout publication target already exists: {}",
                published_rollout_path.display()
            ),
        });
    }
    preserve_rollout_source(rollout_path, preserved_source_path).await?;
    let pinned_fingerprint = preserved_source_fingerprint(preserved_source_path).await?;
    if pinned_fingerprint != *expected_source_fingerprint {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "rollout changed before its recovery inode was pinned; close older Codex processes and retry: {}",
                rollout_path.display()
            ),
        });
    }
    let recovery = MigrationRecoveryHandle::new_with_published_path(
        rollout_path.to_path_buf(),
        published_rollout_path.to_path_buf(),
        preserved_source_path.to_path_buf(),
    )
    .with_publication_fingerprint(expected_source_fingerprint.clone());
    write_migration_journal_with_recovery(journal_path, &recovery).await?;
    tokio::fs::rename(staged_path, published_rollout_path)
        .await
        .map_err(migration_error)?;
    sync_parent_directory(published_rollout_path).await?;
    Ok(recovery)
}

pub(super) async fn remove_file_if_present(path: &Path) -> ThreadStoreResult<()> {
    if tokio::fs::try_exists(path).await.map_err(migration_error)? {
        tokio::fs::remove_file(path)
            .await
            .map_err(migration_error)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) async fn write_migration_journal(path: &Path) -> ThreadStoreResult<()> {
    write_migration_journal_bytes(path, &[]).await
}

pub(super) async fn write_migration_journal_with_recovery(
    path: &Path,
    recovery: &MigrationRecoveryHandle,
) -> ThreadStoreResult<()> {
    if let Some(existing) = read_migration_journal_recovery(path).await? {
        if existing == *recovery {
            return Ok(());
        }
        if !existing.same_source_identity(recovery) {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "rollout migration journal already names a different recovery path: {}",
                    existing.preserved_source_path.display()
                ),
            });
        }
        if existing.retirement_started && !recovery.retirement_started {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "rollout migration recovery source retirement is already in progress: {}",
                    existing.preserved_source_path.display()
                ),
            });
        }
        if existing.split_generation_detected && !recovery.split_generation_detected {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "rollout migration split-generation marker cannot be cleared implicitly: {}",
                    existing.preserved_source_path.display()
                ),
            });
        }
        if existing.publication_fingerprint.is_some() && recovery.publication_fingerprint.is_none()
        {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "rollout migration publication fingerprint cannot be cleared: {}",
                    existing.preserved_source_path.display()
                ),
            });
        }
        if recovery.retirement_started && recovery.split_generation_detected {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "split rollout generations cannot enter automatic retirement: {}",
                    existing.preserved_source_path.display()
                ),
            });
        }
        if let (Some(existing_fingerprint), Some(recovery_fingerprint)) = (
            existing.publication_fingerprint.as_ref(),
            recovery.publication_fingerprint.as_ref(),
        ) && existing_fingerprint != recovery_fingerprint
        {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "rollout migration publication fingerprint changed: {}",
                    existing.preserved_source_path.display()
                ),
            });
        }
    }

    let mut bytes = serde_json::to_vec(recovery).map_err(migration_error)?;
    bytes.push(b'\n');
    write_migration_journal_bytes(path, &bytes).await
}

/// Moves a completed publication journal into one deterministic retained-recovery record.
///
/// Empty journals came from the older migration implementation and have no preserved inode to
/// manage, so they keep their former cleanup behavior.
pub(super) async fn retain_migration_recovery(
    pending_journal_path: &Path,
) -> ThreadStoreResult<Option<PathBuf>> {
    let Some(recovery) = read_migration_journal_recovery(pending_journal_path).await? else {
        remove_migration_journal(pending_journal_path).await?;
        return Ok(None);
    };
    let retained_path = retained_path_from_pending_journal(pending_journal_path)?;
    if tokio::fs::try_exists(&retained_path)
        .await
        .map_err(migration_error)?
    {
        let existing = read_migration_journal_recovery(&retained_path)
            .await?
            .ok_or_else(|| {
                migration_error("retained rollout migration recovery journal is empty")
            })?;
        if existing != recovery {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "retained rollout migration recovery record conflicts with pending publication: {}",
                    retained_path.display()
                ),
            });
        }
        remove_migration_journal(pending_journal_path).await?;
        return Ok(Some(retained_path));
    }

    tokio::fs::rename(pending_journal_path, &retained_path)
        .await
        .map_err(migration_error)?;
    remove_file_if_present(&migration_journal_staged_path(pending_journal_path)?).await?;
    sync_parent_directory(&retained_path).await?;
    Ok(Some(retained_path))
}

/// Retires one preserved legacy inode after the caller has verified the visible rollout's gated
/// format and explicitly asserted that older writers are quiescent.
///
/// A durable `retirement_started` transition makes deletion retryable without treating a missing
/// source as proof that an unrelated process safely retired it.
pub(super) async fn retire_preserved_source(
    current_rollout_path: &Path,
    retained_journal_path: &Path,
) -> ThreadStoreResult<Option<PathBuf>> {
    let Some(recovery) = read_migration_journal_recovery(retained_journal_path).await? else {
        return Ok(None);
    };
    if recovery.split_generation_detected {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "preserved rollout source belongs to a split generation and cannot be retired before reconciliation: {}",
                recovery.preserved_source_path.display()
            ),
        });
    }
    let Some(expected_fingerprint) = recovery.publication_fingerprint.clone() else {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "preserved rollout source has no publication fingerprint and cannot be retired automatically: {}",
                recovery.preserved_source_path.display()
            ),
        });
    };
    let preserved_source_exists = tokio::fs::try_exists(&recovery.preserved_source_path)
        .await
        .map_err(migration_error)?;
    if !preserved_source_exists {
        if recovery.retirement_started {
            remove_migration_journal(retained_journal_path).await?;
            return Ok(Some(recovery.preserved_source_path));
        }
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "preserved rollout source disappeared before verified retirement: {}",
                recovery.preserved_source_path.display()
            ),
        });
    }
    if same_file::is_same_file(current_rollout_path, &recovery.preserved_source_path)
        .map_err(migration_error)?
    {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "visible rollout still names the legacy inode; publication is not complete: {}",
                current_rollout_path.display()
            ),
        });
    }
    let current_fingerprint = preserved_source_fingerprint(&recovery.preserved_source_path).await?;
    if current_fingerprint != expected_fingerprint {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "preserved rollout source changed after publication; reconcile late writer bytes before retirement: {}",
                recovery.preserved_source_path.display()
            ),
        });
    }

    let retiring = recovery.clone().begin_retirement();
    write_migration_journal_with_recovery(retained_journal_path, &retiring).await?;
    let final_fingerprint = preserved_source_fingerprint(&recovery.preserved_source_path).await?;
    if final_fingerprint != expected_fingerprint {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "preserved rollout source changed while retirement was starting; reconcile late writer bytes: {}",
                recovery.preserved_source_path.display()
            ),
        });
    }
    tokio::fs::remove_file(&recovery.preserved_source_path)
        .await
        .map_err(migration_error)?;
    sync_parent_directory(&recovery.preserved_source_path).await?;
    remove_migration_journal(retained_journal_path).await?;
    Ok(Some(recovery.preserved_source_path))
}

pub(super) async fn read_migration_journal_recovery(
    path: &Path,
) -> ThreadStoreResult<Option<MigrationRecoveryHandle>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(migration_error(error)),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(migration_error)
}

pub(super) async fn remove_migration_journal(path: &Path) -> ThreadStoreResult<()> {
    remove_file_if_present(path).await?;
    remove_file_if_present(&migration_journal_staged_path(path)?).await?;
    sync_parent_directory(path).await
}

async fn write_migration_journal_bytes(path: &Path, bytes: &[u8]) -> ThreadStoreResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| migration_error("rollout migration journal has no parent directory"))?;
    let parent_already_exists = tokio::fs::try_exists(parent)
        .await
        .map_err(migration_error)?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(migration_error)?;
    if !parent_already_exists {
        sync_parent_directory(parent).await?;
    }
    let staged_path = migration_journal_staged_path(path)?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&staged_path)
        .await
        .map_err(migration_error)?;
    if !bytes.is_empty() {
        file.write_all(bytes).await.map_err(migration_error)?;
    }
    file.sync_all().await.map_err(migration_error)?;
    drop(file);
    tokio::fs::rename(&staged_path, path)
        .await
        .map_err(migration_error)?;
    sync_parent_directory(path).await
}

pub(super) async fn sync_parent_directory(path: &Path) -> ThreadStoreResult<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .ok_or_else(|| migration_error("rollout path has no parent directory"))?
            .to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all())
            .await
            .map_err(migration_error)?
            .map_err(migration_error)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::MigrationRecoveryHandle;
    use super::PreservedSourceFingerprint;
    use super::preserve_rollout_source;
    use super::preserved_source_fingerprint;
    use super::publish_staged_rollout;
    use super::read_migration_journal_recovery;
    use super::retain_migration_recovery;
    use super::retire_preserved_source;
    use super::write_migration_journal_with_recovery;
    use crate::ThreadStoreError;

    fn paths(home: &TempDir) -> (PathBuf, PathBuf, PathBuf) {
        (
            home.path().join("rollout.jsonl"),
            home.path().join(".rollout.jsonl.paginated.tmp"),
            home.path().join(".rollout.jsonl.legacy-before-pagination"),
        )
    }

    #[tokio::test]
    async fn append_between_final_validation_and_preservation_blocks_publication() {
        let home = TempDir::new().expect("create temp directory");
        let (rollout_path, staged_path, preserved_path) = paths(&home);
        let journal_path = home.path().join("thread.pending");
        std::fs::write(&rollout_path, b"legacy\n").expect("write legacy source");
        std::fs::write(&staged_path, b"paginated\n").expect("write staged replacement");

        // This metadata read stands in for migration's final validation. The append is
        // deliberately sequenced after it and before the publication boundary.
        let validated_metadata =
            std::fs::metadata(&rollout_path).expect("validate source metadata");
        let validated_fingerprint = PreservedSourceFingerprint::from_metadata(&validated_metadata)
            .expect("fingerprint validated source");
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout_path)
            .expect("open racing writer");
        writer.write_all(b"raced append\n").expect("append race");
        writer.sync_all().expect("sync racing append");
        let error = publish_staged_rollout(
            &rollout_path,
            &rollout_path,
            &staged_path,
            &preserved_path,
            &journal_path,
            &validated_fingerprint,
        )
        .await
        .expect_err("late pre-preservation append must block publication");

        assert!(matches!(error, ThreadStoreError::Conflict { .. }));
        assert_eq!(
            std::fs::read(&rollout_path).expect("read visible legacy rollout"),
            b"legacy\nraced append\n"
        );
        assert_eq!(
            std::fs::read(&preserved_path).expect("read preserved source"),
            b"legacy\nraced append\n"
        );
        assert_eq!(
            std::fs::read(&staged_path).expect("read unpublished staged rollout"),
            b"paginated\n"
        );
    }

    #[tokio::test]
    async fn already_open_writer_appends_to_preserved_inode_after_publish() {
        let home = TempDir::new().expect("create temp directory");
        let (rollout_path, staged_path, preserved_path) = paths(&home);
        let journal_path = home.path().join("thread.pending");
        std::fs::write(&rollout_path, b"legacy\n").expect("write legacy source");
        std::fs::write(&staged_path, b"paginated\n").expect("write staged replacement");
        let expected_fingerprint = preserved_source_fingerprint(&rollout_path)
            .await
            .expect("fingerprint legacy source");
        let mut old_writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout_path)
            .expect("open legacy writer before publication");

        publish_staged_rollout(
            &rollout_path,
            &rollout_path,
            &staged_path,
            &preserved_path,
            &journal_path,
            &expected_fingerprint,
        )
        .await
        .expect("publish with open legacy writer");
        old_writer
            .write_all(b"late old-inode append\n")
            .expect("append through old handle");
        old_writer.sync_all().expect("sync late append");

        assert_eq!(
            std::fs::read(&rollout_path).expect("read published rollout"),
            b"paginated\n"
        );
        assert_eq!(
            std::fs::read(&preserved_path).expect("read preserved source"),
            b"legacy\nlate old-inode append\n"
        );
    }

    #[tokio::test]
    async fn unchanged_preserved_source_retires_after_explicit_second_phase() {
        let home = TempDir::new().expect("create temp directory");
        let (rollout_path, staged_path, preserved_path) = paths(&home);
        let pending_path = home.path().join("thread.pending");
        std::fs::write(&rollout_path, b"legacy\n").expect("write legacy source");
        std::fs::write(&staged_path, b"paginated\n").expect("write staged replacement");
        let expected_fingerprint = preserved_source_fingerprint(&rollout_path)
            .await
            .expect("fingerprint legacy source");

        publish_staged_rollout(
            &rollout_path,
            &rollout_path,
            &staged_path,
            &preserved_path,
            &pending_path,
            &expected_fingerprint,
        )
        .await
        .expect("publish rollout");
        let retained_path = retain_migration_recovery(&pending_path)
            .await
            .expect("retain recovery record")
            .expect("new recovery record");

        assert_eq!(
            retire_preserved_source(&rollout_path, &retained_path)
                .await
                .expect("retire unchanged source"),
            Some(preserved_path.clone())
        );
        assert!(!preserved_path.exists());
        assert!(!retained_path.exists());
    }

    #[tokio::test]
    async fn late_old_handle_append_refuses_retirement_and_keeps_recovery() {
        let home = TempDir::new().expect("create temp directory");
        let (rollout_path, staged_path, preserved_path) = paths(&home);
        let pending_path = home.path().join("thread.pending");
        std::fs::write(&rollout_path, b"legacy\n").expect("write legacy source");
        std::fs::write(&staged_path, b"paginated\n").expect("write staged replacement");
        let expected_fingerprint = preserved_source_fingerprint(&rollout_path)
            .await
            .expect("fingerprint legacy source");
        let mut old_writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&rollout_path)
            .expect("open old writer");

        publish_staged_rollout(
            &rollout_path,
            &rollout_path,
            &staged_path,
            &preserved_path,
            &pending_path,
            &expected_fingerprint,
        )
        .await
        .expect("publish rollout");
        let retained_path = retain_migration_recovery(&pending_path)
            .await
            .expect("retain recovery record")
            .expect("new recovery record");
        old_writer
            .write_all(b"late bytes\n")
            .expect("append through old handle");
        old_writer.sync_all().expect("sync late append");

        let error = retire_preserved_source(&rollout_path, &retained_path)
            .await
            .expect_err("late bytes must refuse retirement");
        assert!(matches!(error, ThreadStoreError::Conflict { .. }));
        assert!(preserved_path.exists());
        assert!(retained_path.exists());
        assert_eq!(
            std::fs::read(&preserved_path).expect("read retained late bytes"),
            b"legacy\nlate bytes\n"
        );
    }

    #[tokio::test]
    async fn pre_publish_crash_retry_reuses_one_deterministic_preserved_link() {
        let home = TempDir::new().expect("create temp directory");
        let (rollout_path, _staged_path, preserved_path) = paths(&home);
        std::fs::write(&rollout_path, b"legacy\n").expect("write legacy source");

        preserve_rollout_source(&rollout_path, &preserved_path)
            .await
            .expect("establish first preservation link");
        // Simulate restart after preservation but before publication.
        preserve_rollout_source(&rollout_path, &preserved_path)
            .await
            .expect("reuse preservation link");

        assert!(
            same_file::is_same_file(&rollout_path, &preserved_path).expect("compare hard links")
        );
        assert_eq!(
            std::fs::read_dir(home.path())
                .expect("list temp directory")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn distinct_existing_recovery_file_fails_closed() {
        let home = TempDir::new().expect("create temp directory");
        let (rollout_path, _staged_path, preserved_path) = paths(&home);
        std::fs::write(&rollout_path, b"current legacy\n").expect("write legacy source");
        std::fs::write(&preserved_path, b"older preserved bytes\n")
            .expect("write conflicting recovery file");

        let error = preserve_rollout_source(&rollout_path, &preserved_path)
            .await
            .expect_err("distinct recovery file must block publication");

        assert!(matches!(error, ThreadStoreError::Conflict { .. }));
        assert_eq!(
            std::fs::read(&rollout_path).expect("read unchanged source"),
            b"current legacy\n"
        );
        assert_eq!(
            std::fs::read(&preserved_path).expect("read unchanged recovery file"),
            b"older preserved bytes\n"
        );
    }

    #[tokio::test]
    async fn recovery_journal_round_trips_one_stable_handle() {
        let home = TempDir::new().expect("create temp directory");
        let (rollout_path, _staged_path, preserved_path) = paths(&home);
        let journal_path = home.path().join("thread.pending");
        let recovery = MigrationRecoveryHandle::new(rollout_path, preserved_path);

        write_migration_journal_with_recovery(&journal_path, &recovery)
            .await
            .expect("write recovery journal");
        write_migration_journal_with_recovery(&journal_path, &recovery)
            .await
            .expect("reuse recovery journal");

        assert_eq!(
            read_migration_journal_recovery(&journal_path)
                .await
                .expect("read recovery journal"),
            Some(recovery)
        );
    }
}
