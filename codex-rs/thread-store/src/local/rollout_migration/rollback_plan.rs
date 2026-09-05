//! Decides which legacy records remain visible after historical rollback.
//!
//! Legacy rollback removes logical instruction turns, not a physical suffix of the rollout file.
//! Most records happen to be ordered that way, but late completion events can target an older
//! surviving turn after a newer turn has started. This planner keeps compact per-record ownership
//! metadata for SQLite visibility, then combines it with `rollback_replay`'s cold-resume answer
//! before the writer makes its second streaming pass. Retained checkpoints are reduced when
//! rollback is encountered, never by reapplying old rollbacks to later checkpoints.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;

use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::UserMessageEvent;
use codex_rollout::CompactedHistoryEntry;
use codex_rollout::CompactedHistoryResolver;
use codex_rollout::RetainedContext;
use codex_rollout::RetainedContextEntry;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutLine;

use super::canonicalizer::CompactionResolution;
use super::migration_error;
use super::rollback;
use super::rollback_replay::ModelReplayPlanner;
use crate::ThreadStoreResult;

#[derive(Clone)]
struct CompactionFrame {
    record_index: usize,
    boundary_depth: usize,
    owner: Option<usize>,
    has_replacement_history: bool,
    referenced_item_ids: Vec<String>,
    retained_context: Option<RetainedContext>,
    has_guardian_history: bool,
    guardian_history_truncations: Vec<GuardianHistoryTruncation>,
    drop_last_n_user_turns: u32,
}

#[derive(Clone, Copy)]
enum CompactionRewrite {
    EmptyReplayAnchor,
    DropLastNUserTurns(u32),
}

#[derive(Clone)]
struct PendingUserResponse {
    boundary: usize,
    content: Vec<ContentItem>,
}

struct InstructionBoundary {
    record_index: usize,
    message_id: Option<ResponseItemId>,
    acceptance_order: Option<u64>,
    alive: bool,
}

struct RetainedFactSource {
    record_index: usize,
    turn_id: String,
    acceptance_order: Option<u64>,
}

#[derive(Clone)]
struct GuardianHistoryTruncation {
    removed_turns: usize,
    first_removed_message_id: Option<ResponseItemId>,
}

/// Compact plan keyed by parsed source-record index.
pub(super) struct RollbackPlan {
    record_boundaries: Vec<Option<usize>>,
    boundary_alive: Vec<bool>,
    compaction_rewrites: HashMap<usize, CompactionRewrite>,
    selected_compaction: Option<usize>,
    requested_item_ids: HashSet<String>,
    retained_context_rewrites: HashMap<usize, Option<RetainedContext>>,
    guardian_history_truncations: HashMap<usize, Vec<GuardianHistoryTruncation>>,
    compacted_history: Mutex<CompactedHistoryResolver>,
}

impl RollbackPlan {
    pub(super) fn record_count(&self) -> usize {
        self.record_boundaries.len()
    }

    pub(super) fn compaction_resolution(&self, record_index: usize) -> CompactionResolution {
        if self.selected_compaction == Some(record_index)
            || self.compaction_rewrites.contains_key(&record_index)
            || self
                .guardian_history_truncations
                .contains_key(&record_index)
        {
            CompactionResolution::Strict
        } else {
            CompactionResolution::PreserveSuperseded
        }
    }

    pub(super) fn apply(
        &self,
        record_index: usize,
        mut line: RolloutLine,
    ) -> ThreadStoreResult<Option<RolloutLine>> {
        let boundary = self
            .record_boundaries
            .get(record_index)
            .ok_or_else(|| migration_error("rollback plan is shorter than source replay"))?;
        if matches!(
            &line.item,
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(_))
        ) {
            return Ok(None);
        }

        let rewrite = self.compaction_rewrites.get(&record_index).copied();
        let boundary_is_dead = boundary.is_some_and(|boundary| !self.boundary_alive[boundary]);
        let mut compacted_history = self
            .compacted_history
            .lock()
            .map_err(|_| migration_error("rollback compaction resolver lock is poisoned"))?;

