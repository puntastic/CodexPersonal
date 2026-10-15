use super::TurnRequestProcessor;
use crate::error_code::invalid_params;
use crate::error_code::invalid_request;
use crate::outgoing_message::ConnectionRequestId;
use crate::request_processors::config_load_error;
use codex_app_server_protocol::ClientResponsePayload;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::TurnAdmitDataParams;
use codex_app_server_protocol::TurnAdmitDataResponse;
use codex_app_server_protocol::TurnAdmitDataStatus;
use codex_core::DataAdmission;
use codex_core::DataAdmissionError;

impl TurnRequestProcessor {
    pub(crate) async fn turn_admit_data(
        &self,
        request_id: &ConnectionRequestId,
        params: TurnAdmitDataParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let (_, thread) = self.load_thread(&params.thread_id).await?;
        self.ensure_direct_input_allowed(request_id, thread.as_ref())
            .await?;
        self.config_manager
            .check_thread_model_provider(thread.config().await.as_ref())
            .await
            .map_err(|error| config_load_error(&error))?;

        let status = match thread
            .admit_data(&params.expected_turn_id, &params.delivery_id, &params.text)
            .await
            .map_err(|error| {
                let reason = match error {
                    DataAdmissionError::InvalidInput => "invalidInput",
                    DataAdmissionError::NoActiveTurn => "noActiveTurn",
                    DataAdmissionError::ExpectedTurnMismatch => "expectedTurnMismatch",
                    DataAdmissionError::NotRegular => "notRegular",
                    DataAdmissionError::Closed => "closed",
                    DataAdmissionError::Cancelled => "cancelled",
                    DataAdmissionError::Duplicate => "duplicate",
                    DataAdmissionError::LimitReached => "limitReached",
                };
                // Core refusals are unit variants with payload-free messages.
                let mut rpc_error = if error == DataAdmissionError::InvalidInput {
                    invalid_params(error.to_string())
                } else {
                    invalid_request(error.to_string())
                };
                rpc_error.data = Some(serde_json::json!({ "reason": reason }));
                rpc_error
            })? {
            DataAdmission::Queued => TurnAdmitDataStatus::Queued,
        };
        Ok(Some(
            TurnAdmitDataResponse {
                status,
                turn_id: params.expected_turn_id,
                delivery_id: params.delivery_id,
            }
            .into(),
        ))
    }
}
