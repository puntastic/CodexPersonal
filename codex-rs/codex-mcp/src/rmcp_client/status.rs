//! Observe the latest startup attempt without polling it or initiating a retry.

use codex_protocol::mcp::McpServerConnectionStatus as Status;

use super::AsyncManagedClient;
use super::McpConnectionStatusObservation;
use super::StartupOutcomeError;

impl AsyncManagedClient {
    pub(crate) async fn connection_status(&self) -> Status {
        self.connection_status_observation().await.status
    }

    pub(crate) async fn connection_status_observation(&self) -> McpConnectionStatusObservation {
        if self.cancel_token.is_cancelled() {
            return McpConnectionStatusObservation {
                status: Status::Cancelled,
                startup_error: None,
            };
        }
        let reconnect_outcome = if let Some(reconnect) = &self.startup_reconnect {
            let state = reconnect
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.reconnect_in_flight {
                return McpConnectionStatusObservation {
                    status: Status::Starting,
                    startup_error: None,
                };
            }
            state
                .current_client
                .clone()
                .map(Ok)
                .or_else(|| state.last_error.clone().map(Err))
        } else {
            None
        };
        let outcome = reconnect_outcome.or_else(|| self.client.peek().cloned());
        match outcome {
            Some(Ok(client)) => {
                if client.client.is_closed().await {
                    McpConnectionStatusObservation {
                        status: Status::Failed,
                        startup_error: None,
                    }
                } else {
                    McpConnectionStatusObservation {
                        status: Status::Connected,
                        startup_error: None,
                    }
                }
            }
            Some(Err(error @ StartupOutcomeError::Failed { .. })) => {
                let status = if error.is_authentication_required() {
                    Status::AuthenticationRequired
                } else {
                    Status::Failed
                };
                McpConnectionStatusObservation {
                    status,
                    startup_error: Some(error),
                }
            }
            Some(Err(StartupOutcomeError::Cancelled)) => McpConnectionStatusObservation {
                status: Status::Cancelled,
                startup_error: None,
            },
            None => McpConnectionStatusObservation {
                status: Status::Starting,
                startup_error: None,
            },
        }
    }
}
