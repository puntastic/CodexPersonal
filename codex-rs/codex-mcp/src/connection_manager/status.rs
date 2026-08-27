//! Read-only connection state; observing a server must not start it.

use std::collections::HashMap;

use codex_protocol::mcp::McpServerConnectionStatus;
use codex_protocol::protocol::McpStartupFailureReason;
use codex_rmcp_client::McpAuthState;

use crate::McpConfig;
use crate::runtime::McpServerConnectionStatusSnapshot;

use super::McpConnectionSet;
use super::McpServerConnection;
use super::startup::mcp_init_error_display;
use super::startup::mcp_startup_failure_reason;

impl McpConnectionSet {
    pub(crate) async fn connection_statuses(&self) -> HashMap<String, McpServerConnectionStatus> {
        use McpServerConnectionStatus as Status;

        let mut statuses = self
            .disabled_servers
            .iter()
            .map(|name| (name.clone(), Status::Disabled))
            .collect::<HashMap<_, _>>();
        for (name, view) in &self.servers {
            let connection = &view.connection;
            let client = &connection.client;
            let status = if connection.startup_is_dormant() && !client.cancel_token.is_cancelled() {
                Status::NotStarted
            } else {
                client.connection_status().await
            };
            statuses.insert(name.clone(), status);
        }
        statuses
    }

    pub(crate) async fn connection_status_details(
        &self,
        config: &McpConfig,
    ) -> HashMap<String, McpServerConnectionStatusSnapshot> {
        use McpServerConnectionStatus as Status;

        let mut statuses = self
            .disabled_servers
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    McpServerConnectionStatusSnapshot {
                        status: Status::Disabled,
                        error: None,
                        failure_reason: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        for (name, view) in &self.servers {
            let connection = &view.connection;
            let client = &connection.client;
            let details = if connection.startup_is_dormant() && !client.cancel_token.is_cancelled()
            {
                McpServerConnectionStatusSnapshot {
                    status: Status::NotStarted,
                    error: None,
                    failure_reason: None,
                }
            } else {
                let observation = client.connection_status_observation().await;
                let (error, failure_reason) = observation.startup_error.map_or_else(
                    || (None, None),
                    |error| {
                        let failure_reason = connection_failure_reason(connection, &error);
                        let configured_server = config
                            .mcp_server_catalog
                            .server(name)
                            .map(super::super::catalog::ResolvedMcpServer::config);
                        (
                            Some(mcp_init_error_display(
                                name,
                                configured_server,
                                &error,
                                failure_reason,
                            )),
                            failure_reason,
                        )
                    },
                );
                McpServerConnectionStatusSnapshot {
                    status: observation.status,
                    error,
                    failure_reason,
                }
            };
            statuses.insert(name.clone(), details);
        }
        statuses
    }
}

fn connection_failure_reason(
    connection: &McpServerConnection,
    error: &crate::rmcp_client::StartupOutcomeError,
) -> Option<McpStartupFailureReason> {
    let has_retained_oauth = connection.identity.as_ref().is_some_and(|identity| {
        identity
            .oauth_credentials()
            .is_ok_and(|credentials| credentials.is_some())
    });
    failure_reason_from_retained_oauth(has_retained_oauth, error)
}

pub(super) fn failure_reason_from_retained_oauth(
    has_retained_oauth: bool,
    error: &crate::rmcp_client::StartupOutcomeError,
) -> Option<McpStartupFailureReason> {
    let auth_state = has_retained_oauth.then_some(McpAuthState::OAuth);
    mcp_startup_failure_reason(auth_state, error)
}
