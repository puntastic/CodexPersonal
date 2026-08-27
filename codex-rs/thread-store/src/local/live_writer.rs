use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_rollout::RolloutConfig;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutRecorder;
use codex_rollout::RolloutRecorderParams;
use codex_rollout::is_persisted_rollout_item;
use tracing::warn;

use super::LocalThreadStore;
use super::create_thread;
use crate::AppendThreadItemsParams;
use crate::CreateThreadParams;
use crate::ReadThreadParams;
use crate::ResumeThreadParams;
use crate::ThreadStoreError;
use crate::ThreadStoreResult;

const ROLLOUT_SIZE_BYTES_METRIC: &str = "codex.rollout.size_bytes";

pub(super) async fn create_thread(
    store: &LocalThreadStore,
    params: CreateThreadParams,
) -> ThreadStoreResult<()> {
    let thread_id = params.thread_id;
    let _live_writer_guard = store.live_writer_locks.lock(thread_id).await;
    let history_mode = params.history_mode;
    store.ensure_live_recorder_absent(thread_id).await?;
    let writer_lock = store.writer_lock_coordinator.acquire(thread_id)?;
    let recorder = create_thread::create_thread(store, params).await?;
    store
        .insert_live_recorder(thread_id, recorder, thread_id, history_mode, writer_lock)
        .await
}

pub(super) async fn resume_thread(
    store: &LocalThreadStore,
    params: ResumeThreadParams,
) -> ThreadStoreResult<()> {
    let ResumeThreadParams {
        thread_id,
        rollout_path,
        history,
        include_archived,
        metadata,
    } = params;
    let _live_writer_guard = store.live_writer_locks.lock(thread_id).await;
    store.ensure_live_recorder_absent(thread_id).await?;
    let writer_lock = store.writer_lock_coordinator.acquire(thread_id)?;

    // The writer lock makes the selected rollout head stable while we reopen it. Caller-supplied
    // replay can have been loaded before a migration or revert, so it is evidence to validate, not
    // authority for the on-disk writer generation.
    let rollout_path = if let Some(rollout_path) = rollout_path {
        let thread = super::read_thread::read_thread_by_rollout_path(
            store,
            rollout_path,
            include_archived,
            /*include_history*/ false,
        )
        .await?;
        let rollout_path = thread
            .rollout_path
            .ok_or_else(|| ThreadStoreError::Internal {
                message: format!("thread {thread_id} does not have a rollout path"),
            })?;
        ensure_requested_rollout_is_current(store, thread_id, rollout_path.as_path()).await?;
        rollout_path
    } else {
        let thread = super::read_thread::read_thread(
            store,
            ReadThreadParams {
                thread_id,
                include_archived,
                include_history: false,
            },
        )
        .await?;
        thread
            .rollout_path
            .ok_or_else(|| ThreadStoreError::Internal {
                message: format!("thread {thread_id} does not have a rollout path"),
            })?
    };
    let canonical_meta = codex_rollout::read_session_meta_line(rollout_path.as_path())
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!(
                "failed to read canonical session metadata {}: {err}",
                rollout_path.display()
            ),
        })?
        .meta;
    if canonical_meta.id != thread_id {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "rollout {} now belongs to thread {}, expected {thread_id}",
                rollout_path.display(),
                canonical_meta.id
            ),
        });
    }
    let history_mode = canonical_meta.history_mode;
    if history_mode == ThreadHistoryMode::PaginatedRefsV1 {
        super::rollout_migration::ensure_migrated_generation_is_writable(
            &store.config.codex_home,
            thread_id,
            rollout_path.as_path(),
        )
        .await?;
    }
    let rollout_id = super::thread_rollout_resolver::rollout_id_from_path_or_legacy_thread_id(
        rollout_path.as_path(),
        thread_id,
        history_mode,
    )?;
    if let Some(history) = history.as_deref() {
        let Some(supplied_meta) = history.iter().find_map(|item| match item {
            RolloutItem::SessionMeta(meta_line) => Some(&meta_line.meta),
            _ => None,
        }) else {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "supplied replay for thread {thread_id} has no canonical session metadata; reload before resuming"
                ),
            });
        };
        if supplied_meta.id != thread_id || supplied_meta.history_mode != history_mode {
            return Err(ThreadStoreError::Conflict {
                message: format!(
                    "supplied replay for thread {thread_id} is stale (id {}, mode {}); canonical head uses id {}, mode {}; reload before resuming",
                    supplied_meta.id,
                    supplied_meta.history_mode.as_str(),
                    canonical_meta.id,
                    history_mode.as_str()
                ),
            });
        }
        if history_mode.is_paginated() {
            let current_context = super::model_context::load_latest_model_context_from_rollout(
                store,
                thread_id,
                rollout_id,
                rollout_path.clone(),
            )
            .await?;
            let matches_model_context = supplied_history_matches_current(
                thread_id,
                history,
                current_context.items.as_slice(),
            )?;
            let matches_raw_rollout = if matches_model_context {
                false
            } else {
                codex_rollout::rollout_items_match_file(rollout_path.as_path(), history)
                    .await
                    .map_err(|err| ThreadStoreError::Internal {
                        message: format!(
                            "failed to compare supplied replay with selected rollout for thread {thread_id}: {err}"
                        ),
                    })?
            };
            if !matches_model_context && !matches_raw_rollout {
                return Err(ThreadStoreError::Conflict {
                    message: format!(
                        "supplied replay for thread {thread_id} is stale ({} items versus {} in the current model context); reload before resuming",
                        history.len(),
                        current_context.items.len()
                    ),
                });
            }
        }
    }
    let cwd = metadata
        .cwd
        .clone()
        .ok_or_else(|| ThreadStoreError::InvalidRequest {
            message: "local thread store requires a cwd".to_string(),
        })?;
    let config = RolloutConfig {
        codex_home: store.config.codex_home.clone(),
        sqlite: store.config.sqlite.clone(),
        cwd,
        model_provider_id: metadata.model_provider.clone(),
        generate_memories: matches!(metadata.memory_mode, ThreadMemoryMode::Enabled),
    };
    let recorder = RolloutRecorder::new(&config, RolloutRecorderParams::resume(rollout_path))
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!("failed to resume local thread recorder: {err}"),
        })?;
    store
        .insert_live_recorder(thread_id, recorder, rollout_id, history_mode, writer_lock)
        .await
}

