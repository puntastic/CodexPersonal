//! Reconstructs model context and preserves source runtime metadata across fork cutoffs.

use std::io;
use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HistoryPosition;
use codex_protocol::protocol::SessionMetaLine;
use codex_rollout::ModelContextScan;
use codex_rollout::ModelContextScanProgress;
use codex_rollout::ReverseJsonlScanner;
use codex_rollout::RolloutItem;
use codex_rollout::ScanOutcome;

use super::LocalThreadStore;
use super::read_thread;
use super::rollout_lineage::RolloutLineage;
use super::thread_rollout_resolver;
use crate::LoadThreadHistoryParams;
use crate::StoredModelContext;
use crate::ThreadStoreError;
use crate::ThreadStoreResult;

#[cfg(test)]
#[path = "model_context_tests.rs"]
mod tests;

/// Loads rollout items needed to reconstruct the latest model-visible context.
///
/// Paginated JSONL rollouts use a reverse scan. It stops at the newest `CompactedItem` with both
/// replacement history and a window number once all referenced sources are available. Historical
/// reference checkpoints without resume metadata also retain one completed-turn metadata baseline;
/// new-contract checkpoints use older records only for demanded sources. If no bounded cutoff is
/// possible, reread the complete lineage because the bounded accumulator drops unrelated payloads.
///
/// Compressed segments are decoded before applying their original JSONL offsets. Legacy rollouts
/// keep the existing full-history path.
pub(super) async fn load_latest_model_context(
    store: &LocalThreadStore,
    params: LoadThreadHistoryParams,
) -> ThreadStoreResult<StoredModelContext> {
    let resolved = if params.include_archived {
        thread_rollout_resolver::resolve_current_including_archived(store, params.thread_id).await?
    } else {
        thread_rollout_resolver::resolve_current(store, params.thread_id).await?
    };
    let resolved = resolved.ok_or_else(|| ThreadStoreError::InvalidRequest {
        message: format!("no rollout found for thread id {}", params.thread_id),
    })?;

    load_latest_model_context_from_rollout(
        store,
        params.thread_id,
        resolved.rollout_id,
        resolved.path,
    )
    .await
}

pub(super) async fn load_latest_model_context_from_rollout(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    rollout_id: ThreadId,
    path: PathBuf,
) -> ThreadStoreResult<StoredModelContext> {
    let session_meta = codex_rollout::read_session_meta_line(path.as_path())
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!("failed to read session metadata {}: {err}", path.display()),
        })?;
    if session_meta.meta.id != thread_id {
        return Err(ThreadStoreError::InvalidRequest {
            message: format!(
                "rollout at {} belongs to thread {}, not {}",
                path.display(),
                session_meta.meta.id,
                thread_id
            ),
        });
    }

    let items = if session_meta.meta.history_mode.is_paginated() {
        let lineage = store
            .resolve_rollout_lineage_from_rollout(thread_id, rollout_id, path.clone())
            .await?;
        scan_model_context_from_lineage(lineage, session_meta).await?
    } else {
        read_thread::load_history_items(path.as_path()).await?
    };

    Ok(StoredModelContext { thread_id, items })
}

/// Loads startup context from a fork's frozen inherited prefix.
pub(super) async fn load_for_fork(
    lineage: RolloutLineage,
    history_base: Option<HistoryPosition>,
) -> ThreadStoreResult<Vec<RolloutItem>> {
    let source_path = lineage
        .segments()
        .last()
        .map(|segment| segment.rollout_path.as_path())
        .ok_or_else(|| ThreadStoreError::Internal {
            message: "fork lineage has no source segment".to_string(),
        })?;
    let mut session_meta = codex_rollout::read_session_meta_line(source_path)
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!(
                "failed to read session metadata {}: {err}",
                source_path.display()
            ),
        })?;
    if session_meta.meta.multi_agent_version.is_none() {
        // Recover only the runtime version before applying the fork cutoff. Stop at the
        // newest version-bearing context instead of retaining the source's full replay.
        let source_lineage = lineage.clone();
        session_meta.meta.multi_agent_version = tokio::task::spawn_blocking(move || {
            for segment in source_lineage.segments().iter().rev() {
                let file =
                    codex_rollout::open_rollout_seekable_reader(segment.rollout_path.as_path())?;
                let mut scanner = match segment.end.map(|end| end.end_byte_offset) {
                    Some(end_byte_offset) => ReverseJsonlScanner::new_at(file, end_byte_offset)?,
                    None => ReverseJsonlScanner::new(file)?,
                };
                while let Some(outcome) = scanner.scan_next_rollout_line()? {
                    let ScanOutcome::Parsed(line) = outcome else {
                        continue;
                    };
                    if let Some(version) = codex_rollout::resume_multi_agent_version(&line.item) {
                        return Ok(Some(version));
                    }
                    // Ancestor metadata does not describe the immediate source's runtime.
                    if matches!(line.item, RolloutItem::SessionMeta(_)) {
                        break;
                    }
                }
            }
            Ok::<_, io::Error>(None)
        })
        .await
        .map_err(|err| ThreadStoreError::Internal {
            message: format!("failed to join fork runtime version scan: {err}"),
        })?
        .map_err(|err| ThreadStoreError::Internal {
            message: format!("failed to read fork runtime version: {err}"),
        })?;
    }
    match history_base {
        Some(history_base) => {
            let lineage = lineage.truncate_at(history_base).await?;
            scan_model_context_from_lineage(lineage, session_meta).await
        }
        None => Ok(vec![RolloutItem::SessionMeta(session_meta)]),
    }
}

