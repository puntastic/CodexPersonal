use std::collections::HashSet;

use crate::ResponseItemEnvelope;
use crate::RolloutItem;
use codex_history::CompactedHistoryEntry;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::SessionMetaLine;

/// Whether a reverse model-context scan needs more rollout items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelContextScanProgress {
    /// The reader should provide the next older rollout item.
    Continue,
    /// The scan has collected a safe bounded suffix.
    Complete,
}

/// Accumulates newest-to-oldest rollout items until they are sufficient to reconstruct the latest
/// model context.
///
/// Storage implementations own how they fetch older items. Local JSONL readers and future
/// reverse-paged cloud readers can both feed their items through this scan to share the cutoff
/// rules and chronological replay assembly.
///
/// The scan stops once it has both:
///
/// - `saw_compaction`: a `CompactedItem` with one replacement-history form and `window_number`;
/// - `saw_completed_turn_context`: a completed user turn with a compatible `TurnContextItem`.
///
/// An entry-backed replacement history is only a bounded base once every `Reference` has found an
/// older source item. Sources may be top-level response items, legacy replacement-history items,
/// or inline entries in older entry-backed checkpoints.
///
/// Before the newest usable checkpoint is found, the scan retains the complete suffix. Older
/// records are retained only when they are an explicit source requested by that checkpoint or are
/// needed to reconstruct one completed user-turn context. This keeps a successful scan bounded by
/// the context it will actually replay, even when a requested source is very old.
///
/// `TurnContextItem` does not identify whether it came from a user turn, so one only counts after
/// the same turn also proves a user-turn boundary: a paginated
/// `ItemCompleted(UserMessage)` marker, agent message, or inter-agent message. Paginated writers
/// persist that marker for real user turns; older rollouts without it conservatively scan to the
/// beginning. A raw `role=user` response item is not sufficient because contextual user fragments
/// use that role but do not count as turn boundaries during reconstruction. The compaction restores
/// model-visible items; the turn context restores previous settings (`model`, `comp_hash`, and
/// `realtime_active`) and the reference baseline.
///
/// These paginated shapes disable the bounded cutoff:
///
/// - compaction without exactly one replacement-history form or without `window_number`;
/// - rollback markers;
///
/// When one appears, the scanner continues to the beginning. Callers that need the legacy full-
/// replay fallback must reread their source after the scan reaches the beginning; dropped pre-base
/// payloads cannot be reconstructed by this one-pass accumulator.
#[derive(Debug, Default)]
pub struct ModelContextScan {
    items_newest_first: Vec<RolloutItem>,
    saw_compaction: bool,
    saw_completed_turn_context: bool,
    must_scan_to_start: bool,
    unresolved_reference_ids: HashSet<String>,
    active_segment: ActiveTurnSegment,
}

impl ModelContextScan {
    /// Adds the next newest-to-oldest rollout item and reports whether the reader can stop.
    pub fn push(&mut self, item: RolloutItem) -> ModelContextScanProgress {
        let retain_complete_item = !self.saw_compaction;
        let observation = self.observe(&item);

        if self.must_scan_to_start {
            // The caller must reread for the exact fallback, so do not carry a doomed partial
            // replay through the rest of the reverse scan.
            self.items_newest_first.clear();
            self.active_segment = ActiveTurnSegment::default();
            return ModelContextScanProgress::Continue;
        }

        if retain_complete_item {
            // Everything newer than and including the selected checkpoint is the exact replay
            // suffix. Do not classify or trim any of it.
            self.items_newest_first.push(item);
        } else if observation.explicit_source_carrier.is_some() || observation.context_metadata {
            self.active_segment
                .retained_items_newest_first
                .push(RetainedPreBaseItem {
                    item: observation.explicit_source_carrier.unwrap_or(item),
                    explicit_source: observation.is_explicit_source,
                    context_metadata: observation.context_metadata,
                });
        }

        if observation.finalizes_segment {
            self.finalize_active_segment();
        }

        if self.has_bounded_cutoff() {
            // A source can complete the scan in the middle of an older turn. Preserve sources from
            // that unfinished segment, but do not recruit another metadata segment.
            self.flush_active_segment_sources();
            ModelContextScanProgress::Complete
        } else {
            ModelContextScanProgress::Continue
        }
    }

    /// Returns the collected items in chronological order with canonical head metadata.
    ///
    /// Call this after [`Self::push`] returns [`ModelContextScanProgress::Complete`]. An incomplete
    /// scan is still returned chronologically, but it is not a full-replay fallback once the scan
    /// has discarded unrelated pre-base records.
    pub fn finish(mut self, session_meta: SessionMetaLine) -> Vec<RolloutItem> {
        self.items_newest_first.reverse();
        if self.has_bounded_cutoff() {
            // A bounded scan stops before reaching the head. Prepend the separately loaded head
            // SessionMeta, which remains canonical when copied fork history contains later
            // metadata.
            self.items_newest_first
                .insert(0, RolloutItem::SessionMeta(session_meta));
        }
        self.items_newest_first
    }

