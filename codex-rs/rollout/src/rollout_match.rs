use std::io;
use std::path::Path;

use codex_history::RolloutItem;
use serde_json::Value;
use tracing::trace;
use tracing::warn;

/// Compares a caller-owned replay with the canonical persisted rollout without collecting a
/// second copy of the rollout in memory.
///
/// This follows [`crate::RolloutRecorder::load_rollout_items`] semantics: blank records, malformed
/// records, and retired legacy ghost snapshots do not participate in the replay. Memory remains
/// bounded to one decoded record in addition to the caller's replay.
pub async fn rollout_items_match_file(path: &Path, expected: &[RolloutItem]) -> io::Result<bool> {
    let mut reader = crate::open_rollout_line_reader(path).await?;
    let mut saw_non_empty_line = false;
    let mut saw_session_meta = false;
    let mut index = 0_usize;

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        saw_non_empty_line = true;
        let mut value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                warn!("failed to parse line as JSON while comparing rollout: {error}");
                continue;
            }
        };
        if crate::strip_legacy_ghost_snapshot_rollout_line(&mut value) {
            trace!("skipping legacy ghost_snapshot rollout line while comparing rollout");
            continue;
        }
        if !saw_session_meta {
            crate::recorder::reject_unknown_thread_history_mode(&value)?;
        }
        let rollout_line = match crate::decode_rollout_line(value) {
            Ok(rollout_line) => rollout_line,
            Err(error) => {
                trace!("failed to parse rollout line while comparing replay: {error}");
                continue;
            }
        };
        if !saw_session_meta && matches!(&rollout_line.item, RolloutItem::SessionMeta(_)) {
            saw_session_meta = true;
        }
        let Some(expected_item) = expected.get(index) else {
            return Ok(false);
        };
        let actual_value = serde_json::to_value(&rollout_line.item).map_err(io::Error::other)?;
        let expected_value = serde_json::to_value(expected_item).map_err(io::Error::other)?;
        if actual_value != expected_value {
            return Ok(false);
        }
        index = index.saturating_add(1);
    }

    if !saw_non_empty_line {
        return Err(io::Error::other("empty session file"));
    }
    Ok(index == expected.len())
}
