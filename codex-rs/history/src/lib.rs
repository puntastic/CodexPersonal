//! Model-history and persisted-rollout domain types.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::ops::DerefMut;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use codex_protocol::ThreadId;
use codex_protocol::capabilities::SelectedCapabilityRoot;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::mcp::McpResourceOriginCheckpoint;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadSource;
use codex_protocol::protocol::TurnContextItem;
use codex_protocol::protocol::WorldStateItem;
use codex_protocol::realtime::RealtimeItem;
use codex_protocol::security_risk::SecurityRiskScore;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::Error as _;
use sha2::Digest as _;
use sha2::Sha256;

mod compacted_history;
pub use compacted_history::CompactedHistoryDigestMismatch;
pub use compacted_history::CompactedHistoryReferenceError;
pub use compacted_history::CompactedHistoryResolver;
pub use compacted_history::MaterializedCompactedHistories;
pub use compacted_history::materialize_compacted_histories;
pub use compacted_history::resolve_checkpoint_at;
pub use compacted_history::resolve_checkpoint_at_detailed;

/// A model-history item with room for history-only metadata.
///
/// Persistence keeps the response item intact and stores its metadata separately.
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseItemEnvelope {
    pub item: ResponseItem,
    pub metadata: Option<CodexHarnessMetadata>,
}

/// Metadata owned by the Codex harness and persisted with a response item.
///
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
pub struct CodexHarnessMetadata {
    /// Whether a developer message was supplied by an app-server client.
    #[serde(default)]
    pub client_authored: bool,
}

const COMPACTED_HISTORY_DIGEST_PREFIX: &str = "sha256-response-item-envelope-v1:";

/// A versioned digest over the complete durable response-item envelope.
///
/// The digest includes the response item and harness-owned metadata exactly as they survive a
/// persistence round trip. Host-only passthrough fields that intentionally skip deserialization
/// are excluded so a legitimate cold reload cannot invalidate its own reference. The versioned
/// prefix makes this durable projection and canonical encoding part of the persisted contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, JsonSchema)]
#[schemars(transparent)]
pub struct CompactedHistoryDigest(String);

impl CompactedHistoryDigest {
    /// Computes the V2 compacted-reference digest for an envelope.
    pub fn from_envelope(
        envelope: &ResponseItemEnvelope,
    ) -> Result<Self, CompactedHistoryDigestEncodingError> {
        // Project through the actual rollout wire before hashing. Response items contain a small
        // set of trusted host-only passthrough fields that serialize for provider use but
        // deliberately skip deserialization at the persistence trust boundary.
        let serialized = serde_json::to_value(RolloutItem::ResponseItem(envelope.clone()))
            .map_err(CompactedHistoryDigestEncodingError::from)?;
        let durable = serde_json::from_value::<RolloutItem>(serialized)
            .map_err(CompactedHistoryDigestEncodingError::from)?;
        let mut value =
            serde_json::to_value(durable).map_err(CompactedHistoryDigestEncodingError::from)?;
        canonicalize_json_value(&mut value);
        let bytes =
            serde_json::to_vec(&value).map_err(CompactedHistoryDigestEncodingError::from)?;
        let hash = Sha256::digest(bytes);
        Ok(Self(format!("{COMPACTED_HISTORY_DIGEST_PREFIX}{hash:x}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CompactedHistoryDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for CompactedHistoryDigest {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some(hex) = value.strip_prefix(COMPACTED_HISTORY_DIGEST_PREFIX) else {
            return Err(format!(
                "compacted history digest must start with `{COMPACTED_HISTORY_DIGEST_PREFIX}`"
            ));
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(
                "compacted history digest must contain exactly 64 lowercase hexadecimal characters"
                    .to_string(),
            );
        }
        Ok(Self(value.to_string()))
    }
}

impl Serialize for CompactedHistoryDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CompactedHistoryDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

/// Failure to encode an otherwise in-memory envelope for a V2 integrity reference.
#[derive(Debug)]
pub struct CompactedHistoryDigestEncodingError(serde_json::Error);

impl fmt::Display for CompactedHistoryDigestEncodingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not encode response-item envelope: {}",
            self.0
        )
    }
}