/// Caller-supplied paginated replay is a snapshot taken before the writer lock. Reload the exact
/// selected rollout while holding that lock and reject a same-path append or rewrite rather than
/// opening a writer with stale model context. Comparing one serialized item at a time keeps the
/// additional memory bound to the largest replay item instead of duplicating the whole context.
fn supplied_history_matches_current(
    thread_id: ThreadId,
    supplied: &[RolloutItem],
    current: &[RolloutItem],
) -> ThreadStoreResult<bool> {
    if supplied.len() != current.len() {
        return Ok(false);
    }
    for (index, (supplied_item, current_item)) in supplied.iter().zip(current).enumerate() {
        let supplied_item =
            serde_json::to_value(supplied_item).map_err(|err| ThreadStoreError::Internal {
                message: format!(
                    "failed to compare supplied replay item {index} for thread {thread_id}: {err}"
                ),
            })?;
        let current_item =
            serde_json::to_value(current_item).map_err(|err| ThreadStoreError::Internal {
                message: format!(
                    "failed to compare canonical replay item {index} for thread {thread_id}: {err}"
                ),
            })?;
        if supplied_item != current_item {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Reject a valid but superseded rollout selected before a revert. A missing, malformed, or
/// cross-thread SQLite path is stale index data and remains repairable through the explicit path.
async fn ensure_requested_rollout_is_current(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    requested_path: &std::path::Path,
) -> ThreadStoreResult<()> {
    let Some(state_db) = store.state_db().await else {
        return Ok(());
    };
    let Some(metadata) =
        state_db
            .get_thread(thread_id)
            .await
            .map_err(|err| ThreadStoreError::Internal {
                message: format!("failed to read selected rollout for thread {thread_id}: {err}"),
            })?
    else {
        return Ok(());
    };
    let Some(selected_path) =
        codex_rollout::existing_rollout_path(metadata.rollout_path.as_path()).await
    else {
        return Ok(());
    };
    let Ok(selected_meta) = codex_rollout::read_session_meta_line(selected_path.as_path()).await
    else {
        return Ok(());
    };
    if selected_meta.meta.id != thread_id {
        return Ok(());
    }
    let Some(requested_path) = codex_rollout::existing_rollout_path(requested_path).await else {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "requested rollout for thread {thread_id} disappeared before resume; reload before resuming"
            ),
        });
    };
    let selected_display = selected_path.display().to_string();
    let requested_display = requested_path.display().to_string();
    let same_file = tokio::task::spawn_blocking(move || {
        same_file::is_same_file(selected_path.as_path(), requested_path.as_path())
    })
    .await
    .map_err(|err| ThreadStoreError::Internal {
        message: format!("failed to compare selected rollout for thread {thread_id}: {err}"),
    })?
    .map_err(|err| ThreadStoreError::Conflict {
        message: format!(
            "selected rollout for thread {thread_id} changed while resuming ({selected_display} versus {requested_display}): {err}; reload before resuming"
        ),
    })?;
    if !same_file {
        return Err(ThreadStoreError::Conflict {
            message: format!(
                "requested rollout for thread {thread_id} is no longer selected ({requested_display}); current rollout is {selected_display}; reload before resuming"
            ),
        });
    }
    Ok(())
}