async fn scan_model_context_from_lineage(
    lineage: RolloutLineage,
    session_meta: SessionMetaLine,
) -> ThreadStoreResult<Vec<RolloutItem>> {
    let scan = tokio::task::spawn_blocking(move || {
        scan_model_context_from_lineage_blocking(&lineage, session_meta)
    })
    .await
    .map_err(|err| ThreadStoreError::Internal {
        message: format!("failed to join model context scan: {err}"),
    })?;
    match scan {
        Ok(items) => Ok(items),
        Err(err) => Err(ThreadStoreError::Internal {
            message: format!("failed to scan paginated model context lineage: {err}"),
        }),
    }
}

fn scan_model_context_from_lineage_blocking(
    lineage: &RolloutLineage,
    session_meta: SessionMetaLine,
) -> io::Result<Vec<RolloutItem>> {
    // The canonical rollout owns the exact generation. SQLite exposes only the public family
    // and cannot distinguish historical reference checkpoints from native paginated history.
    let mut scan = ModelContextScan::for_history_mode(session_meta.meta.history_mode);
    let mut bounded = false;
    'segments: for segment in lineage.segments().iter().rev() {
        let file = codex_rollout::open_rollout_seekable_reader(segment.rollout_path.as_path())?;
        let mut scanner = match segment.end.map(|end| end.end_byte_offset) {
            Some(end_byte_offset) => ReverseJsonlScanner::new_at(file, end_byte_offset)?,
            None => ReverseJsonlScanner::new(file)?,
        };
        while let Some(outcome) = scanner.scan_next_rollout_line()? {
            let ScanOutcome::Parsed(line) = outcome else {
                continue;
            };
            // Each rollout segment contributes only its local delta. Its session metadata is
            // replaced with the requested thread's canonical SessionMeta after replay.
            if matches!(&line.item, RolloutItem::SessionMeta(_)) {
                break;
            }
            match scan.push(line.item) {
                ModelContextScanProgress::Continue => {}
                ModelContextScanProgress::Complete => {
                    bounded = true;
                    break 'segments;
                }
            }
        }
    }

    if !bounded {
        // The bounded accumulator intentionally drops unrelated pre-base payloads. Unsupported
        // legacy shapes, rollback, and unresolved references retain their exact previous behavior
        // by rereading the lineage into the full canonical replay only on this fallback path.
        drop(scan);
        return load_full_model_context_from_lineage_blocking(lineage, session_meta);
    }

    let canonical_meta = session_meta.clone();
    let mut items = scan.finish(session_meta);
    if !matches!(items.first(), Some(RolloutItem::SessionMeta(_))) {
        items.insert(0, RolloutItem::SessionMeta(canonical_meta));
    }
    Ok(items)
}

fn load_full_model_context_from_lineage_blocking(
    lineage: &RolloutLineage,
    session_meta: SessionMetaLine,
) -> io::Result<Vec<RolloutItem>> {
    let mut items_newest_first = Vec::new();
    for segment in lineage.segments().iter().rev() {
        let file = codex_rollout::open_rollout_seekable_reader(segment.rollout_path.as_path())?;
        let mut scanner = match segment.end.map(|end| end.end_byte_offset) {
            Some(end_byte_offset) => ReverseJsonlScanner::new_at(file, end_byte_offset)?,
            None => ReverseJsonlScanner::new(file)?,
        };
        while let Some(outcome) = scanner.scan_next_rollout_line()? {
            let ScanOutcome::Parsed(line) = outcome else {
                continue;
            };
            if matches!(&line.item, RolloutItem::SessionMeta(_)) {
                break;
            }
            items_newest_first.push(line.item);
        }
    }

    items_newest_first.reverse();
    items_newest_first.insert(0, RolloutItem::SessionMeta(session_meta));
    Ok(items_newest_first)
}