impl std::error::Error for CompactedHistoryDigestEncodingError {}

impl From<serde_json::Error> for CompactedHistoryDigestEncodingError {
    fn from(error: serde_json::Error) -> Self {
        Self(error)
    }
}

fn canonicalize_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                canonicalize_json_value(item);
            }
        }
        serde_json::Value::Object(object) => {
            for child in object.values_mut() {
                canonicalize_json_value(child);
            }
            let mut entries = std::mem::take(object).into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
            object.extend(entries);
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

impl ResponseItemEnvelope {
    /// Wraps a raw Responses API item for persisted history.
    pub fn new(item: ResponseItem) -> Self {
        Self {
            item,
            metadata: None,
        }
    }

    /// Unwraps the raw Responses API item.
    pub fn into_item(self) -> ResponseItem {
        self.item
    }
}

impl From<ResponseItem> for ResponseItemEnvelope {
    fn from(item: ResponseItem) -> Self {
        Self::new(item)
    }
}

impl Deref for ResponseItemEnvelope {
    type Target = ResponseItem;

    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl DerefMut for ResponseItemEnvelope {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

impl Borrow<ResponseItem> for ResponseItemEnvelope {
    fn borrow(&self) -> &ResponseItem {
        &self.item
    }
}

/// Persisted rollout item used by core history and rollout storage.
#[derive(Debug, Clone)]
pub enum RolloutItem {
    SessionMeta(SessionMetaLine),
    ResponseItem(ResponseItemEnvelope),
    InterAgentCommunication(InterAgentCommunication),
    InterAgentCommunicationMetadata {
        trigger_turn: bool,
    },
    Compacted(CompactedItem),
    TurnContext(TurnContextItem),
    WorldState(WorldStateItem),
    SecurityRiskScore(SecurityRiskScore),
    EventMsg(EventMsg),
    /// Sparse, model-invisible facts used to reconstruct realtime presentation.
    RealtimeItem(RealtimeItem),
}

impl Serialize for RolloutItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        rollout_payload::RolloutItemWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RolloutItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        rollout_payload::RolloutItemWire::deserialize(deserializer).map(Into::into)
    }
}

impl JsonSchema for RolloutItem {
    fn schema_name() -> String {
        "RolloutItem".to_string()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed(concat!(module_path!(), "::RolloutItem"))
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        rollout_payload::RolloutItemWire::json_schema(generator)
    }
}

mod rollout_payload;

/// A replacement-history entry stored inline or resolved by a persisted item ID.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CompactedHistoryEntry {
    Inline {
        item: Box<ResponseItem>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<CodexHarnessMetadata>,
    },
    Reference {
        item_id: String,
    },
    /// A V2 backward reference that binds the complete source envelope, including metadata.
    ReferenceV2 {
        item_id: String,
        source_digest: CompactedHistoryDigest,
    },
}

impl CompactedHistoryEntry {
    /// Returns the response-item envelope when this entry stores its item inline.
    pub fn into_inline(self) -> Option<ResponseItemEnvelope> {
        match self {
            Self::Inline { item, metadata } => Some(ResponseItemEnvelope {
                item: *item,
                metadata,
            }),
            Self::Reference { .. } | Self::ReferenceV2 { .. } => None,
        }
    }

    /// Creates a V2 reference bound to the supplied envelope's complete durable projection.
    pub fn reference_v2(
        item_id: String,
        envelope: &ResponseItemEnvelope,
    ) -> Result<Self, CompactedHistoryDigestEncodingError> {
        Ok(Self::ReferenceV2 {
            item_id,
            source_digest: CompactedHistoryDigest::from_envelope(envelope)?,
        })
    }
}