        // The second pass still indexes explicit source values from records removed by rollback.
        // A surviving checkpoint can legitimately reference one of those legacy records; once
        // materialized, the canonical writer will inline the source because it is absent from the
        // output prefix. Dead checkpoints themselves are not resolved, so an unrelated dangling
        // reference cannot abort migration.
        if boundary_is_dead && !matches!(rewrite, Some(CompactionRewrite::EmptyReplayAnchor)) {
            compacted_history.index_explicit_sources_for_ids(&line.item, &self.requested_item_ids);
            return Ok(None);
        }

        if let RolloutItem::Compacted(compacted) = &mut line.item
            && let Some(retained_context) = self.retained_context_rewrites.get(&record_index)
        {
            compacted.retained_context = retained_context.clone();
        }

        let materialize_compaction = self.selected_compaction == Some(record_index)
            || rewrite.is_some()
            || self
                .guardian_history_truncations
                .contains_key(&record_index);
        if matches!(&line.item, RolloutItem::Compacted(_)) && !materialize_compaction {
            // An older retained checkpoint is not the rollback-aware replay base. Keep its
            // encoded history intact and expose only its explicit source values for later
            // checkpoints. In particular, do not reject a dangling reference that cold resume
            // will never select.
            compacted_history.index_explicit_sources_for_ids(&line.item, &self.requested_item_ids);
            return Ok(Some(line));
        }

        if !matches!(&line.item, RolloutItem::Compacted(_)) {
            compacted_history.index_explicit_sources_for_ids(&line.item, &self.requested_item_ids);
            return Ok(Some(line));
        }

        // Strictly resolve the rollback-aware replay base and every checkpoint whose history must
        // be edited. The plan never owns their payloads, so peak checkpoint memory is bounded to
        // the current source record rather than the sum of historical replacement histories.
        {
            let RolloutItem::Compacted(compacted) = &mut line.item else {
                return Err(migration_error(
                    "strict rollback checkpoint no longer identifies a compaction",
                ));
            };
            if compacted.replacement_history.is_none()
                && let Some(resolved) = compacted_history
                    .resolve_compacted_item(compacted)
                    .map_err(|missing| {
                        migration_error(format!(
                            "compacted history references could not be resolved during rollback replay: {}",
                            missing.join(", ")
                        ))
                    })?
            {
                compacted.replacement_history = Some(resolved);
                compacted.replacement_history_entries = None;
            }
            if compacted.guardian_history.is_some() {
                let resolved = compacted_history
                    .resolve_guardian_history_detailed(compacted)
                    .map_err(|error| {
                        migration_error(format!(
                            "Guardian history references could not be resolved during rollback replay: {error}"
                        ))
                    })?;
                compacted.guardian_history = resolved;
            }
            if let Some(truncations) = self.guardian_history_truncations.get(&record_index) {
                let checkpoint = compacted.guardian_history.as_mut().ok_or_else(|| {
                    migration_error("rollback Guardian rewrite lost its source checkpoint")
                })?;
                for truncation in truncations {
                    truncate_guardian_history(
                        &mut checkpoint.0,
                        truncation.removed_turns,
                        truncation.first_removed_message_id.as_ref(),
                    );
                }
            }
        }

        if matches!(rewrite, Some(CompactionRewrite::EmptyReplayAnchor)) {
            compacted_history.index_explicit_sources_for_ids(&line.item, &self.requested_item_ids);
            let RolloutItem::Compacted(compacted) = &mut line.item else {
                return Err(migration_error(
                    "rollback replay anchor no longer identifies a compaction",
                ));
            };
            compacted.replacement_history = Some(Vec::new());
            compacted.replacement_history_entries = None;
            compacted.mcp_resource_origins = None;
            return Ok(Some(line));
        }
        compacted_history.index_explicit_sources_for_ids(&line.item, &self.requested_item_ids);
        if let Some(CompactionRewrite::DropLastNUserTurns(num_turns)) = rewrite {
            let RolloutItem::Compacted(compacted) = &mut line.item else {
                return Err(migration_error(
                    "rollback history rewrite no longer identifies a compaction",
                ));
            };
            compacted.mcp_resource_origins = None;
            let replacement_history = compacted.replacement_history.as_mut().ok_or_else(|| {
                migration_error("legacy rollback crosses a compaction without replacement history")
            })?;
            rollback::drop_last_n_user_turns(replacement_history, num_turns);
        }
        Ok(Some(line))
    }
}

