//! Model-invisible checkpoint of the host's bounded Guardian transcript.

use codex_protocol::models::ResponseItem;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

use crate::CompactedHistoryEntry;

/// Original review evidence, separate from the compacted model conversation.
///
/// The public first field is the materialized, review-visible item sequence used by existing
/// hosts. Reference-capable writers may instead persist `entries`; readers must resolve those
/// entries against older rollout sources before handing the checkpoint to a Guardian consumer.
/// Old array-shaped inline checkpoints remain the compatibility representation.
#[derive(Clone, PartialEq)]
pub struct GuardianHistoryCheckpointData(pub Vec<ResponseItem>, Option<Vec<CompactedHistoryEntry>>);

/// Compatibility type name retained for existing hosts and tests.
pub type GuardianHistoryCheckpoint = GuardianHistoryCheckpointData;

/// Constructs an inline Guardian checkpoint using the historical tuple-constructor spelling.
#[allow(non_snake_case)]
pub fn GuardianHistoryCheckpoint(items: Vec<ResponseItem>) -> GuardianHistoryCheckpoint {
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
enum GuardianHistoryCheckpointWire {
    Inline(Vec<ResponseItem>),
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
            None => GuardianHistoryCheckpointWire::Inline(self.0.clone()).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for GuardianHistoryCheckpointData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match GuardianHistoryCheckpointWire::deserialize(deserializer)? {
            GuardianHistoryCheckpointWire::Inline(items) => Ok(Self(items, None)),
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
