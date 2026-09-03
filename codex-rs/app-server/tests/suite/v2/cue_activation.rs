use std::fs;

use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use codex_app_server_protocol::WarningNotification;
use core_test_support::responses;
use core_test_support::responses::ResponsesRequest;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;

const REQUEST: &str = "Please review this pull request and inspect changed behavior.";
const NO_MATCH_REQUEST: &str = "Please compare two spreadsheets.";
const CATALOG: &str = r#"{"schema":"codex.cue-header-catalog.v0","cues":[{"id":"synthetic-pr-review","title":"Synthetic pull-request review cue","source":"fixture://synthetic/cue-pr-review-v0","task_shape":["review pull request"],"risk":"A review may mistake description-writing for change inspection.","discriminator":"Is the task to inspect changed behavior rather than author its description?","release_when":"The changed behavior and nearest regression have both been checked.","avoid_when":["pull request description"],"provenance":{"status":"test_fixture"}}]}"#;

#[cfg(windows)]
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(25);
#[cfg(not(windows))]
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn cue_activation_public_modes_are_bounded_and_fail_open() -> Result<()> {
    let advisory = capture(true, "advisory", Some(CATALOG), REQUEST).await?;
    let cues = cue_fragments(&advisory);
    assert_eq!(cues.len(), 1);
    assert!(cues[0].contains("fixture://synthetic/cue-pr-review-v0"));
    assert!(cues[0].contains("report_cue_outcome"));
    assert!(!cues[0].contains("description-writing"));
    assert!(has_tool(&advisory, "report_cue_outcome"));

    let no_match = capture(true, "advisory", Some(CATALOG), NO_MATCH_REQUEST).await?;
    assert!(cue_fragments(&no_match).is_empty());
    assert!(
        has_tool(&no_match, "report_cue_outcome"),
        "advisory tools are assembled before cue selection, so a no-match turn must still advertise report_cue_outcome"
    );

    let shadow = capture(true, "shadow", Some(CATALOG), REQUEST).await?;
    let off = capture(false, "shadow", Some(CATALOG), REQUEST).await?;
    assert!(cue_fragments(&shadow).is_empty() && cue_fragments(&off).is_empty());
    assert!(!has_tool(&shadow, "report_cue_outcome"));
    assert!(!has_tool(&off, "report_cue_outcome"));
    assert_eq!(stable_prompt_texts(&shadow), stable_prompt_texts(&off));

    for catalog in [Some("{"), None] {
        let failed_open = capture(true, "advisory", catalog, REQUEST).await?;
        assert!(cue_fragments(&failed_open).is_empty());
        assert!(
            has_tool(&failed_open, "report_cue_outcome"),
            "advisory tools are assembled before catalog loading, so a catalog-error turn must still advertise report_cue_outcome"
        );
        assert!(
            failed_open
                .message_input_texts("user")
                .iter()
                .any(|text| text == REQUEST)
        );
    }
    Ok(())
}

#[tokio::test]
async fn advisory_cue_records_the_active_models_bounded_self_report() -> Result<()> {
    let server = responses::start_mock_server().await;
    let call_id = "cue-report-1";
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("cue-response-1"),
                responses::ev_function_call(
                    call_id,
                    "report_cue_outcome",
                    &json!({
                        "effect": "helpful",
                        "source_consulted": "yes",
                        "burden": "light",
                        "outcome": "reframed_approach"
                    })
                    .to_string(),
                ),
                responses::ev_completed("cue-response-1"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message("cue-message-1", "done"),
                responses::ev_completed("cue-response-2"),
            ]),
        ],
    )
    .await;
    let home = TempDir::new()?;
    fs::write(home.path().join("cue-catalog.json"), CATALOG)?;
    MockResponsesConfig::new(&server.uri())
        .with_extra_config(
            "[features.cue_activation]\nenabled = true\nmode = \"advisory\"\ncatalog_path = \"cue-catalog.json\"",
        )
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_json_logging("warn,codex_cue_activation_extension=info")
        .build_initialized()
        .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;

    app.start_turn_and_wait_for_completion(TurnStartParams {
        thread_id: thread.id.clone(),
        input: vec![UserInput::Text {
            text: REQUEST.into(),
            text_elements: Vec::new(),
        }],
        ..Default::default()
    })
    .await?;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);
    assert!(has_tool(&requests[0], "report_cue_outcome"));
    assert_eq!(
        requests[1].function_call_output_text(call_id).as_deref(),
        Some(r#"{"status":"recorded"}"#)
    );
    let events = app
        .wait_for_json_log_events("codex.cue_activation.assessment", 1)
        .await?;
    assert_eq!(events[0]["fields"]["status"], "recorded");
    assert_eq!(events[0]["fields"]["effect"], "helpful");
    assert_eq!(events[0]["fields"]["source_consulted"], "yes");
    assert_eq!(events[0]["fields"]["burden"], "light");
    assert_eq!(events[0]["fields"]["outcome"], "reframed_approach");
    assert!(events[0]["fields"].get("changed").is_none());
    assert_eq!(events[0]["fields"]["thread_id"], thread.id);
    assert!(events[0]["fields"]["catalog_sha256"].as_str().is_some());
    Ok(())
}

