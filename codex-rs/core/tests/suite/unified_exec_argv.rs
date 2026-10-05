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
use core_test_support::TestTargetOs;
use core_test_support::test_target_os;
use pretty_assertions::assert_eq;
use serde_json::json;

fn python_program() -> &'static str {
    match test_target_os() {
        TestTargetOs::Windows => "python",
        TestTargetOs::Linux | TestTargetOs::MacOs => "python3",
    }
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_bash_launches_bash_on_windows() -> Result<()> {
    if core_test_support::is_remote_test_environment() {
        eprintln!("local Windows shell-discovery fixture; remote shell selection is separately constrained");
        return Ok(());
    }
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_model("gpt-5.4")).await?;
    let arguments = json!({"cmd":"printf BASH_ROUTE_OK","shell":"bash","login":false});
    mount_sse_sequence(harness.server(), vec![
        sse(vec![ev_response_created("bash-start"), ev_function_call("requested-bash", "exec_command", &arguments.to_string()), ev_completed("bash-start")]),
        sse(vec![ev_completed("bash-done")]),
    ]).await;
    harness.submit("check requested Bash really runs Bash").await?;
    let output = harness.function_call_stdout("requested-bash").await;
    assert!(output.contains("BASH_ROUTE_OK") && output.contains("Process exited with code 0"), "requested Bash did not execute: {output}");
    Ok(())
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_argv_preserves_hostile_arguments_and_nonzero_exit() -> Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_model("gpt-5.4")).await?;
    let values = ["two words", "", "\"quoted\"", "trailing\\", "$name", "%PATH%", "*", "a|b", "a;b", "line\nbreak", "café 東京"];
    let mut argv = vec![python_program(), "-c", "import json,sys; print(json.dumps(sys.argv[1:],ensure_ascii=True)); print('ARGUMENT_PROBE_STDERR',file=sys.stderr); sys.exit(7)"];
    argv.extend(values);
    let arguments = json!({"argv":argv});
    mount_sse_sequence(harness.server(), vec![
        sse(vec![ev_response_created("literal-start"), ev_function_call("argv-literals", "exec_command", &arguments.to_string()), ev_completed("literal-start")]),
        sse(vec![ev_completed("literal-done")]),
    ]).await;
    harness.submit("exercise literal arguments and a nonzero exit").await?;
    let output = harness.function_call_stdout("argv-literals").await;
    assert!(output.contains("Process exited with code 7"), "unexpected exit result: {output}");
    assert!(output.contains("ARGUMENT_PROBE_STDERR"), "stderr was lost: {output}");
    let payload = output.lines().find(|line| line.starts_with('[')).expect("JSON argument output");
    assert_eq!(serde_json::from_str::<Vec<String>>(payload)?, values);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_argv_retains_pty_input_and_session_completion() -> Result<()> {
    let harness = TestCodexHarness::with_auto_env_builder(test_codex().with_model("gpt-5.4")).await?;
    let arguments = json!({"argv":[python_program(),"-u","-c","import sys; print('ARGV_READY',flush=True); print('ARGV_RECEIVED:'+sys.stdin.readline().strip(),flush=True)"],"tty":true,"yield_time_ms":1000});
    let input = json!({"session_id":1000,"chars":"literal-input\n","yield_time_ms":1000});
    mount_sse_sequence(harness.server(), vec![
        sse(vec![ev_response_created("pty-start"), ev_function_call("argv-pty", "exec_command", &arguments.to_string()), ev_completed("pty-start")]),
        sse(vec![ev_response_created("pty-input"), ev_function_call("argv-stdin", "write_stdin", &input.to_string()), ev_completed("pty-input")]),
        sse(vec![ev_completed("pty-done")]),
    ]).await;
    harness.submit("exercise direct-program PTY input").await?;
    let started = harness.function_call_stdout("argv-pty").await;
    assert!(started.contains("ARGV_READY") && started.contains("Process running with session ID 1000"), "missing live session: {started}");
    let finished = harness.function_call_stdout("argv-stdin").await;
    assert!(finished.contains("ARGV_RECEIVED:literal-input"), "stdin did not reach the child: {finished}");
    assert!(finished.contains("Process exited with code 0"), "session did not complete: {finished}");
    Ok(())
}