/// Streaming builder for RollbackPlan.
pub(super) struct RollbackPlanner {
    record_boundaries: Vec<Option<usize>>,
    boundaries: Vec<InstructionBoundary>,
    boundary_stack: Vec<usize>,
    active_turn_id: Option<String>,
    pending_turn_records: Vec<usize>,
    pending_context_records: Vec<usize>,
    pending_user_response: Option<PendingUserResponse>,
    pending_delivery_boundary: Option<usize>,
    turn_boundaries: HashMap<String, usize>,
    call_boundaries: HashMap<(String, String), Option<usize>>,
    retained_fact_sources: Vec<RetainedFactSource>,
    compactions: Vec<CompactionFrame>,
    model_replay: ModelReplayPlanner,
}

impl RollbackPlanner {
    pub(super) fn new() -> Self {
        Self {
            record_boundaries: Vec::new(),
            boundaries: Vec::new(),
            boundary_stack: Vec::new(),
            active_turn_id: None,
            pending_turn_records: Vec::new(),
            pending_context_records: Vec::new(),
            pending_user_response: None,
            pending_delivery_boundary: None,
            turn_boundaries: HashMap::new(),
            call_boundaries: HashMap::new(),
            retained_fact_sources: Vec::new(),
            compactions: Vec::new(),
            model_replay: ModelReplayPlanner::new(),
        }
    }