#[tokio::test]
async fn advisory_cue_rejects_invalid_report_without_echo_and_allows_retry() -> Result<()> {
    const INVALID_ENUM_SENTINEL: &str = "invalid_enum_sentinel_must_not_echo_c7a20d";
    const INVALID_CALL_ID: &str = "cue-report-invalid";
    const VALID_CALL_ID: &str = "cue-report-retry";

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_sequence(
        &server,
        vec![
            responses::sse(vec![
                responses::ev_response_created("cue-response-invalid"),
                responses::ev_function_call(
                    INVALID_CALL_ID,
                    "report_cue_outcome",
                    &json!({
                        "effect": INVALID_ENUM_SENTINEL,
                        "source_consulted": "yes",
                        "burden": "light",
                        "outcome": "reframed_approach"
                    })
                    .to_string(),
                ),
                responses::ev_completed("cue-response-invalid"),
            ]),
            responses::sse(vec![
                responses::ev_response_created("cue-response-retry"),
                responses::ev_function_call(
                    VALID_CALL_ID,
                    "report_cue_outcome",
                    &json!({
                        "effect": "helpful",
                        "source_consulted": "yes",
                        "burden": "light",
                        "outcome": "reframed_approach"
                    })
                    .to_string(),
                ),
                responses::ev_completed("cue-response-retry"),
            ]),
            responses::sse(vec![
                responses::ev_assistant_message("cue-message-after-retry", "done"),
                responses::ev_completed("cue-response-after-retry"),
            ]),
        ],
    )
    .await;
    let home = TempDir::new()?;
    fs::write(home.path().join("cue-catalog.json"), CATALOG)?;
    MockResponsesConfig::new(&server.uri())
        .with_extra_config(
            "[features.cue_activation]\nenabled = true\nmode = \"advisory\"\ncatalog_path = \"cue-catalog.json\"",
        )
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_json_logging("warn,codex_cue_activation_extension=info")
        .build_initialized()
        .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;

    app.start_turn_and_wait_for_completion(TurnStartParams {
        thread_id: thread.id.clone(),
        input: vec![UserInput::Text {
            text: REQUEST.into(),
            text_elements: Vec::new(),
        }],
        ..Default::default()
    })
    .await?;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 3);
    let invalid_output = requests[1]
        .function_call_output_text(INVALID_CALL_ID)
        .expect("invalid assessment should return a repair message to the model");
    assert_eq!(
        invalid_output,
        "Invalid cue outcome arguments. Use only the documented enum values, then call once."
    );
    assert!(!invalid_output.contains(INVALID_ENUM_SENTINEL));
    assert_eq!(
        requests[2]
            .function_call_output_text(VALID_CALL_ID)
            .as_deref(),
        Some(r#"{"status":"recorded"}"#)
    );

    let events = app
        .wait_for_json_log_events("codex.cue_activation.assessment", 2)
        .await?;
    assert_eq!(events[0]["fields"]["status"], "invalid");
    assert_eq!(events[0]["fields"]["reason"], "invalid_arguments");
    assert!(events[0]["fields"].get("error").is_none());
    assert!(!serde_json::to_string(&events[0])?.contains(INVALID_ENUM_SENTINEL));
    assert_eq!(events[1]["fields"]["status"], "recorded");
    assert_eq!(events[1]["fields"]["thread_id"], thread.id);
    Ok(())
}

