//! Migration-only stable IDs for historical response items that predate durable ID assignment.
//!
//! An ID identifies one ordered occurrence, not merely equal content. The transient fingerprint
//! below is only a lookup accelerator; full envelope equality and left-to-right consumption decide
//! whether a checkpoint copy came from an older occurrence.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::io;
use std::io::Write;

use codex_protocol::ResponseItemId;
use codex_protocol::ThreadId;
use codex_rollout::CompactedHistoryResolver;
use codex_rollout::ResponseItemEnvelope;
use codex_rollout::RolloutItem;
use uuid::Uuid;

use super::migration_error;
use crate::ThreadStoreResult;

// Frozen once emitted: changing this namespace would make retry output depend on the client build.
const MIGRATION_OCCURRENCE_NAMESPACE: Uuid =
    Uuid::from_u128(0x91a3_98f2_7c42_4e54_a8a4_2bc7_1a20_b6d1);

#[derive(Clone)]
struct CandidateOccurrence {
    id: ResponseItemId,
    shape_without_id: ResponseItemEnvelope,
    fingerprint: u64,
}

/// Deferred candidate-state update for one checkpoint.
pub(super) struct OccurrencePreparation {
    next_candidates: Option<Vec<CandidateOccurrence>>,
    clear_on_failure: bool,
}

impl OccurrencePreparation {
    fn unchanged() -> Self {
        Self {
            next_candidates: None,
            clear_on_failure: false,
        }
    }

    fn rebase(next_candidates: Vec<CandidateOccurrence>) -> Self {
        Self {
            next_candidates: Some(next_candidates),
            clear_on_failure: true,
        }
    }
}

/// Assigns deterministic occurrence IDs while one legacy rollout is canonicalized.
pub(super) struct MigrationOccurrenceIds {
    rollout_id: ThreadId,
    reserved_ids: HashSet<String>,
    generated_ids: HashSet<String>,
    active_candidates: Vec<CandidateOccurrence>,
}

impl MigrationOccurrenceIds {
    pub(super) fn new(rollout_id: ThreadId, reserved_ids: HashSet<String>) -> Self {
        Self {
            rollout_id,
            reserved_ids,
            generated_ids: HashSet::new(),
            active_candidates: Vec::new(),
        }
    }

    /// Adds stable IDs before the ordinary backward-reference encoder sees an item.
    pub(super) fn prepare_item(
        &mut self,
        item: &mut RolloutItem,
        resolver: &CompactedHistoryResolver,
        output_ordinal: u64,
    ) -> ThreadStoreResult<OccurrencePreparation> {
        match item {
            RolloutItem::ResponseItem(envelope) => {
                normalize_empty_id(&mut envelope.item);
                if envelope.item.id().is_none() && envelope.item.id_prefix().is_some() {
                    let id = self.generate_id(
                        &envelope.item,
                        format_args!("ordinal:{output_ordinal}:top"),
                    )?;
                    envelope.item.set_id(Some(id));
                }
                if let Some(id) = envelope.item.id() {
                    self.active_candidates
                        .push(candidate(envelope, id.clone())?);
                }
                Ok(OccurrencePreparation::unchanged())
            }
            RolloutItem::Compacted(compacted)
                if compacted.replacement_history.is_some()
                    || compacted.replacement_history_entries.is_some() =>
            {
                let Some(mut resolved) = (match resolver.resolve_compacted_item(compacted) {
                    Ok(resolved) => resolved,
                    Err(_) => {
                        // The ordinary migration policy decides whether this checkpoint is fatal
                        // or superseded. Its active history is unknowable either way.
                        self.active_candidates.clear();
                        return Ok(OccurrencePreparation::unchanged());
                    }
                }) else {
                    return Ok(OccurrencePreparation::unchanged());
                };

                let mut candidate_indices = HashMap::<u64, Vec<usize>>::new();
                for (index, candidate) in self.active_candidates.iter().enumerate() {
                    candidate_indices
                        .entry(candidate.fingerprint)
                        .or_default()
                        .push(index);
                }
                let mut consumed = vec![false; self.active_candidates.len()];

                for (entry_index, envelope) in resolved.iter_mut().enumerate() {
                    normalize_empty_id(&mut envelope.item);
                    if envelope.item.id().is_some() || envelope.item.id_prefix().is_none() {
                        continue;
                    }

                    let target_fingerprint = envelope_fingerprint(envelope)?;
                    let matched = candidate_indices
                        .get(&target_fingerprint)
                        .and_then(|indices| {
                            indices.iter().copied().find(|index| {
                                !consumed[*index]
                                    && self.active_candidates[*index].shape_without_id == *envelope
                            })
                        });
                    let id = if let Some(index) = matched {
                        consumed[index] = true;
                        self.active_candidates[index].id.clone()
                    } else {
                        self.generate_id(
                            &envelope.item,
                            format_args!("ordinal:{output_ordinal}:checkpoint-entry:{entry_index}"),
                        )?
                    };
                    envelope.item.set_id(Some(id));
                }

                let next_candidates = resolved
                    .iter()
                    .filter_map(|envelope| {
                        let id = envelope.item.id()?;
                        Some(candidate(envelope, id.clone()))
                    })
                    .collect::<ThreadStoreResult<Vec<_>>>()?;

                // Give the normal encoder a complete assigned history. It will retain a newly
                // assigned carrier inline the first time and reference it from later checkpoints.
                compacted.replacement_history = Some(resolved);
                compacted.replacement_history_entries = None;
                Ok(OccurrencePreparation::rebase(next_candidates))
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::Compacted(_)
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_) => Ok(OccurrencePreparation::unchanged()),
        }
    }