    fn observe(&mut self, item: &RolloutItem) -> ItemObservation {
        if self.must_scan_to_start {
            return ItemObservation::default();
        }

        let mut observation = ItemObservation {
            context_metadata: item_is_completed_turn_context_metadata(item),
            ..Default::default()
        };

        match item {
            RolloutItem::Compacted(compacted) => {
                if compacted.window_number.is_none()
                    || compacted.replacement_history.is_some()
                        == compacted.replacement_history_entries.is_some()
                {
                    self.must_scan_to_start = true;
                } else {
                    let is_selected_base = !self.saw_compaction;
                    self.saw_compaction = true;
                    if is_selected_base {
                        // Register references only after examining the selected checkpoint. An
                        // inline entry in that same checkpoint is not an older explicit source.
                        if let Some(entries) = &compacted.replacement_history_entries {
                            for entry in entries {
                                if let CompactedHistoryEntry::Reference { item_id } = entry {
                                    self.unresolved_reference_ids.insert(item_id.clone());
                                }
                            }
                        }
                    } else if let Some(carrier) = self.explicit_source_carrier(item) {
                        observation.explicit_source_carrier = Some(carrier);
                        observation.is_explicit_source = true;
                    }
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(_)) => {
                // Paginated threads reject rollback. Keep old rollouts correct rather than
                // duplicating rollback survival semantics in this bounded selector.
                self.must_scan_to_start = true;
            }
            RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) => {
                if self.active_segment.turn_id.is_none() {
                    self.active_segment.turn_id = Some(event.turn_id.clone());
                }
                if turn_ids_are_compatible(
                    self.active_segment.turn_id.as_deref(),
                    Some(event.turn_id.as_str()),
                ) {
                    self.active_segment.has_user_turn |=
                        matches!(&event.item, TurnItem::UserMessage(_));
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                self.active_segment
                    .turn_id
                    .get_or_insert_with(|| event.turn_id.clone());
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                if let Some(turn_id) = &event.turn_id {
                    self.active_segment
                        .turn_id
                        .get_or_insert_with(|| turn_id.clone());
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                if turn_ids_are_compatible(
                    self.active_segment.turn_id.as_deref(),
                    Some(event.turn_id.as_str()),
                ) {
                    observation.finalizes_segment = true;
                }
            }
            RolloutItem::TurnContext(context) => {
                if self.active_segment.turn_id.is_none() {
                    self.active_segment.turn_id = context.turn_id.clone();
                }
                if turn_ids_are_compatible(
                    self.active_segment.turn_id.as_deref(),
                    context.turn_id.as_deref(),
                ) {
                    self.active_segment.has_turn_context = true;
                }
            }
            RolloutItem::ResponseItem(response_item) => {
                if self.saw_compaction
                    && let Some(carrier) = self.explicit_source_carrier(item)
                {
                    observation.explicit_source_carrier = Some(carrier);
                    observation.is_explicit_source = true;
                }
                self.active_segment.has_user_turn |=
                    response_item_counts_as_user_turn(response_item);
            }
            RolloutItem::InterAgentCommunication(_) => {
                self.active_segment.has_user_turn = true;
            }
            RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                self.active_segment.has_user_turn = true;
            }
            RolloutItem::EventMsg(_)
            | RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::RealtimeItem(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::WorldState(_) => {}
        }

        observation
    }

    fn finalize_active_segment(&mut self) {
        let active_segment = std::mem::take(&mut self.active_segment);
        let retain_context_metadata = !self.saw_completed_turn_context
            && active_segment.has_user_turn
            && active_segment.has_turn_context;
        if retain_context_metadata {
            self.saw_completed_turn_context = true;
        }

        self.items_newest_first.extend(
            active_segment
                .retained_items_newest_first
                .into_iter()
                .filter_map(|retained| {
                    (retained.explicit_source
                        || (retain_context_metadata && retained.context_metadata))
                        .then_some(retained.item)
                }),
        );
    }

    fn flush_active_segment_sources(&mut self) {
        self.items_newest_first.extend(
            std::mem::take(&mut self.active_segment.retained_items_newest_first)
                .into_iter()
                .filter_map(|retained| retained.explicit_source.then_some(retained.item)),
        );
    }

    fn explicit_source_carrier(&mut self, item: &RolloutItem) -> Option<RolloutItem> {
        match item {
            RolloutItem::ResponseItem(envelope) => {
                let item_id = envelope.item.id()?;
                self.unresolved_reference_ids
                    .remove(item_id.as_str())
                    .then(|| item.clone())
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    let mut found_ids = HashSet::new();
                    let mut retained = replacement_history
                        .iter()
                        .rev()
                        .filter_map(|envelope| {
                            let item_id = envelope.item.id()?;
                            let item_id = item_id.as_str();
                            (self.unresolved_reference_ids.contains(item_id)
                                && found_ids.insert(item_id.to_string()))
                            .then(|| envelope.clone())
                        })
                        .collect::<Vec<_>>();
                    if retained.is_empty() {
                        return None;
                    }
                    retained.reverse();
                    for item_id in found_ids {
                        self.unresolved_reference_ids.remove(&item_id);
                    }

                    let mut carrier = compacted.clone();
                    carrier.replacement_history = Some(retained);
                    carrier.replacement_history_entries = None;
                    Some(RolloutItem::Compacted(carrier))
                } else if let Some(entries) = &compacted.replacement_history_entries {
                    let mut found_ids = HashSet::new();
                    let mut retained = entries
                        .iter()
                        .rev()
                        .filter_map(|entry| {
                            let CompactedHistoryEntry::Inline { item, .. } = entry else {
                                return None;
                            };
                            let item_id = item.id()?;
                            let item_id = item_id.as_str();
                            (self.unresolved_reference_ids.contains(item_id)
                                && found_ids.insert(item_id.to_string()))
                            .then(|| entry.clone())
                        })
                        .collect::<Vec<_>>();
                    if retained.is_empty() {
                        return None;
                    }
                    retained.reverse();
                    for item_id in found_ids {
                        self.unresolved_reference_ids.remove(&item_id);
                    }

                    let mut carrier = compacted.clone();
                    carrier.replacement_history = None;
                    carrier.replacement_history_entries = Some(retained);
                    Some(RolloutItem::Compacted(carrier))
                } else {
                    None
                }
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_) => None,
        }
    }

    fn has_bounded_cutoff(&self) -> bool {
        !self.must_scan_to_start
            && self.saw_compaction
            && self.saw_completed_turn_context
            && self.unresolved_reference_ids.is_empty()
    }
}

#[derive(Debug, Default)]
struct ActiveTurnSegment {
    turn_id: Option<String>,
    has_user_turn: bool,
    has_turn_context: bool,
    retained_items_newest_first: Vec<RetainedPreBaseItem>,
}

#[derive(Debug)]
struct RetainedPreBaseItem {
    item: RolloutItem,
    explicit_source: bool,
    context_metadata: bool,
}

#[derive(Debug, Default)]
struct ItemObservation {
    explicit_source_carrier: Option<RolloutItem>,
    is_explicit_source: bool,
    context_metadata: bool,
    finalizes_segment: bool,
}

fn turn_ids_are_compatible(active_turn_id: Option<&str>, item_turn_id: Option<&str>) -> bool {
    active_turn_id
        .is_none_or(|turn_id| item_turn_id.is_none_or(|item_turn_id| item_turn_id == turn_id))
}

fn response_item_counts_as_user_turn(response_item: &ResponseItemEnvelope) -> bool {
    match &response_item.item {
        ResponseItem::AgentMessage { .. } => true,
        ResponseItem::Message { role, content, .. } => {
            role == "assistant" && InterAgentCommunication::is_message_content(content)
        }
        _ => false,
    }
}

fn item_is_completed_turn_context_metadata(item: &RolloutItem) -> bool {
    match item {
        RolloutItem::TurnContext(_) | RolloutItem::InterAgentCommunication(_) => true,
        RolloutItem::ResponseItem(response_item) => match &response_item.item {
            ResponseItem::AgentMessage { .. } => true,
            ResponseItem::Message { role, content, .. } => {
                role == "user"
                    || (role == "assistant" && InterAgentCommunication::is_message_content(content))
            }
            _ => false,
        },
        RolloutItem::EventMsg(
            EventMsg::TurnStarted(_)
            | EventMsg::TurnComplete(_)
            | EventMsg::TurnAborted(_)
            | EventMsg::UserMessage(_),
        ) => true,
        RolloutItem::SessionMeta(_)
        | RolloutItem::Compacted(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. }
        | RolloutItem::WorldState(_)
        | RolloutItem::SecurityRiskScore(_)
        | RolloutItem::EventMsg(_)
        | RolloutItem::RealtimeItem(_) => false,
    }
}

#[cfg(test)]
#[path = "model_context_tests.rs"]
mod tests;
