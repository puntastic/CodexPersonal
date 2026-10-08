//! Model-invisible Guardian transcript checkpoints and retained-section delivery proof.
//! Entries preserve rollback provenance; their flattened wire shape still reads as
//! ResponseItem on older hosts, and old metadata-free checkpoints remain readable.

use std::borrow::Cow;

use codex_protocol::models::ResponseItem;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

use crate::CompactedHistoryEntry;

use crate::CodexHarnessMetadata;
use crate::ResponseItemEnvelope;

/// Host omission notices delivered by a complete retained-instructions section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GuardianRetainedOmissions {
    pub user_instructions: bool,
    pub assistant_context: bool,
}

/// Original review evidence, separate from the compacted model conversation.
///
/// The public first field is the materialized, review-visible item sequence used by existing
/// hosts. Reference-capable writers may instead persist `entries`; readers must resolve those
/// entries against older rollout sources before handing the checkpoint to a Guardian consumer.
/// Old array-shaped inline checkpoints remain the compatibility representation.
#[derive(Clone, PartialEq)]
pub struct GuardianHistoryCheckpointData(
    pub Vec<ResponseItemEnvelope>,
    Option<Vec<CompactedHistoryEntry>>,
);

#[derive(Serialize, Deserialize, JsonSchema)]
struct Entry<'a> {
    #[serde(flatten)]
    item: Cow<'a, ResponseItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guardian_metadata: Option<Cow<'a, CodexHarnessMetadata>>,
}

/// Compatibility type name retained for existing hosts and tests.
pub type GuardianHistoryCheckpoint = GuardianHistoryCheckpointData;

/// Constructs an inline Guardian checkpoint using the historical tuple-constructor spelling.
#[allow(non_snake_case)]
pub fn GuardianHistoryCheckpoint(items: Vec<ResponseItemEnvelope>) -> GuardianHistoryCheckpoint {
    GuardianHistoryCheckpointData(items, None)
}

impl GuardianHistoryCheckpointData {
    /// Creates a checkpoint whose ordered items must be resolved from older rollout sources.
    pub fn from_entries(entries: Vec<CompactedHistoryEntry>) -> Self {
        Self(Vec::new(), Some(entries))
    }

    /// Returns the persisted entry representation, when this checkpoint is reference-backed.
    pub fn entries(&self) -> Option<&[CompactedHistoryEntry]> {
        self.1.as_deref()
    }

    /// Returns whether this checkpoint requires source resolution before Guardian use.
    pub fn is_reference_backed(&self) -> bool {
        self.1.is_some()
    }
}

impl std::fmt::Debug for GuardianHistoryCheckpointData {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuardianHistoryCheckpoint")
            .field("items", &self.0.len())
            .field("entries", &self.1.as_ref().map(Vec::len))
            .finish()
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
enum GuardianHistoryCheckpointWire<'a> {
    Inline(Vec<Entry<'a>>),
    ReferenceBacked { entries: Vec<CompactedHistoryEntry> },
}

impl Serialize for GuardianHistoryCheckpointData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.1 {
            Some(entries) => GuardianHistoryCheckpointWire::ReferenceBacked {
                entries: entries.clone(),
            }
            .serialize(serializer),
            None => serializer.collect_seq(self.0.iter().map(|entry| Entry {
                item: Cow::Borrowed(&entry.item),
                guardian_metadata: entry.metadata.as_ref().map(Cow::Borrowed),
            })),
        }
    }
}

impl<'de> Deserialize<'de> for GuardianHistoryCheckpointData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match GuardianHistoryCheckpointWire::deserialize(deserializer)? {
            GuardianHistoryCheckpointWire::Inline(items) => Ok(Self(
                items
                    .into_iter()
                    .map(|entry| ResponseItemEnvelope {
                        item: entry.item.into_owned(),
                        metadata: entry.guardian_metadata.map(Cow::into_owned),
                    })
                    .collect(),
                None,
            )),
            GuardianHistoryCheckpointWire::ReferenceBacked { entries } => {
                Ok(Self::from_entries(entries))
            }
        }
    }
}

impl JsonSchema for GuardianHistoryCheckpointData {
    fn schema_name() -> String {
        "GuardianHistoryCheckpoint".to_string()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed(concat!(module_path!(), "::GuardianHistoryCheckpoint"))
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        GuardianHistoryCheckpointWire::json_schema(generator)
    }
}
