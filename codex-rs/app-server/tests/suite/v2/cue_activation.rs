use std::fs;

use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::UserInput;
use core_test_support::responses;
use core_test_support::responses::ResponsesRequest;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

const REQUEST: &str = "Please review this pull request and inspect changed behavior.";
const CATALOG: &str = r#"{"schema":"codex.cue-header-catalog.v0","cues":[{"id":"synthetic-pr-review","title":"Synthetic pull-request review cue","source":"fixture://synthetic/cue-pr-review-v0","task_shape":["review pull request"],"risk":"A review may mistake description-writing for change inspection.","discriminator":"Is the task to inspect changed behavior rather than author its description?","release_when":"The changed behavior and nearest regression have both been checked.","avoid_when":["pull request description"],"provenance":{"status":"test_fixture"}}]}"#;

#[tokio::test]
async fn cue_activation_public_modes_are_bounded_and_fail_open() -> Result<()> {
    let advisory = capture(true, "advisory", Some(CATALOG)).await?;
    let cues = cue_fragments(&advisory);
    assert_eq!(cues.len(), 1);
    assert!(cues[0].contains("fixture://synthetic/cue-pr-review-v0"));
    assert!(!cues[0].contains("description-writing"));

    let shadow = capture(true, "shadow", Some(CATALOG)).await?;
    let off = capture(false, "shadow", Some(CATALOG)).await?;
    assert!(cue_fragments(&shadow).is_empty() && cue_fragments(&off).is_empty());
    assert_eq!(stable_prompt_texts(&shadow), stable_prompt_texts(&off));

    for catalog in [Some("{"), None] {
        let failed_open = capture(true, "advisory", catalog).await?;
        assert!(cue_fragments(&failed_open).is_empty());
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

async fn capture(enabled: bool, mode: &str, catalog: Option<&str>) -> Result<ResponsesRequest> {
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
            text: REQUEST.into(),
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
