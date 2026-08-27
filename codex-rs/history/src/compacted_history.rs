use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;

use crate::CompactedHistoryDigest;
use crate::CompactedHistoryEntry;
use crate::ResponseItemEnvelope;
use crate::RolloutItem;

/// One V2 reference whose addressed source exists but does not match its persisted digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedHistoryDigestMismatch {
    item_id: String,
    expected: CompactedHistoryDigest,
    actual: CompactedHistoryDigest,
}

impl CompactedHistoryDigestMismatch {
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    pub fn expected(&self) -> &CompactedHistoryDigest {
        &self.expected
    }

    pub fn actual(&self) -> &CompactedHistoryDigest {
        &self.actual
    }
}

/// Typed failure for resolving a compacted-history reference set.
///
/// Missing sources and same-ID substitutions are kept in separate buckets so callers can choose
/// recovery for absence without accidentally treating an integrity failure as ordinary absence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactedHistoryReferenceError {
    missing_item_ids: Vec<String>,
    digest_mismatches: Vec<CompactedHistoryDigestMismatch>,
    digest_encoding_failure_item_ids: Vec<String>,
}

impl CompactedHistoryReferenceError {
    pub fn missing_item_ids(&self) -> &[String] {
        &self.missing_item_ids
    }

    pub fn digest_mismatches(&self) -> &[CompactedHistoryDigestMismatch] {
        &self.digest_mismatches
    }

    pub fn digest_encoding_failure_item_ids(&self) -> &[String] {
        &self.digest_encoding_failure_item_ids
    }

    /// IDs affected by any resolution failure, retained for V1-oriented diagnostics.
    pub fn affected_item_ids(&self) -> Vec<String> {
        let mut item_ids = self.missing_item_ids.clone();
        item_ids.extend(
            self.digest_mismatches
                .iter()
                .map(|mismatch| mismatch.item_id.clone()),
        );
        item_ids.extend(self.digest_encoding_failure_item_ids.iter().cloned());
        item_ids.sort_unstable();
        item_ids.dedup();
        item_ids
    }