    pub(super) fn observe(&mut self, line: &RolloutLine) -> ThreadStoreResult<()> {
        let index = self.record_boundaries.len();
        self.model_replay.observe(index, &line.item);
        self.record_boundaries
            .push(self.boundary_stack.last().copied());
        let paired_user_boundary = match (&self.pending_user_response, &line.item) {
            (Some(pending), RolloutItem::EventMsg(EventMsg::UserMessage(event)))
                if user_response_matches_event(&pending.content, event) =>
            {
                Some(pending.boundary)
            }
            _ => None,
        };
        let paired_delivery_boundary = match (&self.pending_delivery_boundary, &line.item) {
            (Some(boundary), RolloutItem::ResponseItem(response))
                if matches!(&response.item, ResponseItem::AgentMessage { .. }) =>
            {
                Some(*boundary)
            }
            _ => None,
        };
        self.pending_user_response = None;
        self.pending_delivery_boundary = None;

        match &line.item {
            RolloutItem::SessionMeta(_) => self.record_boundaries[index] = None,
            RolloutItem::ResponseItem(response) => {
                if let Some(boundary) = paired_delivery_boundary {
                    self.record_boundaries[index] = Some(boundary);
                    self.boundaries[boundary].message_id = response.id().cloned();
                } else if rollback::counts_as_boundary(&response.item) {
                    let boundary = self.start_boundary(index);
                    self.boundaries[boundary].message_id = response.id().cloned();
                    self.boundaries[boundary].acceptance_order = response
                        .metadata
                        .as_ref()
                        .and_then(|metadata| metadata.user_input_order);
                    if let ResponseItem::Message { role, content, .. } = &response.item
                        && role == "user"
                    {
                        self.pending_user_response = Some(PendingUserResponse {
                            boundary,
                            content: content.clone(),
                        });
                    }
                } else if rollback::is_pre_turn_context_update(&response.item) {
                    // Until another user boundary arrives, this is trailing context for the
                    // previous turn. Keep that fallback owner so rollback drops it when there is
                    // no later turn to attach it to.
                    self.pending_context_records.push(index);
                }
                if let ResponseItem::FunctionCall { call_id, .. } = &response.item
                    && let Some(turn_id) = response.turn_id().or(self.active_turn_id.as_deref())
                {
                    self.call_boundaries.insert(
                        (turn_id.to_owned(), call_id.clone()),
                        self.record_boundaries[index],
                    );
                }
            }
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(rollback)) => {
                self.apply_rollback(rollback.num_turns)?;
            }
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => {
                self.active_turn_id = Some(event.turn_id.clone());
                self.pending_turn_records.clear();
                self.pending_turn_records.push(index);
            }
            RolloutItem::EventMsg(EventMsg::TurnComplete(event)) => {
                self.assign_targeted_record(index, Some(event.turn_id.as_str()));
                if self.active_turn_id.as_deref() == Some(event.turn_id.as_str()) {
                    self.active_turn_id = None;
                    self.pending_turn_records.clear();
                }
            }
            RolloutItem::EventMsg(EventMsg::TurnAborted(event)) => {
                self.assign_targeted_record(index, event.turn_id.as_deref());
                if event
                    .turn_id
                    .as_deref()
                    .is_some_and(|turn_id| self.active_turn_id.as_deref() == Some(turn_id))
                {
                    self.active_turn_id = None;
                    self.pending_turn_records.clear();
                }
            }
            RolloutItem::EventMsg(EventMsg::UserMessage(_)) => {
                let boundary = paired_user_boundary.unwrap_or_else(|| self.start_boundary(index));
                self.record_boundaries[index] = Some(boundary);
            }
            RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) => {
                self.assign_targeted_record(index, Some(event.turn_id.as_str()));
            }
            RolloutItem::EventMsg(event) => {
                self.assign_targeted_record(index, explicit_event_turn_id(event));
            }
            RolloutItem::InterAgentCommunication(_) => {
                self.start_boundary(index);
            }
            RolloutItem::InterAgentCommunicationMetadata { .. } => {
                let boundary = self.start_boundary(index);
                self.pending_delivery_boundary = Some(boundary);
            }
            RolloutItem::Compacted(item) => {
                let owner = self
                    .active_turn_id
                    .as_deref()
                    .and_then(|turn_id| self.turn_boundaries.get(turn_id).copied());
                self.record_boundaries[index] = owner;
                self.compactions.push(CompactionFrame {
                    record_index: index,
                    boundary_depth: self.boundary_stack.len(),
                    owner,
                    has_replacement_history: item.replacement_history.is_some()
                        || item.replacement_history_entries.is_some(),
                    referenced_item_ids: item
                        .replacement_history_entries
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .chain(
                            item.guardian_history
                                .as_ref()
                                .and_then(|checkpoint| checkpoint.entries())
                                .into_iter()
                                .flatten(),
                        )
                        .filter_map(|entry| match entry {
                            CompactedHistoryEntry::Reference { item_id }
                            | CompactedHistoryEntry::ReferenceV2 { item_id, .. } => {
                                Some(item_id.clone())
                            }
                            CompactedHistoryEntry::Inline { .. } => None,
                        })
                        .collect(),
                    retained_context: item.retained_context.clone(),
                    has_guardian_history: item.guardian_history.is_some(),
                    guardian_history_truncations: Vec::new(),
                    drop_last_n_user_turns: 0,
                });
            }
            RolloutItem::TurnContext(_) => {
                if self.active_turn_id.is_some()
                    && self
                        .active_turn_id
                        .as_deref()
                        .is_none_or(|turn_id| !self.turn_boundaries.contains_key(turn_id))
                {
                    self.pending_turn_records.push(index);
                }
            }
            RolloutItem::TokenUsageRecord(record) => {
                self.assign_targeted_record(index, Some(record.turn_id.as_str()));
            }
            RolloutItem::WorldState(_) | RolloutItem::RealtimeItem(_) => {}
            RolloutItem::RetainedContext(codex_rollout::RetainedContextEvent::VerifiedAnswer {
                answer,
                acceptance_order,
            }) => {
                let source = (answer.turn_id.clone(), answer.call_id.clone());
                // A late answer still belongs to its call's instruction boundary, even
                // if the same running turn has since received another user steer.
                if let Some(boundary) = self.call_boundaries.get(&source) {
                    self.record_boundaries[index] = *boundary;
                } else {
                    self.assign_targeted_record(index, Some(&answer.turn_id));
                }
                self.call_boundaries
                    .insert(source, self.record_boundaries[index]);
                self.retained_fact_sources.push(RetainedFactSource {
                    record_index: index,
                    turn_id: answer.turn_id.clone(),
                    acceptance_order: *acceptance_order,
                });
            }
            RolloutItem::SecurityRiskScore(_) => self.record_boundaries[index] = None,
        }

        Ok(())
    }

    pub(super) fn finish(self) -> RollbackPlan {
        let RollbackPlanner {
            record_boundaries,
            boundaries,
            compactions,
            model_replay,
            ..
        } = self;
        let replay_plan = model_replay.finish();
        let replay_anchor = replay_plan.empty_replacement_history_compaction;
        let boundary_alive = boundaries
            .iter()
            .map(|boundary| boundary.alive)
            .collect::<Vec<_>>();
        let requested_item_ids = compactions
            .iter()
            .filter(|frame| {
                replay_plan.selected_compaction == Some(frame.record_index)
                    || replay_anchor == Some(frame.record_index)
                    || !frame.guardian_history_truncations.is_empty()
                    || (frame.drop_last_n_user_turns > 0
                        && frame.owner.is_none_or(|boundary| boundary_alive[boundary]))
            })
            .flat_map(|frame| frame.referenced_item_ids.iter().cloned())
            .collect::<HashSet<_>>();
        let retained_context_rewrites = compactions
            .iter()
            .filter(|frame| {
                Some(frame.record_index) == replay_anchor
                    || frame.owner.is_none_or(|boundary| boundary_alive[boundary])
            })
            .map(|frame| (frame.record_index, frame.retained_context.clone()))
            .collect::<HashMap<_, _>>();
        let guardian_history_truncations = compactions
            .iter()
            .filter(|frame| {
                Some(frame.record_index) == replay_anchor
                    || frame.owner.is_none_or(|boundary| boundary_alive[boundary])
            })
            .filter(|frame| !frame.guardian_history_truncations.is_empty())
            .map(|frame| {
                (
                    frame.record_index,
                    frame.guardian_history_truncations.clone(),
                )
            })
            .collect::<HashMap<_, _>>();
        let compaction_rewrites = compactions
            .into_iter()
            .filter_map(|frame| {
                if Some(frame.record_index) == replay_anchor {
                    return Some((frame.record_index, CompactionRewrite::EmptyReplayAnchor));
                }
                (frame.drop_last_n_user_turns > 0
                    && frame.owner.is_none_or(|boundary| boundary_alive[boundary]))
                .then_some((
                    frame.record_index,
                    CompactionRewrite::DropLastNUserTurns(frame.drop_last_n_user_turns),
                ))
            })
            .collect::<HashMap<_, _>>();
        RollbackPlan {
            record_boundaries,
            boundary_alive,
            compaction_rewrites,
            selected_compaction: replay_plan.selected_compaction,
            requested_item_ids,
            retained_context_rewrites,
            guardian_history_truncations,
            compacted_history: Mutex::new(CompactedHistoryResolver::default()),
        }
    }

    fn start_boundary(&mut self, index: usize) -> usize {
        let boundary = self.boundaries.len();
        self.boundaries.push(InstructionBoundary {
            record_index: index,
            message_id: None,
            acceptance_order: None,
            alive: true,
        });
        let had_prior_boundary = !self.boundary_stack.is_empty();
        if had_prior_boundary {
            for pending_index in self.pending_context_records.drain(..) {
                self.record_boundaries[pending_index] = Some(boundary);
            }
        } else {
            self.pending_context_records.clear();
        }
        for pending_index in self.pending_turn_records.drain(..) {
            self.record_boundaries[pending_index] = Some(boundary);
        }
        self.record_boundaries[index] = Some(boundary);
        self.boundary_stack.push(boundary);
        self.bind_active_turn(boundary);
        boundary
    }

    fn bind_active_turn(&mut self, boundary: usize) {
        if let Some(turn_id) = self.active_turn_id.as_ref() {
            self.turn_boundaries.insert(turn_id.clone(), boundary);
        }
    }

    fn assign_targeted_record(&mut self, index: usize, turn_id: Option<&str>) {
        if let Some(boundary) = turn_id.and_then(|turn_id| self.turn_boundaries.get(turn_id)) {
            self.record_boundaries[index] = Some(*boundary);
        } else if self.active_turn_id.is_some()
            && self
                .active_turn_id
                .as_deref()
                .is_none_or(|turn_id| !self.turn_boundaries.contains_key(turn_id))
        {
            self.pending_turn_records.push(index);
        }
    }

    fn apply_rollback(&mut self, num_turns: u32) -> ThreadStoreResult<()> {
        let count = usize::try_from(num_turns).unwrap_or(usize::MAX);
        if count == 0 {
            return Ok(());
        }
        let depth_before = self.boundary_stack.len();
        let mut removed_boundaries = HashSet::new();
        let mut first_removed_boundary = None;
        for _ in 0..count {
            let Some(boundary) = self.boundary_stack.pop() else {
                break;
            };
            self.boundaries[boundary].alive = false;
            removed_boundaries.insert(boundary);
            first_removed_boundary = Some(boundary);
        }
        if let Some(boundary) = first_removed_boundary {
            let source = &self.boundaries[boundary];
            let depth_after = self.boundary_stack.len();
            for frame in &mut self.compactions {
                let removed_from_checkpoint = frame
                    .boundary_depth
                    .min(depth_before)
                    .saturating_sub(frame.boundary_depth.min(depth_after));
                if removed_from_checkpoint > 0 && frame.has_guardian_history {
                    frame
                        .guardian_history_truncations
                        .push(GuardianHistoryTruncation {
                            removed_turns: removed_from_checkpoint,
                            first_removed_message_id: source.message_id.clone(),
                        });
                }
            }
            if let Some(order) = source.acceptance_order {
                // An answer may have been persisted before the queued instruction
                // accepted ahead of it. Both are removed at that acceptance boundary.
                for fact in &self.retained_fact_sources {
                    if fact
                        .acceptance_order
                        .is_some_and(|accepted| accepted >= order)
                    {
                        self.record_boundaries[fact.record_index] = Some(boundary);
                    }
                }
            }
            let removed_turns = self
                .turn_boundaries
                .iter()
                .filter(|(_, boundary)| removed_boundaries.contains(*boundary))
                .map(|(turn_id, _)| turn_id.as_str())
                .chain(
                    self.retained_fact_sources
                        .iter()
                        .filter(|fact| {
                            self.record_boundaries[fact.record_index]
                                .is_some_and(|boundary| removed_boundaries.contains(&boundary))
                        })
                        .map(|fact| fact.turn_id.as_str()),
                )
                .collect::<Vec<_>>();
            // Accepted answers can precede a queued instruction in the rollout,
            // including in checkpoints. Legacy evidence uses the recorded boundary.
            // Later checkpoints have not been observed yet and already reflect this rollback.
            for frame in self.compactions.iter_mut().rev().take_while(|frame| {
                source.acceptance_order.is_some() || frame.record_index >= source.record_index
            }) {
                if let Some(context) = &mut frame.retained_context {
                    if source.acceptance_order.is_some()
                        || context
                            .ordered_entries()
                            .any(|entry| matches!(entry, RetainedContextEntry::UserMessage(_)))
                    {
                        context.rollback(
                            &removed_turns,
                            source.message_id.as_ref().map(ResponseItemId::as_str),
                            source.acceptance_order,
                        );
                    } else {
                        // Checkpoints written without instruction retention keep the legacy
                        // source-call boundary, including answers before a same-turn steer.
                        context.retain_answers(|answer| {
                            self.call_boundaries
                                .get(&(answer.turn_id.clone(), answer.call_id.clone()))
                                .map_or_else(
                                    || !removed_turns.contains(&answer.turn_id.as_str()),
                                    |boundary| {
                                        boundary
                                            .is_none_or(|boundary| self.boundaries[boundary].alive)
                                    },
                                )
                        });
                    }
                }
            }
        }
        let compaction_index = self.compactions.iter().rposition(|frame| {
            frame
                .owner
                .is_none_or(|boundary| self.boundaries[boundary].alive)
        });
        if let Some(compaction_index) = compaction_index {
            let frame = &mut self.compactions[compaction_index];
            let post_compaction_turns = depth_before.saturating_sub(frame.boundary_depth);
            let remaining = count.saturating_sub(post_compaction_turns);
            if remaining > 0 {
                if !frame.has_replacement_history {
                    return Err(migration_error(
                        "legacy rollback crosses a compaction without replacement history",
                    ));
                }
                frame.drop_last_n_user_turns = frame
                    .drop_last_n_user_turns
                    .saturating_add(u32::try_from(remaining).unwrap_or(u32::MAX));
            }
        }
        self.active_turn_id = None;
        self.pending_turn_records.clear();
        self.pending_context_records.clear();
        self.pending_user_response = None;
        self.pending_delivery_boundary = None;
        Ok(())
    }
}

