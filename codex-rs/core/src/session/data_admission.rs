//! Experimental, turn-bound helper DATA. This is a standalone tool result, not
//! a contextual-user fragment or a user steering message.

use super::TurnInput;
use super::session::Session;
use crate::codex_thread::CodexThread;
use crate::state::TaskKind;
use codex_history::ResponseItemEnvelope;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use std::collections::HashSet;

// Final text includes caller wrappers and qualifications; normal tool-output
// truncation still applies after admission. This is not a token-limit proof.
const MAX_DATA_TEXT_BYTES: usize = 4096;

/// The admission receipt deliberately does not claim delivery.
#[derive(Debug, PartialEq, Eq)]
pub enum DataAdmission {
    /// Queued for this turn; not evidence of a request, delivery, or comprehension.
    Queued,
}

/// Refusals leave both history and pending input unchanged.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DataAdmissionError {
    #[error(
        "DATA requires nonempty IDs of at most 128 bytes and nonempty text of at most {max} bytes",
        max = MAX_DATA_TEXT_BYTES
    )]
    InvalidInput,
    #[error("no active task")]
    NoActiveTurn,
    #[error("expected turn does not match the active task")]
    ExpectedTurnMismatch,
    #[error("active task is not a regular turn")]
    NotRegular,
    #[error("turn DATA admission is closed")]
    Closed,
    #[error("turn is cancelled")]
    Cancelled,
    #[error("delivery ID was already admitted in this turn")]
    Duplicate,
    #[error("turn DATA admission limit reached")]
    LimitReached,
}

/// Small proof-slice limits, not a general production sizing recommendation.
#[derive(Default)]
pub(crate) struct DataAdmissionState {
    closed: bool,
    delivery_ids: HashSet<String>,
    pub(crate) pending: Vec<ResponseItem>,
}

impl DataAdmissionState {
    pub(crate) fn close(&mut self) {
        self.closed = true;
        self.pending.clear();
    }
}

impl CodexThread {
    /// Admit bounded standalone helper DATA only to the named live regular turn.
    /// Never starts a turn, falls back to history, steers, or reopens mailbox delivery.
    /// IDs deduplicate for this live turn state, even after consumption, not across
    /// recovery. Pending DATA is discarded on abort or terminal cleanup. Once drained,
    /// ordinary history recording can precede transport; queued does not mean sampled.
    /// The final text limit includes wrappers; ordinary tool-output policy may truncate it.
    pub async fn admit_data(
        &self,
        expected_turn_id: &str,
        delivery_id: &str,
        text: &str,
    ) -> Result<DataAdmission, DataAdmissionError> {
        self.session
            .admit_data(expected_turn_id, delivery_id, text)
            .await
    }
}

impl Session {
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "only the owning turn may close and discard its pending DATA"
    )]
    pub(crate) async fn discard_pending_data(&self, turn_id: &str) {
        let active = self.active_turn.lock().await;
        if let Some(turn) = active.as_ref().filter(|turn| {
            turn.task
                .as_ref()
                .is_some_and(|task| task.turn_context.sub_id == turn_id)
        }) {
            turn.turn_state.lock().await.data_admission.close();
        }
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "a stale consumer must not drain DATA belonging to a replacement task"
    )]
    pub(crate) async fn take_pending_data(&self, turn_id: &str) -> Vec<TurnInput> {
        let active = self.active_turn.lock().await;
        let Some(turn) = active.as_ref().filter(|turn| {
            turn.task.as_ref().is_some_and(|task| {
                task.turn_context.sub_id == turn_id
                    && task.kind == TaskKind::Regular
                    && !task.cancellation_token.is_cancelled()
            })
        }) else {
            return Vec::new();
        };
        let pending = std::mem::take(&mut turn.turn_state.lock().await.data_admission.pending);
        pending
            .into_iter()
            .map(ResponseItemEnvelope::new)
            .map(TurnInput::FunctionCallOutput)
            .collect()
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "identity, cancellation and admission must be atomic with task completion"
    )]
    pub(crate) async fn admit_data(
        &self,
        expected_turn_id: &str,
        delivery_id: &str,
        text: &str,
    ) -> Result<DataAdmission, DataAdmissionError> {
        if expected_turn_id.is_empty()
            || expected_turn_id.len() > 128
            || delivery_id.is_empty()
            || delivery_id.len() > 128
            || text.is_empty()
            || text.len() > MAX_DATA_TEXT_BYTES
        {
            return Err(DataAdmissionError::InvalidInput);
        }
        let active = self.active_turn.lock().await;
        let turn = active.as_ref().ok_or(DataAdmissionError::NoActiveTurn)?;
        let task = turn.task.as_ref().ok_or(DataAdmissionError::NoActiveTurn)?;
        if task.turn_context.sub_id != expected_turn_id {
            return Err(DataAdmissionError::ExpectedTurnMismatch);
        }
        if task.kind != TaskKind::Regular {
            return Err(DataAdmissionError::NotRegular);
        }
        if task.cancellation_token.is_cancelled() {
            return Err(DataAdmissionError::Cancelled);
        }
        let mut state = turn.turn_state.lock().await;
        let data = &mut state.data_admission;
        if data.closed || task.turn_context.terminal_error.lock().await.is_some() {
            return Err(DataAdmissionError::Closed);
        }
        if data.delivery_ids.contains(delivery_id) {
            return Err(DataAdmissionError::Duplicate);
        }
        // IDs are never evicted, including after the pending queue is drained.
        if data.delivery_ids.len() >= 4 {
            return Err(DataAdmissionError::LimitReached);
        }
        data.delivery_ids.insert(delivery_id.to_owned());
        data.pending.push(ResponseItem::FunctionCallOutput {
            id: None,
            call_id: None,
            name: Some("helper_result".to_owned()),
            namespace: Some("codex_data".to_owned()),
            output: FunctionCallOutputPayload::from_text(text.to_owned()),
            internal_chat_message_metadata_passthrough: None,
        });
        Ok(DataAdmission::Queued)
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "the final DATA empty check and closing admission share the admission lock"
    )]
    pub(crate) async fn close_data_admission_if_idle(&self, turn_id: &str) -> bool {
        let active = self.active_turn.lock().await;
        let Some(turn) = active.as_ref().filter(|turn| {
            turn.task
                .as_ref()
                .is_some_and(|task| task.turn_context.sub_id == turn_id)
        }) else {
            return true;
        };
        let mut state = turn.turn_state.lock().await;
        if !state.data_admission.pending.is_empty() {
            return false;
        }
        state.data_admission.close();
        true
    }
}

#[cfg(test)]
#[path = "data_admission_tests.rs"]
mod tests;