#[tracing::instrument(
    level = "trace",
    skip_all,
    fields(item_count = params.items.len())
)]
pub(super) async fn append_items(
    store: &LocalThreadStore,
    params: AppendThreadItemsParams,
) -> ThreadStoreResult<()> {
    write_and_project(
        store,
        params.thread_id,
        RolloutWriteOp::AppendItems(params.items),
    )
    .await
}

pub(super) async fn persist_thread(
    store: &LocalThreadStore,
    thread_id: ThreadId,
) -> ThreadStoreResult<()> {
    write_and_project(store, thread_id, RolloutWriteOp::Persist).await
}

pub(super) async fn flush_thread(
    store: &LocalThreadStore,
    thread_id: ThreadId,
) -> ThreadStoreResult<()> {
    write_and_project(store, thread_id, RolloutWriteOp::Flush).await
}

pub(super) async fn shutdown_thread(
    store: &LocalThreadStore,
    thread_id: ThreadId,
) -> ThreadStoreResult<()> {
    let mut pending_metadata = store.pending_thread_metadata.lock(thread_id).await;
    let _live_writer_guard = store.live_writer_locks.lock(thread_id).await;
    let (recorder, rollout_id, history_mode) = live_writer_parts(store, thread_id).await?;
    let rollout_path = recorder.rollout_path().to_path_buf();
    if history_mode == ThreadHistoryMode::PaginatedRefsV1 {
        super::rollout_migration::ensure_migrated_generation_is_writable(
            &store.config.codex_home,
            thread_id,
            rollout_path.as_path(),
        )
        .await?;
    }
    recorder.shutdown().await.map_err(thread_store_io_error)?;
    let generation_check = if history_mode == ThreadHistoryMode::PaginatedRefsV1 {
        super::rollout_migration::ensure_migrated_generation_is_writable(
            &store.config.codex_home,
            thread_id,
            rollout_path.as_path(),
        )
        .await
    } else {
        Ok(())
    };
    if !matches!(history_mode, ThreadHistoryMode::Legacy)
        && generation_check.is_ok()
        && let Err(err) = super::thread_history_materialization::materialize_to_sqlite(
            store,
            rollout_id,
            rollout_path.as_path(),
        )
        .await
    {
        warn!("failed to project durable rollout during shutdown for {thread_id}: {err}");
    }
    sync_materialized_rollout_path(store, thread_id, rollout_path.as_path()).await?;
    if let Some(metrics) = codex_otel::global()
        && let Ok(metadata) = tokio::fs::metadata(&rollout_path).await
    {
        let size_bytes = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        let _ = metrics.histogram(ROLLOUT_SIZE_BYTES_METRIC, size_bytes, &[]);
    }
    let rollout_exists = match tokio::fs::try_exists(&rollout_path).await {
        Ok(rollout_exists) => rollout_exists,
        Err(err) => {
            warn!(
                "failed to check rollout path after shutdown for {thread_id}; preserving pending metadata: {err}"
            );
            true
        }
    };
    store.live_recorders.lock().await.remove(&thread_id);
    drop(_live_writer_guard);
    if !rollout_exists && pending_metadata.take().is_some() {
        store.pending_thread_metadata.remove(thread_id).await;
    }
    generation_check
}

pub(super) async fn discard_thread(
    store: &LocalThreadStore,
    thread_id: ThreadId,
) -> ThreadStoreResult<()> {
    let mut pending_metadata = store.pending_thread_metadata.lock(thread_id).await;
    let _live_writer_guard = store.live_writer_locks.lock(thread_id).await;
    if pending_metadata.take().is_some() {
        store.pending_thread_metadata.remove(thread_id).await;
    }
    store
        .live_recorders
        .lock()
        .await
        .remove(&thread_id)
        .map(|_| ())
        .ok_or(ThreadStoreError::ThreadNotFound { thread_id })
}

pub(super) async fn rollout_path(
    store: &LocalThreadStore,
    thread_id: ThreadId,
) -> ThreadStoreResult<PathBuf> {
    Ok(store
        .live_recorders
        .lock()
        .await
        .get(&thread_id)
        .ok_or(ThreadStoreError::ThreadNotFound { thread_id })?
        .recorder
        .rollout_path()
        .to_path_buf())
}

