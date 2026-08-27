use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ConfigWarningNotification;
use codex_app_server_protocol::JSONRPCNotification;
use codex_app_server_protocol::ServerNotification;
use serde_json::Value;
use tempfile::TempDir;
use tokio::time::timeout;

const LOGS_FALLBACK_WARNING_SUMMARY: &str =
    "Codex could not open its local diagnostic log database.";
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn is_logs_fallback_warning(notification: &JSONRPCNotification) -> bool {
    notification.method == "configWarning"
        && notification
            .params
            .as_ref()
            .and_then(|params| params.get("summary"))
            .and_then(Value::as_str)
            == Some(LOGS_FALLBACK_WARNING_SUMMARY)
}

#[tokio::test]
async fn initialize_warns_and_continues_when_optional_logs_db_is_unavailable() -> Result<()> {
    let codex_home = TempDir::new()?;
    let blocked_logs_path = codex_home.path().join("logs_2.sqlite");
    std::fs::create_dir(&blocked_logs_path)?;
    let marker = blocked_logs_path.join("leave-me-alone");
    std::fs::write(&marker, "persistent-store-marker")?;

    let mut app_server = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;

    let notification = timeout(
        READ_TIMEOUT,
        app_server.read_stream_until_matching_notification(
            "optional logs database configWarning",
            is_logs_fallback_warning,
        ),
    )
    .await??;
    let notification: ServerNotification = notification.try_into()?;
    let ServerNotification::ConfigWarning(ConfigWarningNotification {
        summary,
        details,
        path,
        range,
    }) = notification
    else {
        anyhow::bail!("unexpected notification variant");
    };

    assert_eq!(summary, LOGS_FALLBACK_WARNING_SUMMARY);
    let details = details.context("logs fallback warning should include details")?;
    assert!(
        details.contains(&blocked_logs_path.display().to_string()),
        "warning should identify the unavailable persistent store: {details}"
    );
    assert!(
        details.contains("remain in memory") && details.contains("retry it on the next launch"),
        "warning should explain the bounded fallback: {details}"
    );
    assert_eq!(path, None);
    assert_eq!(range, None);
    assert_eq!(std::fs::read_to_string(&marker)?, "persistent-store-marker");
    assert!(blocked_logs_path.is_dir());

    Ok(())
}