    /// Commits a checkpoint's active occurrence view only after reference encoding succeeds.
    pub(super) fn finish_item(&mut self, preparation: OccurrencePreparation, success: bool) {
        if success {
            if let Some(next_candidates) = preparation.next_candidates {
                self.active_candidates = next_candidates;
            }
        } else if preparation.clear_on_failure {
            self.active_candidates.clear();
        }
    }

    fn generate_id(
        &mut self,
        item: &codex_protocol::models::ResponseItem,
        coordinate: impl std::fmt::Display,
    ) -> ThreadStoreResult<ResponseItemId> {
        let prefix = item
            .id_prefix()
            .ok_or_else(|| migration_error("response item cannot carry a migration ID"))?;
        let name = format!("{}:{coordinate}", self.rollout_id);
        let suffix = Uuid::new_v5(&MIGRATION_OCCURRENCE_NAMESPACE, name.as_bytes());
        let id = ResponseItemId::with_suffix(prefix, suffix);
        if self.reserved_ids.contains(id.as_str()) || !self.generated_ids.insert(id.to_string()) {
            return Err(migration_error(format!(
                "deterministic migration response item ID collided with an existing occurrence: {id}"
            )));
        }
        Ok(id)
    }
}

fn normalize_empty_id(item: &mut codex_protocol::models::ResponseItem) {
    if item.id().is_some_and(|id| id.as_str().is_empty()) {
        item.set_id(None);
    }
}

fn candidate(
    envelope: &ResponseItemEnvelope,
    id: ResponseItemId,
) -> ThreadStoreResult<CandidateOccurrence> {
    let mut shape_without_id = envelope.clone();
    shape_without_id.item.set_id(None);
    let fingerprint = envelope_fingerprint(&shape_without_id)?;
    Ok(CandidateOccurrence {
        id,
        shape_without_id,
        fingerprint,
    })
}

fn envelope_fingerprint(envelope: &ResponseItemEnvelope) -> ThreadStoreResult<u64> {
    let mut writer = HashWriter(DefaultHasher::new());
    serde_json::to_writer(&mut writer, &(&envelope.item, &envelope.metadata))
        .map_err(migration_error)?;
    Ok(writer.0.finish())
}

struct HashWriter(DefaultHasher);

impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseItem;
    use codex_rollout::CompactedHistoryResolver;
    use codex_rollout::RolloutItem;

    use super::*;

    #[test]
    fn reserved_deterministic_id_collision_fails_closed() {
        let rollout_id = ThreadId::from_u128(7);
        let suffix = Uuid::new_v5(
            &MIGRATION_OCCURRENCE_NAMESPACE,
            format!("{rollout_id}:ordinal:0:top").as_bytes(),
        );
        let reserved_id = ResponseItemId::with_suffix("msg", suffix);
        let mut occurrence_ids =
            MigrationOccurrenceIds::new(rollout_id, HashSet::from([reserved_id.to_string()]));
        let mut item = RolloutItem::ResponseItem(
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "collision".to_string(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }
            .into(),
        );

        let error = occurrence_ids
            .prepare_item(&mut item, &CompactedHistoryResolver::default(), 0)
            .err()
            .expect("reserved synthetic ID must block migration");

        assert!(error.to_string().contains(reserved_id.as_str()));
    }
}
