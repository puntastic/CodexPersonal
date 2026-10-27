//! One-shot Desktop policy preflight, before transports or thread storage start.

use crate::config_manager::ConfigManager;
use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::error_code::invalid_request;
use crate::request_processors::config_load_error;
use crate::request_processors::requirements_read_response;
use axum::http::HeaderValue;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::ConfigRequirementsReadResponse;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_arg0::Arg0DispatchPaths;
use codex_config::ConfigLoadError;
use codex_config::LoaderOverrides;
use codex_config::NoopThreadConfigLoader;
use codex_core::config::find_codex_home;
use codex_login::AuthManager;
use codex_login::default_client::SetOriginatorError;
use codex_login::default_client::USER_AGENT_SUFFIX;
use codex_login::default_client::set_default_originator;
use codex_utils_cli::CliConfigOverrides;
use serde::Serialize;
use std::io::Write;
use std::sync::Arc;

/// Options for the one-shot Desktop startup contract.
#[derive(Debug, Default)]
pub struct StartupConfigOptions {
    pub client_info: Option<ClientInfo>,
    pub sign_out: bool,
    pub strict_config: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupConfigResponse {
    app_server_version: &'static str,
    config_requirements: ConfigRequirementsReadResponse,
}

#[derive(Serialize)]
struct StartupConfigError {
    error: JSONRPCErrorError,
}

/// Prints one complete JSON document and exits without opening app-server transports or stores.
///
/// Policy and authentication errors remain errors; no default-policy response is substituted.
/// Explicit sign-out uses the configured credential backend before loading cloud requirements.
pub async fn print_startup_config(
    arg0_paths: Arg0DispatchPaths,
    cli_config_overrides: CliConfigOverrides,
    options: StartupConfigOptions,
) -> std::io::Result<()> {
    let result = async {
        if let Some(client_info) = &options.client_info {
            apply_client_info(client_info)?;
        }
        let overrides = cli_config_overrides
            .parse_overrides()
            .map_err(|err| invalid_params(format!("error parsing -c overrides: {err}")))?;
        let codex_home = find_codex_home().map_err(|err| startup_config_error(&err))?;
        let loader_overrides = crate::loader_overrides_with_test_user_config_file(
            LoaderOverrides::default(),
            crate::test_user_config_file_from_env(),
        )
        .map_err(|err| startup_config_error(&err))?;
        let manager = ConfigManager::new(
            codex_home.to_path_buf(),
            overrides,
            loader_overrides,
            options.strict_config,
            Default::default(),
            arg0_paths,
            Arc::new(NoopThreadConfigLoader),
        );
        read_startup_config(&manager, options.sign_out).await
    }
    .await;

    let failed = result.is_err();
    let mut stdout = std::io::stdout().lock();
    match result {
        Ok(response) => serde_json::to_writer(&mut stdout, &response)?,
        Err(error) => serde_json::to_writer(&mut stdout, &StartupConfigError { error })?,
    }
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    if failed {
        Err(std::io::Error::other(
            "startup configuration could not be loaded",
        ))
    } else {
        Ok(())
    }
}

async fn read_startup_config(
    manager: &ConfigManager,
    sign_out: bool,
) -> Result<StartupConfigResponse, JSONRPCErrorError> {
    // Load local policy before authentication or cloud access. Unlike ordinary server startup,
    // preflight must not fall back to defaults after a malformed or inaccessible configuration.
    let bootstrap = manager
        .load_non_project_config()
        .await
        .map_err(|err| startup_config_error(&err))?;
    bootstrap
        .auth_config()
        .validate()
        .map_err(|err| startup_config_error(&err))?;
    let bootstrap_auth =
        AuthManager::shared_from_config(&bootstrap, /*enable_codex_api_key_env*/ false)
            .await
            .map_err(|err| startup_config_error(&std::io::Error::other(err)))?;
    if sign_out {
        bootstrap_auth
            .logout_with_revoke()
            .await
            .map_err(|err| invalid_request(format!("logout failed: {err}")))?;
    }
    manager.replace_cloud_config_bundle_loader(
        bootstrap_auth,
        bootstrap.chatgpt_base_url.clone(),
        bootstrap.http_client_factory(),
    );
    let config = manager
        .load_non_project_config()
        .await
        .map_err(|err| startup_config_error(&err))?;
    config
        .validate_windows_mxc_requirement()
        .map_err(|err| startup_config_error(&err))?;
    config
        .auth_config()
        .validate()
        .map_err(|err| startup_config_error(&err))?;
    let auth = AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ false)
        .await
        .map_err(|err| startup_config_error(&std::io::Error::other(err)))?;
    // Keep requirements and the auth policy bound to the same successful config load.
    let requirements = config.config_layer_stack.requirements_toml();
    let requirements = (!requirements.is_empty()).then(|| requirements.clone());
    Ok(StartupConfigResponse {
        app_server_version: env!("CARGO_PKG_VERSION"),
        config_requirements: requirements_read_response(requirements, auth.allowed_login_methods()),
    })
}

fn startup_config_error(err: &std::io::Error) -> JSONRPCErrorError {
    let mut error = config_load_error(err);
    // Preserve the existing cloud error classification, including relogin and retry metadata.
    if error.data.is_some() {
        return error;
    }
    let mut data = serde_json::json!({ "reason": "configLoad", "detail": err.to_string() });
    let mut source = err
        .get_ref()
        .map(|err| err as &(dyn std::error::Error + 'static));
    while let Some(current) = source {
        if let Some(load_error) = current.downcast_ref::<ConfigLoadError>() {
            let location = load_error.config_error();
            data["filePath"] = serde_json::json!(location.path.to_string_lossy());
            data["line"] = serde_json::json!(location.range.start.line);
            data["column"] = serde_json::json!(location.range.start.column);
            break;
        }
        source = current.source();
    }
    error.data = Some(data);
    error
}

fn validate_client_info(client_info: &ClientInfo) -> Result<(), JSONRPCErrorError> {
    if HeaderValue::from_str(&client_info.name).is_err()
        || HeaderValue::from_str(&client_info.version).is_err()
    {
        return Err(invalid_params(
            "startup client name and version must be valid HTTP header values",
        ));
    }
    Ok(())
}

fn apply_client_info(client_info: &ClientInfo) -> Result<(), JSONRPCErrorError> {
    validate_client_info(client_info)?;
    if matches!(
        client_info.name.as_str(),
        "codex_app_server_daemon" | "codex-backend"
    ) {
        return Ok(());
    }
    match set_default_originator(client_info.name.clone()) {
        Ok(()) | Err(SetOriginatorError::AlreadyInitialized) => {}
        Err(SetOriginatorError::InvalidHeaderValue) => {
            return Err(invalid_params("invalid startup client name"));
        }
    }
    let mut suffix = USER_AGENT_SUFFIX
        .lock()
        .map_err(|_| internal_error("startup client identity is unavailable"))?;
    *suffix = Some(format!("{}; {}", client_info.name, client_info.version));
    Ok(())
}

#[cfg(test)]
#[path = "startup_config_tests.rs"]
mod tests;