    fn empty() -> Self {
        Self {
            missing_item_ids: Vec::new(),
            digest_mismatches: Vec::new(),
            digest_encoding_failure_item_ids: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.missing_item_ids.is_empty()
            && self.digest_mismatches.is_empty()
            && self.digest_encoding_failure_item_ids.is_empty()
    }

    fn normalize(&mut self) {
        self.missing_item_ids.sort_unstable();
        self.missing_item_ids.dedup();
        self.digest_mismatches.sort_unstable_by(|left, right| {
            left.item_id
                .cmp(&right.item_id)
                .then_with(|| left.expected.as_str().cmp(right.expected.as_str()))
                .then_with(|| left.actual.as_str().cmp(right.actual.as_str()))
        });
        self.digest_mismatches.dedup();
        self.digest_encoding_failure_item_ids.sort_unstable();
        self.digest_encoding_failure_item_ids.dedup();
    }

    fn normalized(mut self) -> Self {
        self.normalize();
        self
    }
}

impl fmt::Display for CompactedHistoryReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if !self.missing_item_ids.is_empty() {
            parts.push(format!(
                "missing sources: {}",
                self.missing_item_ids.join(", ")
            ));
        }
        if !self.digest_mismatches.is_empty() {
            parts.push(format!(
                "digest mismatches: {}",
                self.digest_mismatches
                    .iter()
                    .map(|mismatch| format!(
                        "{} (expected {}, actual {})",
                        mismatch.item_id, mismatch.expected, mismatch.actual
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.digest_encoding_failure_item_ids.is_empty() {
            parts.push(format!(
                "sources that could not be canonically encoded: {}",
                self.digest_encoding_failure_item_ids.join(", ")
            ));
        }
        formatter.write_str(&parts.join("; "))
    }
}

impl std::error::Error for CompactedHistoryReferenceError {}

/// A rollout view whose reference-backed compaction checkpoints have been resolved when possible.
pub struct MaterializedCompactedHistories<'a> {
    pub rollout_items: Cow<'a, [RolloutItem]>,
    pub unresolved_item_ids: Vec<String>,
}

/// Incrementally resolves reference-backed compaction histories in chronological rollout order.
#[derive(Default)]
pub struct CompactedHistoryResolver {
    known_items: HashMap<String, ResponseItemEnvelope>,
}

impl CompactedHistoryResolver {
    /// Retains candidate envelopes that have an exact explicit source in the rollout.
    ///
    /// Only IDs present in `candidates` are collected while scanning the rollout, so qualifying a
    /// small active history does not clone every distinct historical source. When an ID was
    /// explicitly stored more than once, the newest value wins, matching chronological indexing.
    /// References are not treated as explicit sources. Candidate order is preserved, and both the
    /// response item and its harness metadata must match the stored source.
    pub fn filter_exact_explicit_sources(
        rollout_items: &[RolloutItem],
        candidates: &[ResponseItemEnvelope],
    ) -> Vec<ResponseItemEnvelope> {
        let requested_item_ids = candidates
            .iter()
            .filter_map(|candidate| candidate.item.id())
            .map(|item_id| item_id.as_str().to_string())
            .collect::<HashSet<_>>();
        let explicit_sources = latest_explicit_sources_for_ids(rollout_items, &requested_item_ids);

        candidates
            .iter()
            .filter(|candidate| {
                candidate.item.id().is_some_and(|item_id| {
                    explicit_sources.get(item_id.as_str()) == Some(*candidate)
                })
            })
            .cloned()
            .collect()
    }

    /// Indexes the complete response-item values contributed by one rollout item.
    ///
    /// Entry-backed checkpoints are validated against the resolver's older sources before any
    /// inline value from the checkpoint is indexed. A missing reference therefore leaves the
    /// resolver unchanged, and an inline entry can never satisfy a reference from its own
    /// checkpoint.
    pub fn index_item(
        &mut self,
        rollout_item: &RolloutItem,
    ) -> Result<(), Vec<String>> {
        self.index_item_detailed(rollout_item)
            .map_err(|error| error.affected_item_ids())
    }

    /// Typed form of [`Self::index_item`] for integrity-aware callers.
    pub fn index_item_detailed(
        &mut self,
        rollout_item: &RolloutItem,
    ) -> Result<(), CompactedHistoryReferenceError> {
        if let RolloutItem::Compacted(compacted) = rollout_item {
            validate_compacted_history_references(&self.known_items, compacted)?;
        }
        self.index_explicit_sources(rollout_item);
        Ok(())
    }

    /// Indexes only source values carried directly by one rollout item.
    ///
    /// Unlike [`Self::index_item`], this does not validate or follow references in an entry-backed
    /// checkpoint. It is used while scanning an older prefix for possible suppliers: a dangling
    /// reference in an unrelated historical checkpoint must not prevent a newer selected
    /// checkpoint from resolving against independently complete inline sources.
    pub fn index_explicit_sources(&mut self, rollout_item: &RolloutItem) {
        match rollout_item {
            RolloutItem::ResponseItem(envelope) => {
                note_known_item(&mut self.known_items, envelope);
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    for envelope in replacement_history {
                        note_known_item(&mut self.known_items, envelope);
                    }
                } else if let Some(entries) = &compacted.replacement_history_entries {
                    for entry in entries {
                        if let CompactedHistoryEntry::Inline { item, metadata } = entry
                            && let Some(item_id) = item.id()
                        {
                            self.known_items.insert(
                                item_id.as_str().to_string(),
                                ResponseItemEnvelope {
                                    item: item.as_ref().clone(),
                                    metadata: metadata.clone(),
                                },
                            );
                        }
                    }
                }
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_) => {}
        }
    }

    /// Indexes direct source values only when their stable ID is currently demanded.
    ///
    /// This is the bounded form for multi-pass repair: callers can retain the union of IDs used by
    /// checkpoints that will actually be resolved, then scan arbitrarily large or malformed
    /// historical prefixes without cloning unrelated inline payloads. References are deliberately
    /// ignored and no checkpoint is validated by this source-carrier pass.
    pub fn index_explicit_sources_for_ids(
        &mut self,
        rollout_item: &RolloutItem,
        requested_item_ids: &HashSet<String>,
    ) {
        match rollout_item {
            RolloutItem::ResponseItem(envelope) => {
                note_requested_item(&mut self.known_items, requested_item_ids, envelope);
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    for envelope in replacement_history {
                        note_requested_item(&mut self.known_items, requested_item_ids, envelope);
                    }
                } else if let Some(entries) = &compacted.replacement_history_entries {
                    for entry in entries {
                        if let CompactedHistoryEntry::Inline { item, metadata } = entry {
                            let Some(item_id) =
                                item.id().map(codex_protocol::ResponseItemId::as_str)
                            else {
                                continue;
                            };
                            if requested_item_ids.contains(item_id) {
                                self.known_items.insert(
                                    item_id.to_string(),
                                    ResponseItemEnvelope {
                                        item: item.as_ref().clone(),
                                        metadata: metadata.clone(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_) => {}
        }
    }

    /// Resolves one checkpoint from the resolver's already-indexed older sources.
    ///
    /// The checkpoint and resolver are not changed. A history-less checkpoint returns `Ok(None)`.
    pub fn resolve_compacted_item(
        &self,
        compacted: &crate::CompactedItem,
    ) -> Result<Option<Vec<ResponseItemEnvelope>>, Vec<String>> {
        self.resolve_compacted_item_detailed(compacted)
            .map_err(|error| error.affected_item_ids())
    }

    /// Typed form of [`Self::resolve_compacted_item`] for integrity-aware callers.
    pub fn resolve_compacted_item_detailed(
        &self,
        compacted: &crate::CompactedItem,
    ) -> Result<Option<Vec<ResponseItemEnvelope>>, CompactedHistoryReferenceError> {
        resolve_compacted_history(&self.known_items, compacted)
    }

    /// Returns whether this exact response-item envelope is the source currently known by ID.
    ///
    /// Both the response item and its harness metadata must match. An envelope without a stable ID
    /// cannot be referenced and is therefore never known by this resolver.
    pub fn knows_exact_envelope(&self, envelope: &ResponseItemEnvelope) -> bool {
        envelope.item.id().is_some_and(|item_id| {
            self.known_items
                .get(item_id.as_str())
                .is_some_and(|known| known == envelope)
        })
    }

    /// Materializes one rollout item in place and indexes its complete response-item values.
    ///
    /// An error lists every reference in this checkpoint that has no older exact source. The
    /// checkpoint remains entry-backed and contributes no sources when that happens.
    /// A successfully resolved checkpoint rebases the resolver to that complete current context;
    /// subsequent top-level items form its suffix.
    pub fn materialize_item(
        &mut self,
        rollout_item: &mut RolloutItem,
    ) -> Result<(), Vec<String>> {
        self.materialize_item_detailed(rollout_item)
            .map_err(|error| error.affected_item_ids())
    }

    /// Typed form of [`Self::materialize_item`] for integrity-aware callers.
    pub fn materialize_item_detailed(
        &mut self,
        rollout_item: &mut RolloutItem,
    ) -> Result<(), CompactedHistoryReferenceError> {
        match rollout_item {
            RolloutItem::ResponseItem(envelope) => {
                note_known_item(&mut self.known_items, envelope);
            }
            RolloutItem::Compacted(compacted) => {
                let was_entry_backed = compacted.replacement_history_entries.is_some();
                let resolved = resolve_compacted_history(&self.known_items, compacted)?;

                if let Some(resolved) = resolved {
                    replace_known_items(&mut self.known_items, &resolved);
                    if was_entry_backed {
                        compacted.replacement_history = Some(resolved);
                        compacted.replacement_history_entries = None;
                    }
                }
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_) => {}
        }
        Ok(())
    }

    /// Re-encodes one rollout item using only exact sources from older rollout items.
    ///
    /// A checkpoint is first resolved without changing it. Envelopes whose stable ID and complete
    /// value exactly match an older source become references; all others remain inline. The entry
    /// representation is used only when that saves at least one duplicate. Sources from the
    /// checkpoint being encoded are indexed only after every entry has been classified, so a
    /// checkpoint can never reference itself.
    ///
    /// Missing references fail without changing the checkpoint or adding any of its inline values
    /// to the resolver. After a successful checkpoint, older values outside its complete history
    /// are released so the resolver remains bounded to the current context and its later suffix.
    pub fn reencode_item_with_backward_references(
        &mut self,
        rollout_item: &mut RolloutItem,
    ) -> Result<(), Vec<String>> {
        self.reencode_item_with_references(rollout_item, false)
            .map_err(|error| error.affected_item_ids())
    }

    /// Re-encodes exact older sources as V2 digest-bound references.
    pub fn reencode_item_with_integrity_references(
        &mut self,
        rollout_item: &mut RolloutItem,
    ) -> Result<(), CompactedHistoryReferenceError> {
        self.reencode_item_with_references(rollout_item, true)
    }

    fn reencode_item_with_references(
        &mut self,
        rollout_item: &mut RolloutItem,
        integrity_bound: bool,
    ) -> Result<(), CompactedHistoryReferenceError> {
        match rollout_item {
            RolloutItem::ResponseItem(envelope) => {
                note_known_item(&mut self.known_items, envelope);
            }
            RolloutItem::Compacted(compacted) => {
                let Some(resolved) = resolve_compacted_history(&self.known_items, compacted)?
                else {
                    return Ok(());
                };

                // One stable ID cannot address multiple ordered occurrences. If it appears more
                // than once in a checkpoint, retain every occurrence inline so a later
                // last-write-wins source lookup cannot silently substitute one value for another.
                let conflicting_item_ids = conflicting_history_item_ids(&resolved);
                let mut has_reference = false;
                let entries = resolved
                    .iter()
                    .cloned()
                    .map(|envelope| {
                        let exact_older_item_id = envelope.item.id().and_then(|item_id| {
                            if conflicting_item_ids.contains(item_id.as_str()) {
                                None
                            } else {
                                self.known_items
                                    .get(item_id.as_str())
                                    .filter(|known| *known == &envelope)
                                    .map(|_| item_id.as_str().to_string())
                            }
                        });

                        if let Some(item_id) = exact_older_item_id {
                            if integrity_bound {
                                match CompactedHistoryEntry::reference_v2(item_id, &envelope) {
                                    Ok(reference) => {
                                        has_reference = true;
                                        reference
                                    }
                                    // Durable-projection failure cannot justify a weaker reference.
                                    // Keep the complete envelope inline instead.
                                    Err(_) => CompactedHistoryEntry::from(envelope),
                                }
                            } else {
                                has_reference = true;
                                CompactedHistoryEntry::Reference { item_id }
                            }
                        } else {
                            CompactedHistoryEntry::from(envelope)
                        }
                    })
                    .collect();

                if has_reference {
                    compacted.replacement_history = None;
                    compacted.replacement_history_entries = Some(entries);
                } else {
                    compacted.replacement_history = Some(resolved.clone());
                    compacted.replacement_history_entries = None;
                }

                replace_known_items(&mut self.known_items, &resolved);
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_) => {}
        }
        Ok(())
    }
}

/// Resolves the checkpoint at `checkpoint_index` using only sources from older rollout items.
///
/// The prefix is scanned for top-level response items, legacy full histories, and inline entry
/// values without materializing or changing any historical checkpoint. References in older
/// checkpoints are irrelevant to source discovery and are not followed or validated. The selected
/// target remains strict. An index that does not identify a checkpoint, or a checkpoint without
/// replacement history, returns `Ok(None)`.
pub fn resolve_checkpoint_at(
    rollout_items: &[RolloutItem],
    checkpoint_index: usize,
) -> Result<Option<Vec<ResponseItemEnvelope>>, Vec<String>> {
    resolve_checkpoint_at_detailed(rollout_items, checkpoint_index)
        .map_err(|error| error.affected_item_ids())
}

/// Typed form of [`resolve_checkpoint_at`] for integrity-aware callers.
pub fn resolve_checkpoint_at_detailed(
    rollout_items: &[RolloutItem],
    checkpoint_index: usize,
) -> Result<Option<Vec<ResponseItemEnvelope>>, CompactedHistoryReferenceError> {
    let Some(RolloutItem::Compacted(compacted)) = rollout_items.get(checkpoint_index) else {
        return Ok(None);
    };

    if let Some(replacement_history) = &compacted.replacement_history {
        return Ok(Some(replacement_history.clone()));
    }
    let Some(entries) = &compacted.replacement_history_entries else {
        return Ok(None);
    };

    let requested_item_ids = entries
        .iter()
        .filter_map(|entry| match entry {
            CompactedHistoryEntry::Reference { item_id }
            | CompactedHistoryEntry::ReferenceV2 { item_id, .. } => Some(item_id.clone()),
            CompactedHistoryEntry::Inline { .. } => None,
        })
        .collect::<HashSet<_>>();
    let known_items =
        latest_explicit_sources_for_ids(&rollout_items[..checkpoint_index], &requested_item_ids);
    resolve_compacted_history(&known_items, compacted)
}

/// Resolves reference-backed checkpoints into their complete inline history shape.
///
/// References are backward-only: a source must have appeared in a top-level response item or an
/// older complete checkpoint. The persistent rollout is never changed; callers receive a borrowed
/// slice when no work is needed and an owned temporary view otherwise.
pub fn materialize_compacted_histories(
    rollout_items: &[RolloutItem],
) -> MaterializedCompactedHistories<'_> {
    if !rollout_items.iter().any(|item| {
        matches!(
            item,
            RolloutItem::Compacted(compacted) if compacted.replacement_history_entries.is_some()
        )
    }) {
        return MaterializedCompactedHistories {
            rollout_items: Cow::Borrowed(rollout_items),
            unresolved_item_ids: Vec::new(),
        };
    }

    let mut materialized = rollout_items.to_vec();
    let mut resolver = CompactedHistoryResolver::default();
    let mut unresolved_item_ids = Vec::new();
    for rollout_item in &mut materialized {
        if let Err(missing) = resolver.materialize_item(rollout_item) {
            unresolved_item_ids.extend(missing);
        }
    }

    unresolved_item_ids.sort_unstable();
    unresolved_item_ids.dedup();
    MaterializedCompactedHistories {
        rollout_items: Cow::Owned(materialized),
        unresolved_item_ids,
    }
}

fn note_known_item(
    known_items: &mut HashMap<String, ResponseItemEnvelope>,
    envelope: &ResponseItemEnvelope,
) {
    if let Some(item_id) = envelope.item.id() {
        known_items.insert(item_id.as_str().to_string(), envelope.clone());
    }
}

fn replace_known_items(
    known_items: &mut HashMap<String, ResponseItemEnvelope>,
    replacement_history: &[ResponseItemEnvelope],
) {
    known_items.clear();
    for envelope in replacement_history {
        note_known_item(known_items, envelope);
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

/// Collects only the newest explicit source for each requested ID.
///
/// The scan runs newest-to-oldest and visits checkpoint contents in reverse storage order. This is
/// equivalent to chronological last-write-wins indexing, but it can stop as soon as every
/// requested ID has been found and never clones an unrelated envelope.
fn latest_explicit_sources_for_ids(
    rollout_items: &[RolloutItem],
    requested_item_ids: &HashSet<String>,
) -> HashMap<String, ResponseItemEnvelope> {
    let mut sources = HashMap::with_capacity(requested_item_ids.len());
    if requested_item_ids.is_empty() {
        return sources;
    }

    for rollout_item in rollout_items.iter().rev() {
        match rollout_item {
            RolloutItem::ResponseItem(envelope) => {
                note_requested_item(&mut sources, requested_item_ids, envelope);
            }
            RolloutItem::Compacted(compacted) => {
                if let Some(replacement_history) = &compacted.replacement_history {
                    for envelope in replacement_history.iter().rev() {
                        note_requested_item(&mut sources, requested_item_ids, envelope);
                    }
                } else if let Some(entries) = &compacted.replacement_history_entries {
                    for entry in entries.iter().rev() {
                        if let CompactedHistoryEntry::Inline { item, metadata } = entry {
                            let Some(item_id) =
                                item.id().map(codex_protocol::ResponseItemId::as_str)
                            else {
                                continue;
                            };
                            if requested_item_ids.contains(item_id)
                                && !sources.contains_key(item_id)
                            {
                                sources.insert(
                                    item_id.to_string(),
                                    ResponseItemEnvelope {
                                        item: item.as_ref().clone(),
                                        metadata: metadata.clone(),
                                    },
                                );
                            }
                        }
                    }
                }
            }
            RolloutItem::SessionMeta(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::TurnContext(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::EventMsg(_)
            | RolloutItem::RealtimeItem(_) => {}
        }

        if sources.len() == requested_item_ids.len() {
            break;
        }
    }
    sources
}

fn note_requested_item(
    sources: &mut HashMap<String, ResponseItemEnvelope>,
    requested_item_ids: &HashSet<String>,
    envelope: &ResponseItemEnvelope,
) {
    let Some(item_id) = envelope
        .item
        .id()
        .map(codex_protocol::ResponseItemId::as_str)
    else {
        return;
    };
    if requested_item_ids.contains(item_id) && !sources.contains_key(item_id) {
        sources.insert(item_id.to_string(), envelope.clone());
    }
}

fn resolve_compacted_history(
    known_items: &HashMap<String, ResponseItemEnvelope>,
    compacted: &crate::CompactedItem,
) -> Result<Option<Vec<ResponseItemEnvelope>>, CompactedHistoryReferenceError> {
    if let Some(replacement_history) = &compacted.replacement_history {
        return Ok(Some(replacement_history.clone()));
    }
    let Some(entries) = &compacted.replacement_history_entries else {
        return Ok(None);
    };

    let mut history = Vec::with_capacity(entries.len());
    let mut errors = CompactedHistoryReferenceError::empty();
    for entry in entries {
        match entry {
            CompactedHistoryEntry::Inline { item, metadata } => {
                history.push(ResponseItemEnvelope {
                    item: item.as_ref().clone(),
                    metadata: metadata.clone(),
                });
            }
            CompactedHistoryEntry::Reference { item_id } => {
                if let Some(envelope) = known_items.get(item_id) {
                    history.push(envelope.clone());
                } else {
                    errors.missing_item_ids.push(item_id.clone());
                }
            }
            CompactedHistoryEntry::ReferenceV2 {
                item_id,
                source_digest,
            } => match known_items.get(item_id) {
                None => errors.missing_item_ids.push(item_id.clone()),
                Some(envelope) => match CompactedHistoryDigest::from_envelope(envelope) {
                    Ok(actual) if actual == *source_digest => history.push(envelope.clone()),
                    Ok(actual) => errors
                        .digest_mismatches
                        .push(CompactedHistoryDigestMismatch {
                            item_id: item_id.clone(),
                            expected: source_digest.clone(),
                            actual,
                        }),
                    Err(_) => errors
                        .digest_encoding_failure_item_ids
                        .push(item_id.clone()),
                },
            },
        }
    }

    if errors.is_empty() {
        Ok(Some(history))
    } else {
        Err(errors.normalized())
    }
}

fn validate_compacted_history_references(
    known_items: &HashMap<String, ResponseItemEnvelope>,
    compacted: &crate::CompactedItem,
) -> Result<(), CompactedHistoryReferenceError> {
    let Some(entries) = &compacted.replacement_history_entries else {
        return Ok(());
    };

    let mut errors = CompactedHistoryReferenceError::empty();
    for entry in entries {
        match entry {
            CompactedHistoryEntry::Reference { item_id } => {
                if !known_items.contains_key(item_id) {
                    errors.missing_item_ids.push(item_id.clone());
                }
            }
            CompactedHistoryEntry::ReferenceV2 {
                item_id,
                source_digest,
            } => match known_items.get(item_id) {
                None => errors.missing_item_ids.push(item_id.clone()),
                Some(envelope) => match CompactedHistoryDigest::from_envelope(envelope) {
                    Ok(actual) if actual == *source_digest => {}
                    Ok(actual) => errors
                        .digest_mismatches
                        .push(CompactedHistoryDigestMismatch {
                            item_id: item_id.clone(),
                            expected: source_digest.clone(),
                            actual,
                        }),
                    Err(_) => errors
                        .digest_encoding_failure_item_ids
                        .push(item_id.clone()),
                },
            },
            CompactedHistoryEntry::Inline { .. } => {}
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.normalized())
    }
}
