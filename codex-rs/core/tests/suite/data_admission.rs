use anyhow::Context;
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
use test_case::test_case;
use tokio::sync::oneshot;

fn request(text: &str) -> TurnInputRequest {
    TurnInputRequest::user_input(vec![UserInput::Text {
        text: text.to_owned(),
        text_elements: Vec::new(),
    }])
}

#[derive(Clone, Copy)]
enum ToolOutputPolicy {
    Default,
    Low,
}

#[test_case(ToolOutputPolicy::Default; "default_policy_keeps_data_intact")]
#[test_case(ToolOutputPolicy::Low; "low_policy_still_truncates_admitted_data")]
#[tokio::test]
async fn data_admitted_in_flight_joins_only_next_same_turn_request(
    policy: ToolOutputPolicy,
) -> anyhow::Result<()> {
    let (release, gate) = oneshot::channel();
    let (server, _completions) = start_streaming_sse_server(vec![
        vec![StreamingSseChunk {
            gate: Some(gate),
            body: responses::sse_completed("first"),
        }],
        vec![StreamingSseChunk {
            gate: None,
            body: responses::sse_completed("second"),
        }],
    ])
    .await;
    let mock = responses::start_mock_server().await;
    let test = test_codex().build_with_auto_env(&mock).await?;
    let mut config = test.config.clone();
    config.model_provider.base_url = Some(format!("{}/v1", server.uri()));
    if matches!(policy, ToolOutputPolicy::Low) {
        config.tool_output_token_limit = Some(50);
    }
    let data = "L".repeat(/*n*/ 2048) + &"R".repeat(/*n*/ 2048);
    let expected_output = match policy {
        ToolOutputPolicy::Default => data.clone(),
        // Existing 20% serialization allowance: 60 approximate tokens retain
        // 240 bytes. This witnesses current clipping, not actual token counts.
        ToolOutputPolicy::Low => format!(
            "{}\u{2026}964 tokens truncated\u{2026}{}",
            "L".repeat(/*n*/ 120),
            "R".repeat(/*n*/ 120)
        ),
    };
    let thread = test
        .thread_manager
        .start_thread(StartThreadOptions {
            environments: Some(vec![test.executor_environment().selection().clone()]),
            ..StartThreadOptions::new(config)
        })
        .await?
        .thread;
    let TurnInputSubmission::Started { turn_id } =
        thread.start_or_steer_turn(request("Start")).await?
    else {
        anyhow::bail!("expected started turn")
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(/*secs*/ 30),
        server.wait_for_request_count(/*count*/ 1),
    )
    .await?;
    let frozen: Value = serde_json::from_slice(&server.requests().await[0])?;
    assert_eq!(
        thread.admit_data(&turn_id, "delivery-1", &data).await,
        Ok(DataAdmission::Queued)
    );
    assert_eq!(
        thread
            .admit_data("wrong-turn", "delivery-2", "rejected DATA")
            .await,
        Err(DataAdmissionError::ExpectedTurnMismatch)
    );
    release.send(()).expect("response gate is live");
    let mut starts = Vec::new();
    let completed = wait_for_event(&thread, |event| {
        if let EventMsg::TurnStarted(started) = event {
            starts.push(started.turn_id.clone());
        }
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    let EventMsg::TurnComplete(completed) = completed else {
        unreachable!()
    };
    assert_eq!(completed.turn_id, turn_id);
    assert_eq!(starts, vec![turn_id.clone()]);
    let requests = server.requests().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(serde_json::from_slice::<Value>(&requests[0])?, frozen);
    let next: Value = serde_json::from_slice(&requests[1])?;
    for request in [&frozen, &next] {
        let metadata: Value = serde_json::from_str(
            request["client_metadata"]["x-codex-turn-metadata"]
                .as_str()
                .context("canonical request turn metadata")?,
        )?;
        assert_eq!(metadata["turn_id"], turn_id);
    }
    assert!(
        frozen["input"]
            .as_array()
            .context("first request input")?
            .iter()
            .all(|item| item["namespace"] != "codex_data")
    );
    let helpers: Vec<_> = next["input"]
        .as_array()
        .context("follow-up request input")?
        .iter()
        .filter(|item| item["namespace"] == "codex_data")
        .collect();
    assert_eq!(helpers.len(), 1);
    let helper = helpers[0];
    assert!(
        helper["id"]
            .as_str()
            .context("helper item id")?
            .starts_with("fco_")
    );
    let create_time = &helper["internal_chat_message_metadata_passthrough"]["create_time"];
    assert!(create_time.as_f64().context("helper creation time")? > 0.0);
    let output = json!({
        "type": "function_call_output", "name": "helper_result", "namespace": "codex_data",
        "output": expected_output, "id": helper["id"],
        "internal_chat_message_metadata_passthrough": { "create_time": create_time, "turn_id": turn_id },
    });
    assert_eq!(helpers, vec![&output]);
    assert!(!next.to_string().contains("rejected DATA"));
    assert_eq!(
        thread.admit_data(&turn_id, "delivery-2", "late DATA").await,
        Err(DataAdmissionError::NoActiveTurn)
    );
    server.shutdown().await;
    Ok(())
}
