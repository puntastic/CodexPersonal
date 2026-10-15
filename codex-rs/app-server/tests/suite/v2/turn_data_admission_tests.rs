use anyhow::Context;
use anyhow::Result;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use app_test_support::create_final_assistant_message_sse_response;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use codex_app_server_protocol::ClientInfo;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::DynamicToolFunctionSpec;
use codex_app_server_protocol::DynamicToolSpec;
use codex_app_server_protocol::InitializeCapabilities;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ServerRequest;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::TurnAdmitDataParams;
use codex_app_server_protocol::TurnAdmitDataResponse;
use codex_app_server_protocol::TurnAdmitDataStatus;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::MockServer;

const TIMEOUT: Duration = Duration::from_secs(60);
const MALFORMED_REQUEST: &str = "Invalid turn/admitData request: expected exactly threadId, expectedTurnId, deliveryId, and text as strings";

async fn refuse(mcp: &mut TestAppServer, params: Value) -> Result<JSONRPCErrorError> {
    let id = mcp.send_request("turn/admitData", Some(params)).await?;
    Ok(timeout(
        TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(id)),
    )
    .await??
    .error)
}

#[tokio::test]
async fn turn_admit_data_requires_experimental_opt_in() -> Result<()> {
    let mut mcp = TestAppServer::builder().build().await?;
    let init = mcp
        .initialize_with_capabilities(
            ClientInfo {
                name: DEFAULT_CLIENT_NAME.to_string(),
                title: None,
                version: "0.1.0".to_string(),
            },
            Some(InitializeCapabilities::default()),
        )
        .await?;
    assert!(matches!(init, JSONRPCMessage::Response(_)));
    let error = refuse(
        &mut mcp,
        json!({"threadId":"missing", "expectedTurnId":"turn", "deliveryId":"one", "text":"DATA"}),
    )
    .await?;
    assert_eq!(error.code, -32600);
    assert!(error.message.contains("turn/admitData"));
    assert!(error.message.contains("experimentalApi"));
    Ok(())
}

#[tokio::test]
async fn turn_admit_data_rejects_idle_missing_and_malformed_without_starting() -> Result<()> {
    let server = MockServer::start().await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri()).write(home.path())?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let thread = mcp.start_thread(ThreadStartParams::default()).await?.thread;
    let params = json!({
        "threadId":thread.id, "expectedTurnId":"idle", "deliveryId":"one", "text":"private DATA"
    });
    assert_eq!(
        refuse(&mut mcp, params.clone()).await?,
        JSONRPCErrorError {
            code: -32600,
            message: "no active task".to_string(),
            data: Some(json!({"reason":"noActiveTurn"})),
        }
    );
    let mut missing_thread = params.clone();
    missing_thread["threadId"] = json!("00000000-0000-4000-8000-000000000001");
    let error = refuse(&mut mcp, missing_thread).await?;
    assert_eq!(error.code, -32600);
    assert!(error.message.starts_with("thread not found:"));
    let malformed_error = JSONRPCErrorError {
        code: -32600,
        message: MALFORMED_REQUEST.to_string(),
        data: None,
    };
    for field in ["threadId", "expectedTurnId", "deliveryId", "text"] {
        let mut missing = params.clone();
        missing.as_object_mut().context("params object")?.remove(field);
        assert_eq!(refuse(&mut mcp, missing).await?, malformed_error);
    }
    let mut extra = params.clone();
    extra["role"] = json!("developer");
    assert_eq!(refuse(&mut mcp, extra).await?, malformed_error);
    let marker = "SYNTHETIC_PRIVATE_DATA_4729";
    let mut unknown_field = params.clone();
    unknown_field[marker] = json!(marker);
    assert_eq!(refuse(&mut mcp, unknown_field).await?, malformed_error);
    for wrong_type in [json!(86753094729_u64), json!([marker]), json!({"value":marker})] {
        let mut malformed = params.clone();
        malformed["text"] = wrong_type;
        assert_eq!(refuse(&mut mcp, malformed).await?, malformed_error);
    }
    assert_eq!(refuse(&mut mcp, json!(marker)).await?, malformed_error);
    let mut oversized = params;
    oversized["text"] = json!("\u{e9}".repeat(257));
    let error = refuse(&mut mcp, oversized).await?;
    assert_eq!(error.code, -32602);
    assert_eq!(error.data, Some(json!({"reason":"invalidInput"})));
    assert!(
        server.received_requests().await.context("requests")?
            .iter().all(|request| !request.url.path().ends_with("/responses"))
    );
    Ok(())
}

