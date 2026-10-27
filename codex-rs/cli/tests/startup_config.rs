//! Desktop preflight runs against synthetic homes without starting thread storage or transports.

use anyhow::Context;
use anyhow::Result;
use anyhow::ensure;
use app_test_support::ChatGptAuthFixture;
use app_test_support::write_chatgpt_auth;
use codex_config::ConfigLoadOptions;
use codex_config::types::AuthCredentialsStoreMode;
use codex_core::config::load_config_toml_with_layer_stack;
use codex_login::AuthKeyringBackendKind;
use codex_login::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;
use codex_login::REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR;
use codex_login::login_with_api_key;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;
use tempfile::TempDir;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const FILE_AUTH_CONFIG: &str = "cli_auth_credentials_store = 'file'\n";
const CLIENT_INFO: &str =
    r#"{"name":"codex_desktop","title":"Codex Desktop","version":"26.1021.31345"}"#;

fn home(config: &str) -> Result<TempDir> {
    let home = TempDir::new()?;
    std::fs::write(home.path().join("config.toml"), config)?;
    for directory in ["sqlite", "tmp", "appdata", "localappdata"] {
        std::fs::create_dir(home.path().join(directory))?;
    }
    Ok(home)
}

fn command(home: &Path) -> Result<Command> {
    let mut command = Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    command.env_clear();
    // Keep only process-loader essentials. No inherited credentials, endpoint, proxy, or
    // Codex configuration variables may escape into these child processes.
    for name in ["PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .current_dir(home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("appdata"))
        .env("LOCALAPPDATA", home.join("localappdata"))
        .env("XDG_CONFIG_HOME", home.join("appdata"))
        .env("XDG_CACHE_HOME", home.join("localappdata"))
        .env("CODEX_HOME", home)
        .env("CODEX_SQLITE_HOME", home.join("sqlite"))
        .env("TEMP", home.join("tmp"))
        .env("TMP", home.join("tmp"))
        .env("TMPDIR", home.join("tmp"))
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .args([
            "app-server",
            "--analytics-default-enabled",
            "--print-startup-config",
            "--startup-client-info",
            CLIENT_INFO,
        ])
        .stdin(Stdio::null());
    Ok(command)
}

fn document(output: &Output, success: bool) -> Result<Value> {
    assert_eq!(
        output.status.success(),
        success,
        "stdout={}; stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // Parsing the entire buffer rejects extra RPC notifications or diagnostic stdout.
    let value: Value = serde_json::from_slice(&output.stdout)?;
    if success {
        assert_eq!(value["appServerVersion"], env!("CARGO_PKG_VERSION"));
        assert!(value["configRequirements"].get("requirements").is_some());
        assert!(value.get("error").is_none());
    } else {
        assert!(value["error"]["code"].is_number());
        assert!(value["error"]["message"].is_string());
        assert!(value.get("configRequirements").is_none());
    }
    Ok(value)
}

fn assert_no_backend_artifacts(home: &Path) -> Result<()> {
    for directory in [
        "sessions",
        "archived_sessions",
        "app-server-control",
        "app-server-daemon",
    ] {
        assert!(!home.join(directory).exists(), "unexpected {directory}");
    }
    assert!(std::fs::read_dir(home.join("sqlite"))?.next().is_none());
    for entry in std::fs::read_dir(home)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        assert!(!name.contains(".sqlite"), "unexpected database {name}");
    }
    Ok(())
}

async fn check_auth_isolation(home: &Path, backend: Option<&str>) -> Result<()> {
    let loaded = load_config_toml_with_layer_stack(
        home,
        Some(&AbsolutePathBuf::from_absolute_path(home)?),
        Vec::new(),
        ConfigLoadOptions::default(),
    )
    .await?;
    ensure!(
        loaded.config_toml.cli_auth_credentials_store == Some(AuthCredentialsStoreMode::File),
        "host-managed policy prevents file-only fixture credentials"
    );
    if let Some(backend) = backend {
        ensure!(
            loaded.config_toml.chatgpt_base_url.as_deref() == Some(backend),
            "host-managed policy prevents the isolated loopback backend"
        );
    }
    Ok(())
}

#[test]
fn startup_config_rejects_malformed_config_with_a_located_error() -> Result<()> {
    let config = "cli_auth_credentials_store = 'file'\ninvalid = [\n";
    let home = home(config)?;
    let value = document(&command(home.path())?.output()?, false)?;
    let data = &value["error"]["data"];
    assert_eq!(data["reason"], "configLoad");
    assert_eq!(
        Path::new(data["filePath"].as_str().context("config path")?),
        home.path().join("config.toml"),
    );
    assert!(data["line"].as_u64().is_some_and(|line| line > 0));
    assert!(data["column"].as_u64().is_some_and(|column| column > 0));
    assert!(
        data["detail"]
            .as_str()
            .is_some_and(|detail| !detail.is_empty())
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("config.toml"))?,
        config
    );
    assert_no_backend_artifacts(home.path())
}

#[tokio::test]
async fn startup_config_accepts_the_custom_cue_activation_table() -> Result<()> {
    let config =
        format!("{FILE_AUTH_CONFIG}[features.cue_activation]\nenabled = true\nmode = 'advisory'\n");
    let home = home(&config)?;
    check_auth_isolation(home.path(), /*backend*/ None).await?;
    document(
        &command(home.path())?.arg("--strict-config").output()?,
        true,
    )?;
    assert_eq!(
        std::fs::read_to_string(home.path().join("config.toml"))?,
        config
    );
    assert_no_backend_artifacts(home.path())
}