async fn sync_materialized_rollout_path(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    rollout_path: &std::path::Path,
) -> ThreadStoreResult<()> {
    if codex_rollout::existing_rollout_path(rollout_path)
        .await
        .is_none()
    {
        return Ok(());
    }
    let Some(state_db) = store.state_db().await else {
        return Ok(());
    };
    let result: ThreadStoreResult<()> = async {
        let Some(mut metadata) =
            state_db
                .get_thread(thread_id)
                .await
                .map_err(|err| ThreadStoreError::Internal {
                    message: format!("failed to read thread metadata for {thread_id}: {err}"),
                })?
        else {
            return Ok(());
        };
        if metadata.rollout_path != rollout_path {
            metadata.rollout_path = rollout_path.to_path_buf();
            state_db
                .upsert_thread(&metadata)
                .await
                .map_err(|err| ThreadStoreError::Internal {
                    message: format!("failed to update thread metadata for {thread_id}: {err}"),
                })?;
        }
        Ok(())
    }
    .await;
    if let Err(err) = result {
        warn!("failed to sync materialized rollout path for thread {thread_id}: {err}");
    }
    Ok(())
}

fn thread_store_io_error(err: std::io::Error) -> ThreadStoreError {
    ThreadStoreError::Internal {
        message: err.to_string(),
    }
}

/// The rollout writer has three distinct lifecycle moments:
/// - `AppendItems` is normal turn/event persistence and adds new rollout records.
/// - `Persist` makes the thread durable before any turn items exist; locally this can write the
///   initial `SessionMeta`.
/// - `Flush` writes any rollout records already queued in the recorder and ensures they are
///   durably persisted.
///
/// Each can advance the rollout JSONL file on disk, so we need to make sure we materialize the
/// new data into the SQLite history tables (turns and items) as necessary.
enum RolloutWriteOp {
    AppendItems(Vec<RolloutItem>),
    Persist,
    Flush,
}

pub(super) async fn live_writer_parts(
    store: &LocalThreadStore,
    thread_id: ThreadId,
) -> ThreadStoreResult<(RolloutRecorder, ThreadId, ThreadHistoryMode)> {
    let live_recorders = store.live_recorders.lock().await;
    let entry = live_recorders
        .get(&thread_id)
        .ok_or(ThreadStoreError::ThreadNotFound { thread_id })?;
    Ok((entry.recorder.clone(), entry.rollout_id, entry.history_mode))
}

async fn write_and_project(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    write_op: RolloutWriteOp,
) -> ThreadStoreResult<()> {
    // Every live write should have a recorder: create/resume installs one, while
    // shutdown/discard/delete removes it. Keep the lookup defensive so late writes fail after
    // teardown.
    let _live_writer_guard = store.live_writer_locks.lock(thread_id).await;
    let (recorder, rollout_id, history_mode) = live_writer_parts(store, thread_id).await?;
    let sync_rollout_path = matches!(&write_op, RolloutWriteOp::Persist | RolloutWriteOp::Flush);
    let write_op = match write_op {
        RolloutWriteOp::AppendItems(mut items) => {
            items.retain(|item| is_persisted_rollout_item(item, history_mode));
            if items.is_empty() {
                return Ok(());
            }
            RolloutWriteOp::AppendItems(items)
        }
        RolloutWriteOp::Persist => RolloutWriteOp::Persist,
        RolloutWriteOp::Flush => RolloutWriteOp::Flush,
    };
    if history_mode == ThreadHistoryMode::PaginatedRefsV1 {
        super::rollout_migration::ensure_migrated_generation_is_writable(
            &store.config.codex_home,
            thread_id,
            recorder.rollout_path(),
        )
        .await?;
    }
    if matches!(history_mode, ThreadHistoryMode::Legacy) {
        durable_write(&recorder, write_op).await?;
    } else {
        let rollout_path = recorder.rollout_path();
        // SQLite is a rebuildable view. The flush barrier must win before projection starts so it
        // can lag JSONL after failure, but can never get ahead of canonical history.
        durable_write(&recorder, write_op).await?;
        if history_mode == ThreadHistoryMode::PaginatedRefsV1 {
            super::rollout_migration::ensure_migrated_generation_is_writable(
                &store.config.codex_home,
                thread_id,
                rollout_path,
            )
            .await?;
        }
        if let Err(err) = super::thread_history_materialization::materialize_to_sqlite(
            store,
            rollout_id,
            rollout_path,
        )
        .await
        {
            warn!("failed to project durable rollout for {thread_id}: {err}");
        }
    }
    if sync_rollout_path {
        sync_materialized_rollout_path(store, thread_id, recorder.rollout_path()).await?;
    }
    Ok(())
}

async fn durable_write(recorder: &RolloutRecorder, write: RolloutWriteOp) -> ThreadStoreResult<()> {
    match write {
        RolloutWriteOp::AppendItems(items) => {
            recorder
                .record_canonical_items(items.as_slice())
                .await
                .map_err(thread_store_io_error)?;
            recorder.flush().await.map_err(thread_store_io_error)
        }
        RolloutWriteOp::Persist => recorder.persist().await.map_err(thread_store_io_error),
        RolloutWriteOp::Flush => recorder.flush().await.map_err(thread_store_io_error),
    }
}