impl From<ResponseItemEnvelope> for CompactedHistoryEntry {
    fn from(envelope: ResponseItemEnvelope) -> Self {
        Self::Inline {
            item: Box::new(envelope.item),
            metadata: envelope.metadata,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompactedItem {
    pub message: String,
    pub replacement_history: Option<Vec<ResponseItemEnvelope>>,
    pub replacement_history_entries: Option<Vec<CompactedHistoryEntry>>,
    pub mcp_resource_origins: Option<McpResourceOriginCheckpoint>,
    pub window_number: Option<u64>,
    pub first_window_id: Option<String>,
    pub previous_window_id: Option<String>,
    pub window_id: Option<String>,
}

impl Serialize for CompactedItem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        rollout_payload::CompactedItemWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CompactedItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        rollout_payload::CompactedItemWire::deserialize(deserializer)?
            .try_into()
            .map_err(D::Error::custom)
    }
}

impl JsonSchema for CompactedItem {
    fn schema_name() -> String {
        "CompactedItem".to_string()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed(concat!(module_path!(), "::CompactedItem"))
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::schema::Schema {
        rollout_payload::CompactedItemWire::json_schema(generator)
    }
}

impl From<CompactedItem> for ResponseItem {
    fn from(value: CompactedItem) -> Self {
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: value.message,
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, JsonSchema)]
pub struct RolloutLine {
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u64>,
    #[serde(flatten)]
    pub item: RolloutItem,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResumedHistory {
    pub conversation_id: ThreadId,
    pub history: Arc<Vec<RolloutItem>>,
    pub rollout_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum InitialHistory {
    New,
    Cleared,
    Resumed(ResumedHistory),
    Forked(Vec<RolloutItem>),
}

impl InitialHistory {
    pub fn scan_rollout_items(&self, mut predicate: impl FnMut(&RolloutItem) -> bool) -> bool {
        match self {
            Self::New | Self::Cleared => false,
            Self::Resumed(resumed) => resumed.history.iter().any(&mut predicate),
            Self::Forked(items) => items.iter().any(predicate),
        }
    }

    pub fn forked_from_id(&self) -> Option<ThreadId> {
        match self {
            Self::New | Self::Cleared => None,
            Self::Resumed(resumed) => resumed.history.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => meta_line.meta.forked_from_id,
                _ => None,
            }),
            Self::Forked(items) => items.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => Some(meta_line.meta.id),
                _ => None,
            }),
        }
    }

    pub fn session_cwd(&self) -> Option<PathBuf> {
        match self {
            Self::New | Self::Cleared => None,
            Self::Resumed(resumed) => session_cwd_from_items(&resumed.history),
            Self::Forked(items) => session_cwd_from_items(items),
        }
    }

    pub fn get_rollout_items(&self) -> &[RolloutItem] {
        match self {
            Self::New | Self::Cleared => &[],
            Self::Resumed(resumed) => &resumed.history,
            Self::Forked(items) => items,
        }
    }

    pub fn get_event_msgs(&self) -> Option<Vec<EventMsg>> {
        match self {
            Self::New | Self::Cleared => None,
            Self::Resumed(resumed) => Some(
                resumed
                    .history
                    .iter()
                    .filter_map(|item| match item {
                        RolloutItem::EventMsg(event) => Some(event.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            Self::Forked(items) => Some(
                items
                    .iter()
                    .filter_map(|item| match item {
                        RolloutItem::EventMsg(event) => Some(event.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
        }
    }

    pub fn get_base_instructions(&self) -> Option<BaseInstructions> {
        match self {
            Self::New | Self::Cleared => None,
            Self::Resumed(resumed) => resumed.history.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => meta_line.meta.base_instructions.clone(),
                _ => None,
            }),
            Self::Forked(items) => items.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => meta_line.meta.base_instructions.clone(),
                _ => None,
            }),
        }
    }

    pub fn get_dynamic_tools(&self) -> Option<Vec<DynamicToolSpec>> {
        match self {
            Self::New | Self::Cleared => None,
            Self::Resumed(resumed) => resumed.history.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => meta_line.meta.dynamic_tools.clone(),
                _ => None,
            }),
            Self::Forked(items) => items.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => meta_line.meta.dynamic_tools.clone(),
                _ => None,
            }),
        }
    }