#[tokio::test]
async fn turn_admit_data_queues_exact_data_on_same_turn_and_refuses_retries() -> Result<()> {
    // An outstanding dynamic-tool response holds a real regular turn open
    // without host-local shell commands, sleeps, or a live model.
    let server = create_mock_responses_server_sequence_unchecked(vec![
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_function_call("barrier-call", "barrier", "{}"),
            responses::ev_completed("resp-1"),
        ]),
        create_final_assistant_message_sse_response("Done")?,
    ])
    .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .with_provider_name("OpenAI")
        .write(home.path())?;
    let mut mcp = TestAppServer::builder()
        .with_codex_home(home.path())
        .build_initialized()
        .await?;
    let thread = mcp.start_thread(ThreadStartParams {
        dynamic_tools: Some(vec![DynamicToolSpec::Function(DynamicToolFunctionSpec {
            name: "barrier".to_string(),
            description: "Hold the test turn open".to_string(),
            input_schema: json!({"type":"object", "properties":{}, "additionalProperties":false}),
            defer_loading: false,
        })]),
        ..Default::default()
    }).await?.thread;
    let TurnStartResponse { turn } = mcp.request(|request_id| ClientRequest::TurnStart {
        request_id,
        params: TurnStartParams {
            thread_id: thread.id.clone(),
            input: vec![UserInput::Text { text: "Run the barrier".to_string(), text_elements: vec![] }],
            ..Default::default()
        },
    }).await?;
    let request = timeout(TIMEOUT, mcp.read_stream_until_request_message()).await??;
    let ServerRequest::DynamicToolCall { request_id, params: tool } = request else {
        anyhow::bail!("expected dynamic tool barrier");
    };
    assert_eq!((&tool.thread_id, &tool.turn_id), (&thread.id, &turn.id));
    let params = TurnAdmitDataParams {
        thread_id: thread.id.clone(),
        expected_turn_id: turn.id.clone(),
        delivery_id: "delivery-1".to_string(),
        text: "helper DATA".to_string(),
    };
    let mut wrong = serde_json::to_value(&params)?;
    wrong["expectedTurnId"] = json!("wrong-turn");
    assert_eq!(
        refuse(&mut mcp, wrong).await?.data,
        Some(json!({"reason":"expectedTurnMismatch"}))
    );
    let queued: TurnAdmitDataResponse = mcp.request(|request_id| ClientRequest::TurnAdmitData {
        request_id, params: params.clone(),
    }).await?;
    assert_eq!(queued, TurnAdmitDataResponse {
        status: TurnAdmitDataStatus::Queued,
        turn_id: turn.id.clone(),
        delivery_id: params.delivery_id.clone(),
    });
    assert_eq!(
        refuse(&mut mcp, serde_json::to_value(&params)?).await?.data,
        Some(json!({"reason":"duplicate"}))
    );
    mcp.send_response(request_id, json!({
        "contentItems":[{"type":"inputText", "text":"barrier released"}], "success":true
    })).await?;
    let completed = timeout(TIMEOUT, mcp.read_stream_until_notification_message("turn/completed")).await??;
    let completed = completed.params.context("completed params")?;
    assert_eq!(completed["turn"]["id"], turn.id);
    assert_eq!(
        refuse(&mut mcp, serde_json::to_value(params)?).await?.data,
        Some(json!({"reason":"noActiveTurn"}))
    );
    assert_eq!(
        mcp.pending_notification_methods().iter()
            .filter(|method| method.as_str() == "turn/started").count(),
        1
    );
    let started = timeout(TIMEOUT, mcp.read_stream_until_notification_message("turn/started")).await??;
    assert_eq!(started.params.context("started params")?["turn"]["id"], turn.id);

    let requests = server.received_requests().await.context("requests")?;
    let bodies = requests.iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .map(|request| request.body_json::<Value>())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        let metadata: Value = serde_json::from_str(
            body["client_metadata"]["x-codex-turn-metadata"].as_str()
                .context("canonical request turn metadata")?
        )?;
        assert_eq!(metadata["turn_id"], turn.id);
    }
    assert!(!bodies[0].to_string().contains("helper DATA"));
    let input = bodies[1]["input"].as_array().context("follow-up input")?;
    let data = input.iter().filter(|item| item["namespace"] == "codex_data").collect::<Vec<_>>();
    assert_eq!(data.len(), 1);
    let data = data[0];
    assert!(data["id"].as_str().is_some_and(|id| id.starts_with("fco_")));
    let metadata = &data["internal_chat_message_metadata_passthrough"];
    assert!(metadata["create_time"].as_f64().is_some_and(|time| time > 0.0));
    assert_eq!(data, &json!({
        "id":data["id"],
        "internal_chat_message_metadata_passthrough": {
            "create_time":metadata["create_time"], "turn_id":turn.id
        },
        "type":"function_call_output", "name":"helper_result", "namespace":"codex_data",
        "output":"helper DATA"
    }));
    assert!(!input.iter().any(|item| {
        (item["role"] == "user" || item["role"] == "developer")
            && item.to_string().contains("helper DATA")
    }));
    Ok(())
}
