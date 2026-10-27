use super::read_startup_config;
use super::startup_config_error;
use super::validate_client_info;
use crate::config_manager::ConfigManager;
use anyhow::Result;
use codex_app_server_protocol::ClientInfo;
use codex_config::CloudConfigBundleLoader;
use codex_config::LoaderOverrides;
use codex_config::types::AuthCredentialsStoreMode;
use codex_login::AuthKeyringBackendKind;
use codex_login::login_with_api_key;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

const FILE_AUTH_CONFIG: &str = "cli_auth_credentials_store = 'file'\n";

fn fixture(config: &str) -> Result<(TempDir, ConfigManager)> {
    let home = TempDir::new()?;
    std::fs::write(home.path().join("config.toml"), config)?;
    let loader_overrides = LoaderOverrides {
        managed_config_path: Some(home.path().join("managed_config.toml")),
        system_config_path: Some(home.path().join("system_config.toml")),
        system_requirements_path: Some(home.path().join("requirements.toml")),
        ignore_project_config: true,
        ..LoaderOverrides::without_managed_config_for_tests()
    };
    let manager = ConfigManager::new_for_tests(
        home.path().to_path_buf(),
        Vec::new(),
        loader_overrides,
        CloudConfigBundleLoader::default(),
    );
    Ok((home, manager))
}

