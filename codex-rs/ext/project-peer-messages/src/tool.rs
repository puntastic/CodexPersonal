use crate::DeliveryMode;
use crate::MAX_MESSAGE_BYTES;
use crate::PeerMessageHost;
use crate::PeerPolicy;
use crate::PeerRuntimeBinding;
use crate::PeerSendOutcome;
use crate::PeerSendRequest;
use crate::RefusalReason;
use codex_protocol::ThreadId;
use codex_tools::FunctionCallError;
use codex_tools::JsonSchema;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolCall;
use codex_tools::ToolExecutor;
use codex_tools::ToolExecutorFuture;
use codex_tools::ToolName;
use codex_tools::ToolOutput;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

const MAX_ARGUMENT_BYTES: usize = 32 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    recipient_id: ThreadId,
    message: String,
    delivery: DeliveryMode,
}

pub(super) struct PeerTool {
    pub(super) caller: ThreadId,
    pub(super) runtime: PeerRuntimeBinding,
    pub(super) policy: Arc<PeerPolicy>,
    pub(super) host: Arc<dyn PeerMessageHost>,
}

impl PeerTool {
    async fn send(&self, raw: &str, turn_id: String) -> Result<PeerSendOutcome, FunctionCallError> {
        if raw.len() > MAX_ARGUMENT_BYTES {
            return Err(FunctionCallError::RespondToModel(
                "peer message arguments exceed 32768 bytes".into(),
            ));
        }
        let args: Arguments = serde_json::from_str(raw).map_err(|_| {
            FunctionCallError::RespondToModel(
                "provide only recipient_id, message, and delivery".into(),
            )
        })?;
        if args.message.is_empty() || args.message.len() > MAX_MESSAGE_BYTES {
            return Ok(PeerSendOutcome::refused(RefusalReason::InvalidMessage));
        }
        if !self
            .policy
            .allows(self.caller, args.recipient_id, args.delivery)
        {
            return Ok(PeerSendOutcome::refused(RefusalReason::Denied));
        }
        Ok(self
            .host
            .send(PeerSendRequest {
                sender: self.caller,
                sender_turn_id: turn_id,
                sender_runtime_token: self.runtime.token(),
                policy_generation: self.runtime.generation().to_owned(),
                recipient: args.recipient_id,
                delivery: args.delivery,
                message: args.message,
            })
            .await)
    }
}

impl<'call> ToolExecutor<ToolCall<'call>> for PeerTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("send_peer_message")
    }

    fn spec(&self) -> ToolSpec {
        let parameters = JsonSchema::object(
            BTreeMap::from([
                (
                    "recipient_id".into(),
                    JsonSchema::string_enum(
                        self.policy
                            .recipients(self.caller)
                            .into_iter()
                            .map(serde_json::Value::String)
                            .collect(),
                        /*description*/ None,
                    ),
                ),
                (
                    "message".into(),
                    JsonSchema::string(Some(
                        "Nonempty text, at most 4096 UTF-8 bytes; never truncated.".into(),
                    )),
                ),
                (
                    "delivery".into(),
                    JsonSchema::string_enum(
                        vec!["active_turn".into(), "idle_turn".into()],
                        /*description*/ None,
                    ),
                ),
            ]),
            Some(vec![
                "recipient_id".into(),
                "message".into(),
                "delivery".into(),
            ]),
            Some(false.into()),
        );
        ToolSpec::Function(ResponsesApiTool {
            name: "send_peer_message".into(),
            description: "Send an attributed message from this chat to a host-allowed independent chat already loaded in this process, within the current user-authorized task scope. active_turn joins only an accepting active regular turn; idle_turn starts only an idle recipient and requires a separate wake grant. Neither mode resumes a cold chat or falls back to another mode. At most four authorized attempts per sender turn. Grants permit communication, not authority to carry out the message. Accepted/started reports admission only, not reading, use or durable storage. An unknown outcome must not be retried automatically.".into(),
            strict: false,
            defer_loading: None,
            parameters,
            output_schema: None,
        })
    }

    fn handle<'a>(&'a self, call: ToolCall<'call>) -> ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(async move {
            // Reserve a complete receipt budget before any possible delivery.
            if call.response_byte_budget(1024) < 1024 {
                return Err(FunctionCallError::RespondToModel(
                    "insufficient output budget for a peer receipt; nothing sent".into(),
                ));
            }
            let outcome = self
                .send(call.function_arguments()?, call.turn_id.clone())
                .await?;
            let accepted = outcome.accepted();
            let value = json!({"policy_generation":self.runtime.generation(), "admission":outcome});
            Ok(
                Box::new(JsonToolOutput::with_success(value, Some(accepted)))
                    as Box<dyn ToolOutput>,
            )
        })
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