/// Mirror `TranscriptHistory::truncate_before` for a persisted checkpoint without making
/// thread-store depend on the live Guardian context implementation. IDs identify the exact
/// boundary when available. Older ID-less rollouts fall back to the checkpoint's ordered
/// instruction boundaries; if retention evicted the requested boundary, clear the remaining
/// evidence rather than risk preserving a later grant.
fn truncate_guardian_history(
    history: &mut Vec<ResponseItem>,
    removed_turns: usize,
    first_removed_message_id: Option<&ResponseItemId>,
) {
    if removed_turns == 0 {
        return;
    }
    let boundary_index = if let Some(message_id) = first_removed_message_id {
        history
            .iter()
            .position(|item| item.id() == Some(message_id))
    } else {
        let mut remaining = removed_turns;
        history.iter().enumerate().rev().find_map(|(index, item)| {
            if !rollback::counts_as_boundary(item) {
                return None;
            }
            remaining -= 1;
            (remaining == 0).then_some(index)
        })
    };
    history.truncate(boundary_index.unwrap_or(0));
}

fn explicit_event_turn_id(event: &EventMsg) -> Option<&str> {
    match event {
        EventMsg::ExecCommandEnd(event) => Some(event.turn_id.as_str()),
        EventMsg::PatchApplyEnd(event) => Some(event.turn_id.as_str()),
        EventMsg::DynamicToolCallResponse(event) => Some(event.turn_id.as_str()),
        EventMsg::EnteredReviewMode(event) => event.turn_id.as_deref(),
        EventMsg::ExitedReviewMode(event) => event.turn_id.as_deref(),
        _ => None,
    }
    .filter(|turn_id| !turn_id.is_empty())
}

fn user_response_matches_event(content: &[ContentItem], event: &UserMessageEvent) -> bool {
    let mut text = String::new();
    let mut images = Vec::new();
    let mut audio = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text: item_text } => text.push_str(item_text),
            ContentItem::InputImage { image_url, .. } => images.push(image_url.as_str()),
            ContentItem::InputAudio { audio_url } => audio.push(audio_url.as_str()),
            ContentItem::OutputText { .. } => return false,
        }
    }
    text == event.message
        && images
            == event
                .images
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        && audio
            == event
                .audio
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        && event.local_images.is_empty()
        && event.local_audio.is_empty()
}