#[tokio::test]
async fn advisory_cue_fails_loud_when_the_report_is_missing() -> Result<()> {
    let server = responses::start_mock_server().await;
    responses::mount_sse_once(
        &server,
        create_final_assistant_message_sse_response("done")?,
    )
    .await;
    let home = TempDir::new()?;
    fs::write(home.path().join("cue-catalog.json"), CATALOG)?;
    MockResponsesConfig::new(&server.uri())
        .with_extra_config(
            "[features.cue_activation]\nenabled = true\nmode = \"advisory\"\ncatalog_path = \"cue-catalog.json\"",
        )
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_json_logging("warn,codex_cue_activation_extension=info")
        .build_initialized()
        .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;

    let _: TurnStartResponse = app
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![UserInput::Text {
                    text: REQUEST.into(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            },
        })
        .await?;
    let warning = timeout(DEFAULT_READ_TIMEOUT, async {
        let mut cue_warning = None;
        loop {
            let message = app.read_next_message().await?;
            let JSONRPCMessage::Notification(notification) = message else {
                continue;
            };
            match notification.method.as_str() {
                "warning" => {
                    let Some(params) = notification.params else {
                        continue;
                    };
                    let warning: WarningNotification = serde_json::from_value(params)?;
                    if warning.message.contains("self-assessment was not recorded") {
                        cue_warning = Some(warning);
                    }
                }
                "turn/completed" => {
                    break cue_warning.ok_or_else(|| {
                        anyhow::anyhow!(
                            "turn/completed arrived before the cue-specific missing-assessment warning"
                        )
                    });
                }
                _ => {}
            }
        }
    })
    .await??;
    assert_eq!(warning.thread_id.as_deref(), Some(thread.id.as_str()));

    let events = app
        .wait_for_json_log_events("codex.cue_activation.assessment", 1)
        .await?;
    assert_eq!(events[0]["fields"]["status"], "missing");
    assert_eq!(events[0]["fields"]["thread_id"], thread.id);
    Ok(())
}

#[tokio::test]
async fn cue_activation_shadow_receipts_simulate_advisory_cooldown() -> Result<()> {
    let server = responses::start_mock_server().await;
    let home = TempDir::new()?;
    let path = home.path().join("cue-catalog.json");
    fs::write(&path, CATALOG)?;
    MockResponsesConfig::new(&server.uri())
        .with_extra_config(
            "[features.cue_activation]\nenabled = true\nmode = \"shadow\"\ncatalog_path = \"cue-catalog.json\"",
        )
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .with_json_logging("warn,codex_cue_activation_extension=debug")
        .build_initialized()
        .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;

    let mut requests = Vec::new();
    for request in [REQUEST, "continue", "Please compare two spreadsheets"] {
        let mock = responses::mount_sse_once(
            &server,
            create_final_assistant_message_sse_response("done")?,
        )
        .await;
        app.start_turn_and_wait_for_completion(TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text {
                text: request.into(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
        requests.push(mock.single_request());
    }

    let events = app
        .wait_for_json_log_events("codex.cue_activation.decision", 3)
        .await?;
    let statuses = events
        .iter()
        .map(|event| event["fields"]["status"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        [Some("selected"), Some("cooldown"), Some("no_match")]
    );
    assert_eq!(events[0]["fields"]["matched_scope"], "current");
    assert_eq!(events[1]["fields"]["matched_scope"], "prior_1");
    assert!(events.iter().all(|event| {
        event["fields"]["thread_id"] == thread.id
            && event["fields"]["catalog_sha256"].as_str().is_some()
    }));
    assert!(
        requests
            .iter()
            .all(|request| cue_fragments(request).is_empty())
    );
    Ok(())
}

async fn capture(
    enabled: bool,
    mode: &str,
    catalog: Option<&str>,
    request: &str,
) -> Result<ResponsesRequest> {
    let server = responses::start_mock_server().await;
    let mock = responses::mount_sse_once(
        &server,
        create_final_assistant_message_sse_response("done")?,
    )
    .await;
    let home = TempDir::new()?;
    let path = home.path().join("cue-catalog.json");
    if let Some(catalog) = catalog {
        fs::write(&path, catalog)?;
    }
    MockResponsesConfig::new(&server.uri())
        .with_extra_config(&format!(
            "[features.cue_activation]\nenabled = {enabled}\nmode = \"{mode}\"\ncatalog_path = \"cue-catalog.json\""
        ))
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let thread = app.start_thread(ThreadStartParams::default()).await?.thread;
    app.start_turn_and_wait_for_completion(TurnStartParams {
        thread_id: thread.id,
        input: vec![UserInput::Text {
            text: request.into(),
            text_elements: Vec::new(),
        }],
        ..Default::default()
    })
    .await?;
    Ok(mock.single_request())
}

fn cue_fragments(request: &ResponsesRequest) -> Vec<String> {
    request
        .message_input_texts("user")
        .into_iter()
        .filter(|text| text.starts_with("<codex_internal_context source=\"cue_activation\">"))
        .collect()
}

fn stable_prompt_texts(request: &ResponsesRequest) -> Vec<String> {
    ["developer", "user"]
        .into_iter()
        .flat_map(|role| request.message_input_texts(role))
        .filter(|text| {
            !text.starts_with("<skills_instructions>") && !text.starts_with("<environment_context>")
        })
        .collect()
}

fn has_tool(request: &ResponsesRequest, name: &str) -> bool {
    request.body_json()["tools"]
        .as_array()
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some(name))
        })
}