#[tokio::test]
async fn startup_config_honors_cli_auth_policy_without_writing_config() -> Result<()> {
    let config = format!("{FILE_AUTH_CONFIG}forced_login_method = 'chatgpt'\n");
    let home = home(&config)?;
    check_auth_isolation(home.path(), /*backend*/ None).await?;
    let value = document(
        &command(home.path())?
            .args(["-c", "forced_login_method='api'"])
            .output()?,
        true,
    )?;
    assert_eq!(
        value["configRequirements"]["requirements"]["allowedLoginMethods"],
        json!(["api"])
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("config.toml"))?,
        config
    );
    assert_no_backend_artifacts(home.path())
}

#[tokio::test]
async fn startup_sign_out_only_removes_the_selected_fixture_auth() -> Result<()> {
    let home = home(FILE_AUTH_CONFIG)?;
    check_auth_isolation(home.path(), /*backend*/ None).await?;
    login_with_api_key(
        home.path(),
        "sk-startup-fixture",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let auth_path = home.path().join("auth.json");
    let original = std::fs::read(&auth_path)?;
    let other_home = TempDir::new()?;
    let sentinel = other_home.path().join("auth.json");
    std::fs::write(&sentinel, &original)?;
    document(&command(home.path())?.output()?, true)?;
    assert_eq!(std::fs::read(&auth_path)?, original);
    for _ in 0..2 {
        document(
            &command(home.path())?.arg("--startup-sign-out").output()?,
            true,
        )?;
        assert!(!auth_path.exists());
        assert_eq!(std::fs::read(&sentinel)?, original);
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml"))?,
            FILE_AUTH_CONFIG
        );
    }
    assert_no_backend_artifacts(home.path())
}

async fn cloud_home(server: &MockServer) -> Result<TempDir> {
    let backend = format!("{}/backend-api", server.uri());
    let home = home(&format!(
        "{FILE_AUTH_CONFIG}chatgpt_base_url = '{backend}'\n"
    ))?;
    check_auth_isolation(home.path(), Some(&backend)).await?;
    write_chatgpt_auth(
        home.path(),
        ChatGptAuthFixture::new("startup-fixture-token")
            .account_id("startup-fixture-workspace")
            .chatgpt_account_id("startup-fixture-workspace")
            .chatgpt_user_id("startup-fixture-user")
            .plan_type("enterprise"),
        AuthCredentialsStoreMode::File,
    )?;
    Ok(home)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_config_exposes_cloud_policy_and_sends_client_identity() -> Result<()> {
    let server = MockServer::start().await;
    let home = cloud_home(&server).await?;
    Mock::given(method("GET"))
        .and(path("/backend-api/wham/config/bundle"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "requirements_toml": {"enterprise_managed": [{
                "id": "startup-policy", "name": "Synthetic startup policy",
                "contents": "[application.network.domains]\n'Allowed.example.' = 'allow'\n'blocked.example' = 'deny'\n",
            }]},
        })))
        .expect(1)
        .mount(&server)
        .await;
    let value = document(&command(home.path())?.output()?, true)?;
    assert_eq!(
        value["configRequirements"]["requirements"]["application"],
        json!({"network": {"enabled": true, "domains": {"allowed.example": "allow", "blocked.example": "deny"}}}),
    );
    let requests = server
        .received_requests()
        .await
        .context("captured requests")?;
    let request = requests.first().context("startup bundle request")?;
    // The existing backend client conveys its originator in the User-Agent prefix.
    let user_agent = request.headers["user-agent"].to_str()?;
    assert!(user_agent.starts_with(&format!("codex_desktop/{} ", env!("CARGO_PKG_VERSION"))));
    assert!(user_agent.contains("(codex_desktop; 26.1021.31345)"));
    server.verify().await;
    assert_no_backend_artifacts(home.path())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_sign_out_revokes_synthetic_chatgpt_auth_before_reading_cloud_policy() -> Result<()>
{
    let server = MockServer::start().await;
    let home = cloud_home(&server).await?;
    let original_config = std::fs::read(home.path().join("config.toml"))?;
    Mock::given(method("POST"))
        .and(path("/oauth/revoke"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/backend-api/wham/config/bundle"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    document(
        &command(home.path())?
            .env(
                REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR,
                format!("{}/oauth/revoke", server.uri()),
            )
            .arg("--startup-sign-out")
            .output()?,
        true,
    )?;
    assert!(!home.path().join("auth.json").exists());
    assert_eq!(
        std::fs::read(home.path().join("config.toml"))?,
        original_config
    );
    server.verify().await;
    assert_no_backend_artifacts(home.path())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_cloud_failure_remains_a_structured_error() -> Result<()> {
    for status in [401, 503] {
        let server = MockServer::start().await;
        let home = cloud_home(&server).await?;
        Mock::given(method("GET"))
            .and(path("/backend-api/wham/config/bundle"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(json!({"error": {"code": "refresh_token_expired"}})),
            )
            .mount(&server)
            .await;
        let value = document(
            &command(home.path())?
                .env(
                    REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR,
                    format!("{}/oauth/token", server.uri()),
                )
                .output()?,
            false,
        )?;
        assert_eq!(value["error"]["data"]["reason"], "cloudConfigBundle");
        assert_eq!(
            value["error"]["data"]["errorCode"],
            if status == 401 {
                "Auth"
            } else {
                "RequestFailed"
            }
        );
        assert_eq!(value["error"]["data"]["statusCode"], status);
        assert_no_backend_artifacts(home.path())?;
    }
    Ok(())
}
