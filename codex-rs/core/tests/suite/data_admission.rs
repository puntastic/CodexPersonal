use codex_core::DataAdmission;
use codex_core::DataAdmissionError;
use codex_core::StartThreadOptions;
use codex_core::TurnInputRequest;
use codex_core::TurnInputSubmission;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use tokio::sync::oneshot;

fn request(text: &str) -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: text.to_owned(), text_elements: Vec::new(),
    }])
}

#[tokio::test]
async fn data_admitted_in_flight_joins_only_next_same_turn_request() -> anyhow::Result<()> {
    let (release, gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![
        vec![StreamingSseChunk { gate: Some(gate), body: responses::sse_completed("first") }],
        vec![StreamingSseChunk { gate: None, body: responses::sse_completed("second") }],
    ]).await;
    let mock = responses::start_mock_server().await;
    let test = test_codex().build_with_auto_env(&mock).await?;
    let mut config = test.config.clone();
    config.model_provider.base_url = Some(format!("{}/v1", server.uri()));
    let thread = test.thread_manager.start_thread(StartThreadOptions {
        environments: Some(vec![test.executor_environment().selection().clone()]),
        ..StartThreadOptions::new(config)
    }).await?.thread;
    let TurnInputSubmission::Started { turn_id } = thread.start_or_steer_turn(request("Start"))
        .await? else { anyhow::bail!("expected started turn") };
    tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 30), server.wait_for_request_count(/*count*/ 1)).await?;
    let frozen: Value = serde_json::from_slice(&server.requests().await[0])?;
    assert_eq!(thread.admit_data(&turn_id, "delivery-1", "helper DATA").await, Ok(DataAdmission::Queued));
    assert_eq!(thread.admit_data("wrong-turn", "delivery-2", "rejected DATA").await, Err(DataAdmissionError::ExpectedTurnMismatch));
    release.send(()).expect("response gate is live");
    let mut starts = Vec::new();
    let completed = wait_for_event(&thread, |event| {
        if let EventMsg::TurnStarted(started) = event { starts.push(started.turn_id.clone()); }
        matches!(event, EventMsg::TurnComplete(_))
    }).await;
    let EventMsg::TurnComplete(completed) = completed else { unreachable!() };
    assert_eq!(completed.turn_id, turn_id);
    assert_eq!(starts, vec![turn_id.clone()]);
    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(serde_json::from_slice::<Value>(&requests[0])?, frozen);
    let next: Value = serde_json::from_slice(&requests[1])?;
    for request in [&frozen, &next] {
        let metadata: Value = serde_json::from_str(request["client_metadata"]["x-codex-turn-metadata"].as_str().unwrap())?;
        assert_eq!(metadata["turn_id"], turn_id);
    }
    let output = json!({
        "type": "function_call_output", "name": "helper_result", "namespace": "codex_data",
        "output": "helper DATA",
    });
    assert!(!frozen["input"].as_array().unwrap().contains(&output));
    let helpers: Vec<_> = next["input"].as_array().unwrap().iter()
        .filter(|item| item["namespace"] == "codex_data").collect();
    assert_eq!(helpers, vec![&output]);
    assert!(!next.to_string().contains("rejected DATA"));
    assert_eq!(thread.admit_data(&turn_id, "delivery-2", "late DATA").await, Err(DataAdmissionError::NoActiveTurn));
    server.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn data_pending_during_pre_sampling_compaction_failure_does_not_restart_turn() -> anyhow::Result<()> {
    let (release, gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![
        vec![StreamingSseChunk { gate: None, body: responses::sse(vec![
            responses::ev_response_created("seed"),
            responses::ev_completed_with_tokens("seed", 250_000),
        ]) }],
        vec![StreamingSseChunk { gate: Some(gate), body: responses::sse_failed("compact", "server_error", "compaction failed") }],
    ]).await;
    let mock = responses::start_mock_server().await;
    let test = test_codex().build_with_auto_env(&mock).await?;
    let mut config = test.config.clone();
    config.model_provider.name = "OpenAI (test)".to_owned();
    config.model_provider.base_url = Some(format!("{}/v1", server.uri()));
    config.model_provider.supports_websockets = false;
    config.model_provider.stream_max_retries = Some(0);
    config.model_provider.request_max_retries = Some(0);
    config.model_auto_compact_token_limit = Some(200_000);
    config.model_post_turn_compact_threshold_percent = 0;
    let thread = test.thread_manager.start_thread(StartThreadOptions {
        environments: Some(vec![test.executor_environment().selection().clone()]),
        ..StartThreadOptions::new(config)
    }).await?.thread;
    thread.start_or_steer_turn(request("Seed the context usage")).await?;
    wait_for_event(&thread, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    let TurnInputSubmission::Started { turn_id } = thread.start_or_steer_turn(request("Needs pre-turn compact"))
        .await? else { anyhow::bail!("expected started turn") };
    tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 30), server.wait_for_request_count(/*count*/ 2)).await?;
    assert_eq!(thread.admit_data(&turn_id, "one", "must not restart failed compact").await, Ok(DataAdmission::Queued));
    release.send(()).expect("compaction gate is live");
    let completed = wait_for_event(&thread, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    let EventMsg::TurnComplete(completed) = completed else { unreachable!() };
    assert_eq!(completed.turn_id, turn_id);
    assert_eq!(server.requests().await.len(), 2);
    assert_eq!(thread.admit_data(&turn_id, "two", "late").await, Err(DataAdmissionError::NoActiveTurn));
    server.shutdown().await;
    Ok(())
}
