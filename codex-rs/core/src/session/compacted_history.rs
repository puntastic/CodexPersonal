use std::collections::HashMap;
use std::collections::HashSet;

use codex_history::CompactedHistoryEntry;
use codex_history::GuardianHistoryCheckpoint;
use codex_history::ResponseItemEnvelope;
use codex_history::RolloutItem;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ThreadHistoryMode;

/// Storage representation for one complete compacted history.
///
/// Legacy inline history remains the compatibility form for ungated modes and when no item can be
/// referenced. In the reference-gated mode, once at least one exact persisted item can be reused,
/// the entry form carries the complete ordering and inlines only items without an identical
/// durable source.
pub(super) struct PersistedReplacementHistory {
    pub(super) legacy: Option<Vec<ResponseItemEnvelope>>,
    pub(super) entries: Option<Vec<CompactedHistoryEntry>>,
}

pub(super) fn encode_replacement_history(
    items: &[ResponseItemEnvelope],
    persisted_items: &HashMap<String, ResponseItemEnvelope>,
    history_mode: ThreadHistoryMode,
) -> PersistedReplacementHistory {
    if !history_mode.supports_compacted_history_references() {
        return PersistedReplacementHistory {
            legacy: Some(items.to_vec()),
            entries: None,
        };
    }

    // A stable ID is a source address, not an occurrence address. If the active history contains
    // that ID more than once, a later checkpoint cannot recover which occurrence an older
    // Reference meant from a last-write-wins source index. Keep every occurrence inline instead
    // of silently changing the ordered history on cold resume.
    let conflicting_item_ids = conflicting_history_item_ids(items);
    let mut has_reference = false;
    let entries = items
        .iter()
        .cloned()
        .map(|envelope| {
            let item_id = envelope
                .item
                .id()
                .map(codex_protocol::ResponseItemId::as_str);
            if let Some(item_id) = item_id
                && !conflicting_item_ids.contains(item_id)
                && persisted_items.get(item_id) == Some(&envelope)
            {
                if history_mode.supports_compacted_history_integrity() {
                    match CompactedHistoryEntry::reference_v2(item_id.to_string(), &envelope) {
                        Ok(reference) => {
                            has_reference = true;
                            reference
                        }
                        // A source whose durable projection cannot be encoded stays self-contained;
                        // never downgrade a V2 write to an ID-only V1 reference.
                        Err(_) => CompactedHistoryEntry::from(envelope),
                    }
                } else {
                    has_reference = true;
                    CompactedHistoryEntry::Reference {
                        item_id: item_id.to_string(),
                    }
                }
            } else {
                CompactedHistoryEntry::from(envelope)
            }
        })
        .collect::<Vec<_>>();

    if has_reference {
        PersistedReplacementHistory {
            legacy: None,
            entries: Some(entries),
        }
    } else {
        PersistedReplacementHistory {
            legacy: Some(items.to_vec()),
            entries: None,
        }
    }
}

/// Encodes a Guardian checkpoint against exact durable response-item sources.
///
/// Guardian history intentionally carries response items rather than harness metadata. V2
/// references nevertheless bind the complete persisted source envelope, so a later metadata-only
/// substitution still fails closed before its response item can be restored as review evidence.
pub(super) fn encode_guardian_history(
    checkpoint: GuardianHistoryCheckpoint,
    persisted_items: &HashMap<String, ResponseItemEnvelope>,
    history_mode: ThreadHistoryMode,
) -> GuardianHistoryCheckpoint {
    if !history_mode.supports_compacted_history_references() || checkpoint.is_reference_backed() {
        return checkpoint;
    }

    let conflicting_item_ids = conflicting_response_item_ids(&checkpoint.0);
    let mut has_reference = false;
    let entries = checkpoint
        .0
        .iter()
        .cloned()
        .map(|item| {
            let exact_source = item.id().and_then(|item_id| {
                if conflicting_item_ids.contains(item_id.as_str()) {
                    None
                } else {
                    persisted_items
                        .get(item_id.as_str())
                        .filter(|source| source.item == item)
                        .map(|source| (item_id.as_str().to_string(), source))
                }
            });

            if let Some((item_id, source)) = exact_source {
                if history_mode.supports_compacted_history_integrity() {
                    match CompactedHistoryEntry::reference_v2(item_id, source) {
                        Ok(reference) => {
                            has_reference = true;
                            reference
                        }
                        Err(_) => CompactedHistoryEntry::from(ResponseItemEnvelope::new(item)),
                    }
                } else {
                    has_reference = true;
                    CompactedHistoryEntry::Reference { item_id }
                }
            } else {
                CompactedHistoryEntry::from(ResponseItemEnvelope::new(item))
            }
        })
        .collect();

    if has_reference {
        GuardianHistoryCheckpoint::from_entries(entries)
    } else {
        checkpoint
    }
}

fn conflicting_history_item_ids(items: &[ResponseItemEnvelope]) -> HashSet<String> {
    let mut first_by_id = HashMap::new();
    let mut conflicting = HashSet::new();
    for envelope in items {
        if let Some(item_id) = envelope
            .item
            .id()
            .map(codex_protocol::ResponseItemId::as_str)
        {
            match first_by_id.entry(item_id) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(envelope);
                }
                std::collections::hash_map::Entry::Occupied(entry) if entry.get() != &envelope => {
                    conflicting.insert(item_id.to_string());
                }
                std::collections::hash_map::Entry::Occupied(_) => {}
            }
        }
    }
    conflicting
}

