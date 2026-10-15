use crate::JsonSchema;
use crate::TS;
use serde::Deserialize;
use serde::Serialize;

/// Experimental bounded DATA for an existing live regular turn. This never
/// starts a turn, steers with user input, or falls back to history insertion.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export_to = "v2/")]
pub struct TurnAdmitDataParams {
    pub thread_id: String,
    /// Required active-turn precondition: nonempty, at most 128 UTF-8 bytes.
    pub expected_turn_id: String,
    /// Nonempty, at most 128 UTF-8 bytes. Deduplicated only for the lifetime of
    /// this live turn state, including after drain; not across restart/recovery.
    pub delivery_id: String,
    /// Nonempty DATA, at most 512 UTF-8 bytes (enforced at runtime, not by JSON
    /// string length). The server owns the standalone helper_result/codex_data
    /// tool-result shape; clients cannot supply role, call ID, or provenance.
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum TurnAdmitDataStatus {
    Queued,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnAdmitDataResponse {
    /// Admission only: not prepared, delivered, sampled, or understood.
    pub status: TurnAdmitDataStatus,
    /// The active turn identity checked during admission.
    pub turn_id: String,
    pub delivery_id: String,
}
