use anyhow::Result;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::test_codex::TestCodexHarness;
use core_test_support::test_codex::test_codex;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_argv_executes_through_the_normal_tool_loop() -> Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_model("gpt-5.4")).await?;
    let arguments = json!({"argv":["git","--version"],"yield_time_ms":1000});
    let requests = mount_sse_sequence(harness.server(), vec![
        sse(vec![ev_response_created("argv-start"), ev_function_call("argv-git", "exec_command", &arguments.to_string()), ev_completed("argv-start")]),
        sse(vec![ev_completed("argv-done")]),
    ]).await;
    harness.submit_with_permission_profile("inspect Git version", PermissionProfile::Disabled).await?;
    let output = harness.function_call_stdout("argv-git").await;
    assert!(output.contains("git version"), "unexpected direct-program result: {output}");
    let request = requests.requests()[0].body_json();
    let tool = request["tools"].as_array().expect("tools").iter()
        .find(|tool| tool["name"] == "exec_command").expect("exec_command");
    assert!(tool["parameters"]["properties"].get("argv").is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicting_input_modes_do_not_execute_either_command() -> Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_model("gpt-5.4")).await?;
    let arguments = json!({"cmd":"echo wrong > conflicting-mode-ran.txt", "argv":["git","--version"]});
    mount_sse_sequence(harness.server(), vec![
        sse(vec![ev_response_created("conflict-start"), ev_function_call("argv-conflict", "exec_command", &arguments.to_string()), ev_completed("conflict-start")]),
        sse(vec![ev_completed("conflict-done")]),
    ]).await;
    harness.submit_with_permission_profile("check conflicting input rejection", PermissionProfile::Disabled).await?;
    let output = harness.function_call_stdout("argv-conflict").await;
    assert!(output.contains("provide exactly one of cmd"), "unexpected conflict result: {output}");
    assert!(!harness.path_exists("conflicting-mode-ran.txt").await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_argv_keeps_the_existing_permission_rejection() -> Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_model("gpt-5.4")).await?;
    let arguments = json!({"argv":["git","--version"], "sandbox_permissions":"require_escalated", "justification":"permission regression fixture"});
    mount_sse_sequence(harness.server(), vec![
        sse(vec![ev_response_created("permission-start"), ev_function_call("argv-permission", "exec_command", &arguments.to_string()), ev_completed("permission-start")]),
        sse(vec![ev_completed("permission-done")]),
    ]).await;
    harness.test().submit_turn_with_approval_and_permission_profile("check unchanged permission boundary", AskForApproval::Never, PermissionProfile::Disabled).await?;
    let output = harness.function_call_stdout("argv-permission").await;
    assert!(output.contains("approval policy") && output.contains("reject command"), "unexpected permission result: {output}");
    assert!(!output.contains("git version"));
    Ok(())
}