fn conflicting_response_item_ids(items: &[ResponseItem]) -> HashSet<String> {
    let mut first_by_id = HashMap::new();
    let mut conflicting = HashSet::new();
    for item in items {
        if let Some(item_id) = item.id().map(codex_protocol::ResponseItemId::as_str) {
            match first_by_id.entry(item_id) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(item);
                }
                std::collections::hash_map::Entry::Occupied(entry) if entry.get() != &item => {
                    conflicting.insert(item_id.to_string());
                }
                std::collections::hash_map::Entry::Occupied(_) => {}
            }
        }
    }
    conflicting
}

pub(super) fn retained_checkpoint_reference_item_ids(
    items: &[RolloutItem],
    keep_start_index: usize,
    selected_checkpoint_index: Option<usize>,
) -> HashSet<String> {
    items
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            *index >= keep_start_index
                && selected_checkpoint_index.is_none_or(|base| *index >= base)
        })
        .filter_map(|(_, item)| match item {
            RolloutItem::Compacted(compacted) => Some(
                compacted
                    .replacement_history_entries
                    .as_deref()
                    .into_iter()
                    .flatten()
                    .chain(
                        compacted
                            .guardian_history
                            .as_ref()
                            .and_then(GuardianHistoryCheckpoint::entries)
                            .into_iter()
                            .flatten(),
                    ),
            ),
            _ => None,
        })
        .flatten()
        .filter_map(|entry| match entry {
            CompactedHistoryEntry::Reference { item_id }
            | CompactedHistoryEntry::ReferenceV2 { item_id, .. } => Some(item_id.clone()),
            CompactedHistoryEntry::Inline { .. } => None,
        })
        .collect()
}

/// Rewrites a copied fork so every compacted-history reference has an exact source in the child
/// writer's own lineage.
///
/// Superseded checkpoints are source carriers, not replay bases, and are omitted. A retained
/// checkpoint is resolved from the parent stream, then either kept inline for an ungated writer or
/// immediately re-encoded against only those earlier child items that a gated writer persists.
/// This makes a writer-filtered top-level item Inline at the next checkpoint instead of leaving a
/// dangling reference, while keeping peak expansion bounded to one checkpoint.
pub(super) fn normalize_copied_fork_rollout(
    items: Vec<RolloutItem>,
    history_mode: ThreadHistoryMode,
) -> CodexResult<Vec<RolloutItem>> {
    let selected_checkpoint_index =
        super::rollout_reconstruction::selected_surviving_complete_checkpoint_index(&items);
    let requested_source_item_ids =
        retained_checkpoint_reference_item_ids(&items, 0, selected_checkpoint_index);
    let mut source_resolver = codex_history::CompactedHistoryResolver::default();
    let mut destination_resolver = codex_history::CompactedHistoryResolver::default();
    let mut normalized = Vec::with_capacity(items.len());

    for (index, mut item) in items.into_iter().enumerate() {
        let keep_checkpoint = selected_checkpoint_index.is_none_or(|base| index >= base);
        let (resolved_parent_history, resolved_parent_guardian) =
            if keep_checkpoint && let RolloutItem::Compacted(compacted) = &item {
                (
                    source_resolver
                        .resolve_compacted_item(compacted)
                        .map_err(copied_fork_reference_error)?,
                    source_resolver
                        .resolve_guardian_history(compacted)
                        .map_err(copied_fork_reference_error)?,
                )
            } else {
                (None, None)
            };
        source_resolver.index_explicit_sources_for_ids(&item, &requested_source_item_ids);

        if matches!(&item, RolloutItem::Compacted(_)) && !keep_checkpoint {
            continue;
        }

        if let RolloutItem::Compacted(compacted) = &mut item {
            if let Some(replacement_history) = resolved_parent_history {
                compacted.replacement_history = Some(replacement_history);
                compacted.replacement_history_entries = None;
            }
            if let Some(guardian_history) = resolved_parent_guardian {
                compacted.guardian_history = Some(guardian_history);
            }
            if history_mode.supports_compacted_history_references() {
                if history_mode.supports_compacted_history_integrity() {
                    destination_resolver
                        .reencode_item_with_integrity_references(&mut item)
                        .map_err(copied_fork_integrity_reference_error)?;
                } else {
                    destination_resolver
                        .reencode_item_with_backward_references(&mut item)
                        .map_err(copied_fork_reference_error)?;
                }
            }
        } else if codex_rollout::is_persisted_rollout_item(&item, history_mode) {
            destination_resolver.index_explicit_sources(&item);
        }
        normalized.push(item);
    }

    Ok(normalized)
}

fn copied_fork_reference_error(mut missing: Vec<String>) -> CodexErr {
    missing.sort_unstable();
    missing.dedup();
    CodexErr::Fatal(format!(
        "cannot copy fork with unresolved compacted history references: {}",
        missing.join(", ")
    ))
}

fn copied_fork_integrity_reference_error(
    error: codex_history::CompactedHistoryReferenceError,
) -> CodexErr {
    CodexErr::Fatal(format!(
        "cannot copy fork with unresolved compacted history references: {error}"
    ))
}