    pub fn get_selected_capability_roots(&self) -> Vec<SelectedCapabilityRoot> {
        self.get_session_meta()
            .map(|meta| meta.selected_capability_roots.clone())
            .unwrap_or_default()
    }

    pub fn get_multi_agent_version(&self) -> Option<MultiAgentVersion> {
        match self {
            Self::New | Self::Cleared => None,
            Self::Resumed(resumed) => {
                multi_agent_version_from_items(&resumed.history, Some(resumed.conversation_id))
            }
            Self::Forked(items) => multi_agent_version_from_items(items, /*thread_id*/ None),
        }
    }

    pub fn get_history_mode(&self, default_history_mode: ThreadHistoryMode) -> ThreadHistoryMode {
        match self {
            Self::New | Self::Cleared => default_history_mode,
            Self::Resumed(_) | Self::Forked(_) => self
                .get_session_meta()
                .map(|meta| meta.history_mode)
                .unwrap_or(default_history_mode),
        }
    }

    pub fn get_resumed_session_sources(&self) -> Option<(SessionSource, Option<ThreadSource>)> {
        let meta = self.get_resumed_session_meta()?;
        Some((meta.source.clone(), meta.thread_source.clone()))
    }

    pub fn get_resumed_thread_source(&self) -> Option<ThreadSource> {
        self.get_resumed_session_meta()
            .and_then(|meta| meta.thread_source.clone())
    }

    pub fn get_session_originator(&self) -> Option<String> {
        self.get_session_meta()
            .map(|meta| meta.originator.clone())
            .filter(|originator| !originator.is_empty())
    }

    pub fn get_resumed_parent_thread_id(&self) -> Option<ThreadId> {
        self.get_resumed_session_meta()
            .and_then(|meta| meta.parent_thread_id)
    }

    fn get_session_meta(&self) -> Option<&SessionMeta> {
        match self {
            Self::New | Self::Cleared => None,
            Self::Resumed(resumed) => resumed.history.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => Some(&meta_line.meta),
                _ => None,
            }),
            Self::Forked(items) => items.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => Some(&meta_line.meta),
                _ => None,
            }),
        }
    }

    fn get_resumed_session_meta(&self) -> Option<&SessionMeta> {
        match self {
            Self::New | Self::Cleared | Self::Forked(_) => None,
            Self::Resumed(resumed) => resumed.history.iter().find_map(|item| match item {
                RolloutItem::SessionMeta(meta_line) => Some(&meta_line.meta),
                _ => None,
            }),
        }
    }
}

fn session_cwd_from_items(items: &[RolloutItem]) -> Option<PathBuf> {
    items.iter().find_map(|item| match item {
        RolloutItem::SessionMeta(meta_line) => Some(meta_line.meta.cwd.clone()),
        _ => None,
    })
}

fn multi_agent_version_from_items(
    items: &[RolloutItem],
    thread_id: Option<ThreadId>,
) -> Option<MultiAgentVersion> {
    let session_meta_version = items.iter().rev().find_map(|item| match item {
        RolloutItem::SessionMeta(meta_line)
            if thread_id.is_none_or(|thread_id| meta_line.meta.id == thread_id) =>
        {
            meta_line.meta.multi_agent_version
        }
        _ => None,
    });

    session_meta_version.or_else(|| {
        items.iter().rev().find_map(|item| match item {
            RolloutItem::TurnContext(turn_context) => turn_context.multi_agent_version,
            RolloutItem::SessionMeta(_)
            | RolloutItem::ResponseItem(_)
            | RolloutItem::InterAgentCommunication(_)
            | RolloutItem::InterAgentCommunicationMetadata { .. }
            | RolloutItem::Compacted(_)
            | RolloutItem::WorldState(_)
            | RolloutItem::SecurityRiskScore(_)
            | RolloutItem::RealtimeItem(_)
            | RolloutItem::EventMsg(_) => None,
        })
    })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
