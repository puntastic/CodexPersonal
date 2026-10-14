//! Native nested callback regression; this inherits the hooks suite's Unix boundary.
use super::*;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolLifecycleContributor;
use codex_extension_api::ToolLifecycleFuture;
use pretty_assertions::assert_eq;
use std::sync::Mutex;
use tokio::sync::oneshot;

struct AfterPostHook(Mutex<Option<oneshot::Sender<()>>>);

impl ToolLifecycleContributor for AfterPostHook {
    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            if input.tool_name.name == "exec_command"
                && matches!(
                    input.source,
                    codex_extension_api::ToolCallSource::CodeMode { .. }
                )
                && let Some(tx) = self.0.lock().unwrap().take()
            {
                let _ = tx.send(());
            }
        })
    }
}

#[test_case::test_case(false, true; "current opted in")]
#[test_case::test_case(true, true; "interrupted opted in")]
#[test_case::test_case(true, false; "interrupted legacy")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_post_tool_context_has_only_its_declared_lifetime(
    interrupt: bool,
    scoped: bool,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let responses = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("first"),
                ev_custom_tool_call(
                    "nested-read",
                    "exec",
                    r#"text(await tools.exec_command({cmd: "printf owned-tool-output"}));"#,
                ),
                ev_completed("first"),
            ]),
            sse(vec![
                ev_response_created("second"),
                ev_assistant_message("final", "scripted final"),
                ev_completed("second"),
            ]),
        ],
    )
    .await;
    let (finished, finish_rx) = oneshot::channel();
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.tool_lifecycle_contributor(Arc::new(AfterPostHook(Mutex::new(Some(finished)))));
    let mut builder = test_codex().with_model("test-gpt-5.1-codex")
        .with_extensions(Arc::new(extensions.build()))
        .with_pre_build_hook(move |home| {
            write_post_tool_use_hook(home, Some("^Bash$"), "context", "owned-affinity-candidate").unwrap();
            let path = home.join("post_tool_use_hook.py");
            let mut script = fs::read_to_string(&path).unwrap();
            if scoped {
                script = script.replace("\"additionalContext\": reason", "\"additionalContext\": reason,\n        \"additionalContextAffinity\": \"originating_task\"");
            }
            if interrupt {
                script = script.replace("if mode == \"context\":", r#"import time
(log_path.parent / "held").touch()
deadline = time.monotonic() + 15
while not (log_path.parent / "release").exists():
    if time.monotonic() > deadline:
        raise RuntimeError("bounded hook release timed out")
    time.sleep(0.01)
if mode == "context":"#);
            }
            fs::write(path, script).unwrap();
        }).with_config(|config| {
            trust_discovered_hooks(config);
            config.features.enable(Feature::CodeMode).unwrap();
            // Exercise surviving broker callbacks, not cell termination as a substitute.
            config.features.disable(Feature::CodeModeInterrupt).unwrap();
        });
    let test = builder.build_with_auto_env(&server).await?;
    if interrupt {
        test.codex
            .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
                text: "run the owned nested read".into(),
                text_elements: Vec::new(),
            }]))
            .await?;
        fs_wait::wait_for_path_exists(test.codex_home_path().join("held"), Duration::from_secs(10))
            .await?;
        test.codex.submit(Op::Interrupt).await?;
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnAborted(_))
        })
        .await;
        fs::write(test.codex_home_path().join("release"), "ready")?;
        // Unlike HookCompleted, this observer runs after the context publication site.
        tokio::time::timeout(Duration::from_secs(10), finish_rx).await??;
        test.submit_turn("new task; do not continue the old work")
            .await?;
    } else {
        test.submit_turn("run the owned nested read").await?;
        tokio::time::timeout(Duration::from_secs(10), finish_rx).await??;
    }
    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]
            .message_input_texts("developer")
            .contains(&"owned-affinity-candidate".to_string()),
        !interrupt || !scoped
    );
    let inputs = read_post_tool_use_hook_inputs(test.codex_home_path())?;
    assert_eq!(inputs.len(), 1);
    assert!(
        inputs[0]["tool_response"]
            .to_string()
            .contains("owned-tool-output")
    );
    Ok(())
}