#[tokio::test]
async fn startup_config_serializes_no_requirements_without_changing_user_config() -> Result<()> {
    let (home, manager) = fixture(FILE_AUTH_CONFIG)?;
    let response = read_startup_config(&manager, /*sign_out*/ false)
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;

    assert_eq!(
        serde_json::to_value(response)?,
        json!({
            "appServerVersion": env!("CARGO_PKG_VERSION"),
            "configRequirements": {
                "requirements": null,
                "supportsIndependentSpeedModes": true,
            },
        }),
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("config.toml"))?,
        FILE_AUTH_CONFIG,
    );
    assert!(!home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn startup_config_exposes_managed_application_network_policy() -> Result<()> {
    let (home, manager) = fixture(FILE_AUTH_CONFIG)?;
    std::fs::write(
        home.path().join("requirements.toml"),
        "[application.network.domains]\n'GOV.example.com.' = 'allow'\n'github.com' = 'deny'\n",
    )?;
    let response = read_startup_config(&manager, /*sign_out*/ false)
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let wire = serde_json::to_value(response)?;

    assert_eq!(
        wire["configRequirements"]["requirements"]["application"],
        json!({
            "network": {
                "enabled": true,
                "domains": {"gov.example.com": "allow", "github.com": "deny"},
            },
        }),
    );
    assert_eq!(
        std::fs::read_to_string(home.path().join("config.toml"))?,
        FILE_AUTH_CONFIG,
    );
    Ok(())
}

#[tokio::test]
async fn startup_config_rejects_invalid_config_instead_of_returning_defaults() -> Result<()> {
    let invalid_config = "cli_auth_credentials_store = 'file'\ninvalid = [\n";
    let (home, manager) = fixture(invalid_config)?;
    match read_startup_config(&manager, /*sign_out*/ false).await {
        Ok(_) => anyhow::bail!("invalid config must not produce a startup policy response"),
        Err(error) => {
            let data = error
                .data
                .expect("a config failure needs the Desktop error discriminator");
            assert_eq!(data["reason"], "configLoad");
            assert!(
                data["detail"]
                    .as_str()
                    .is_some_and(|detail| !detail.is_empty())
            );
        }
    }
    assert_eq!(
        std::fs::read_to_string(home.path().join("config.toml"))?,
        invalid_config,
    );
    Ok(())
}

#[tokio::test]
async fn startup_config_rejects_invalid_application_policy() -> Result<()> {
    let (home, manager) = fixture(FILE_AUTH_CONFIG)?;
    std::fs::write(
        home.path().join("requirements.toml"),
        "[application.network.domains]\n'*.example.com' = 'allow'\n",
    )?;
    match read_startup_config(&manager, /*sign_out*/ false).await {
        Ok(_) => anyhow::bail!("invalid managed policy must not produce a startup response"),
        Err(error) => assert!(
            error.message.contains("application.network.domains"),
            "{error:?}",
        ),
    }
    Ok(())
}

#[tokio::test]
async fn startup_sign_out_removes_only_the_fixture_credentials_and_can_repeat() -> Result<()> {
    let (home, manager) = fixture(FILE_AUTH_CONFIG)?;
    login_with_api_key(
        home.path(),
        "sk-startup-fixture",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let auth_path = home.path().join("auth.json");
    let original_auth = std::fs::read(&auth_path)?;
    let untouched = TempDir::new()?;
    let untouched_auth = untouched.path().join("auth.json");
    std::fs::write(&untouched_auth, b"unrelated fixture credential sentinel")?;

    read_startup_config(&manager, /*sign_out*/ false)
        .await
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert_eq!(std::fs::read(&auth_path)?, original_auth);

    for _ in 0..2 {
        let response = read_startup_config(&manager, /*sign_out*/ true)
            .await
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
        assert_eq!(
            serde_json::to_value(response)?["configRequirements"]["requirements"],
            json!(null),
        );
        assert!(!auth_path.exists());
        assert_eq!(
            std::fs::read_to_string(home.path().join("config.toml"))?,
            FILE_AUTH_CONFIG,
        );
        assert_eq!(
            std::fs::read(&untouched_auth)?,
            b"unrelated fixture credential sentinel",
        );
    }
    Ok(())
}

#[test]
fn startup_client_info_accepts_desktop_identity_and_rejects_header_injection() -> Result<()> {
    let client_info = ClientInfo {
        name: "codex_desktop".to_string(),
        title: Some("Codex Desktop".to_string()),
        version: "26.1021.31345".to_string(),
    };
    validate_client_info(&client_info).map_err(|error| anyhow::anyhow!("{error:?}"))?;

    for invalid in [
        ClientInfo {
            name: "codex_desktop\r\nInjected: value".to_string(),
            ..client_info.clone()
        },
        ClientInfo {
            version: "26.1021.31345\r\nInjected: value".to_string(),
            ..client_info
        },
    ] {
        assert!(validate_client_info(&invalid).is_err());
    }
    Ok(())
}

#[test]
fn startup_config_errors_preserve_file_location_and_cloud_relogin_metadata() {
    use codex_config::CloudConfigBundleLoadError;
    use codex_config::CloudConfigBundleLoadErrorCode;
    use codex_config::ConfigError;
    use codex_config::ConfigLoadError;
    use codex_config::TextPosition;
    use codex_config::TextRange;

    let config_error = std::io::Error::other(ConfigLoadError::new(
        ConfigError::new(
            std::path::PathBuf::from("fixture-config.toml"),
            TextRange {
                start: TextPosition { line: 3, column: 2 },
                end: TextPosition { line: 3, column: 5 },
            },
            "invalid fixture setting",
        ),
        None,
    ));
    assert_eq!(
        startup_config_error(&config_error).data,
        Some(json!({
            "reason": "configLoad",
            "detail": "fixture-config.toml:3:2: invalid fixture setting",
            "filePath": "fixture-config.toml",
            "line": 3,
            "column": 2,
        })),
    );

    let cloud_error = std::io::Error::other(CloudConfigBundleLoadError::new(
        CloudConfigBundleLoadErrorCode::Auth,
        Some(401),
        "fixture session expired",
    ));
    assert_eq!(
        startup_config_error(&cloud_error).data,
        Some(json!({
            "reason": "cloudConfigBundle",
            "errorCode": "Auth",
            "action": "relogin",
            "statusCode": 401,
            "detail": "fixture session expired",
        })),
    );
}
