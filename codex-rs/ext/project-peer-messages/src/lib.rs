//! Opt-in, caller-bound communication between independent roots in one host.
//! A delivery grant authorizes communication, not execution of the message's requests.

mod extension;
mod policy;
mod tool;

pub use extension::install;
pub use policy::HOST_POLICY_FILE;
pub use policy::MAX_POLICY_BYTES;
pub use policy::PeerPolicy;
pub use policy::process_policy;

use codex_protocol::ThreadId;
use futures::future::BoxFuture;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

pub const MAX_MESSAGE_BYTES: usize = 4096;
pub const MAX_SEND_ATTEMPTS_PER_TURN: u32 = 4;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    ActiveTurn,
    IdleTurn,
}

/// A host-created runtime attachment. Reusing a thread ID does not reuse this token.
#[derive(Clone, Debug)]
pub struct PeerRuntimeBinding {
    generation: String,
    token: Uuid,
}

impl PeerRuntimeBinding {
    pub fn new(policy: &PeerPolicy) -> Self {
        Self {
            generation: policy.generation().to_owned(),
            token: Uuid::now_v7(),
        }
    }

    pub fn generation(&self) -> &str {
        &self.generation
    }

    pub fn token(&self) -> Uuid {
        self.token
    }
}

/// Host metadata plus validated model input. The host rechecks identity and grants.
pub struct PeerSendRequest {
    pub sender: ThreadId,
    pub sender_turn_id: String,
    pub sender_runtime_token: Uuid,
    pub policy_generation: String,
    pub recipient: ThreadId,
    pub delivery: DeliveryMode,
    pub message: String,
}

/// The host owns lifecycle checks, per-turn attempt limits, and typed admission.
/// Implementations never resume cold recipients or convert peer data to user input.
pub trait PeerMessageHost: Send + Sync {
    fn send(&self, request: PeerSendRequest) -> BoxFuture<'_, PeerSendOutcome>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PeerSendOutcome {
    AcceptedCurrentTurn {
        turn_id: String,
        message_id: String,
    },
    StartedIdleTurn {
        turn_id: String,
        root_turn_id: String,
        message_id: String,
    },
    Refused {
        reason: RefusalReason,
    },
    /// A transport/response failure cannot establish that admission did not occur.
    OutcomeUnknown {
        message_id: String,
    },
}

impl PeerSendOutcome {
    pub fn refused(reason: RefusalReason) -> Self {
        Self::Refused { reason }
    }

    pub fn accepted(&self) -> bool {
        matches!(
            self,
            Self::AcceptedCurrentTurn { .. } | Self::StartedIdleTurn { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalReason {
    Denied,
    InvalidMessage,
    HostUnavailable,
    SenderUnavailable,
    SenderTurnInactive,
    RecipientUnloaded,
    NotIndependentRoot,
    PolicyOrRuntimeMismatch,
    TurnAttemptLimit,
    NoActiveRecipientTurn,
    RecipientTurnClosed,
    NotIdle,
    PendingTriggerTurn,
    PlanMode,
    ServerDraining,
    AdmissionDeclined,
}
