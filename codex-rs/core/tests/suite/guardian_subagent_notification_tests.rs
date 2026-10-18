//! Guardian denial limits close the child's approval lane without interrupting safe continuation.

use super::*;
use codex_config::config_toml::CircuitBreakAction;
use codex_core::config::Constrained;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::protocol::GuardianAssessmentStatus;
use codex_protocol::protocol::MultiAgentVersion;
use core_test_support::ThreadIdle;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use test_case::test_case;

#[test_case(CircuitBreakAction::Strict; "strict_closes_approval_lane")]
#[test_case(CircuitBreakAction::Default; "default_closes_approval_lane")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_circuit_breaker_closes_lane_without_interrupting_child(
    action: CircuitBreakAction,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    const SAFE_COMPLETION: &str = "Automatic review is closed for this turn; the denied action was not executed. I can finish the safe status report.";
    let server = start_mock_server().await;
    let mut extensions = ExtensionRegistryBuilder::new();
    extensions.thread_lifecycle_contributor(Arc::new(ThreadIdle));
    let test = test_codex()
        .with_model_info_override("test-gpt-5.1-codex", |model| {
            model.multi_agent_version = Some(MultiAgentVersion::V2);
            model.auto_review_model_override = Some(model.slug.clone());
        })
        .with_extensions(Arc::new(extensions.build()))
        .with_config(move |config| {
            for feature in [
                Feature::Collab,
                Feature::MultiAgentV2,
                Feature::GuardianApproval,
            ] {
                config
                    .features
                    .enable(feature)
                    .expect("enable test feature");
            }
            config.permissions.approval_policy = Constrained::allow_any(AskForApproval::OnRequest);
            config.approvals_reviewer = ApprovalsReviewer::AutoReview;
            config.guardian_circuit_break_action = action;
            config.model_provider.stream_max_retries = Some(0);
        })
        .build_with_auto_env(&server)
        .await?;
    let root = test.session_configured.thread_id;
    let is_root = move |request: &wiremock::Request| {
        decoded_body(request)
            .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
            .is_some_and(|body| body["client_metadata"]["thread_id"] == json!(root))
    };
    let is_worker = move |request: &wiremock::Request| {
        decoded_body(request)
            .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
            .is_some_and(|body| {
                body["client_metadata"]["x-codex-parent-thread-id"] == json!(root)
                    && body["client_metadata"]["x-openai-subagent"] != "guardian"
            })
    };
    let spawn = mount_sse_once_match(
        &server,
        is_root,
        sse(vec![
            ev_function_call_with_namespace(
                SPAWN_CALL_ID,
                MULTI_AGENT_V2_NAMESPACE,
                "spawn_agent",
                &json!({
                    "task_name": "worker", "fork_turns": "none", "message": "Run the checks."
                })
                .to_string(),
            ),
            ev_completed("spawn-worker"),
        ]),
    )
    .await;
    let parent_idle =
        mount_sse_once_match(&server, is_root, sse(vec![ev_completed("parent-idle")])).await;
    let mut calls = Vec::new();
    let mut reviews = Vec::new();
    for call in ["first", "second", "third"] {
        calls.push(
            mount_sse_once_match(
                &server,
                is_worker,
                sse(vec![
                    ev_function_call(
                        call,
                        "exec_command",
                        &json!({
                            "cmd": format!("echo {call}"),
                            "sandbox_permissions": "require_escalated",
                            "justification": "Exercise Guardian review."
                        })
                        .to_string(),
                    ),
                    ev_completed(call),
                ]),
            )
            .await,
        );
        reviews.push(mount_sse_once_match(
            &server,
            |request: &wiremock::Request| {
                decoded_body(request)
                    .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
                    .is_some_and(|body| body["client_metadata"]["x-openai-subagent"] == "guardian")
            },
            sse(vec![
                ev_assistant_message(
                    "denied",
                    r#"{"risk_level":"high","user_authorization":"low","outcome":"deny","rationale":"Action is not authorized."}"#,
                ),
                ev_completed("review-denied"),
            ]),
        ).await);
    }
    let fourth = mount_sse_once_match(
        &server,
        is_worker,
        sse(vec![
            ev_function_call(
                "fourth",
                "exec_command",
                &json!({
                    "cmd": "echo fourth",
                    "sandbox_permissions": "require_escalated",
                    "justification": "Verify the closed approval lane rejects without review."
                })
                .to_string(),
            ),
            ev_completed("fourth"),
        ]),
    )
    .await;
    let worker_completed = mount_sse_once_match(
        &server,
        is_worker,
        sse(vec![
            ev_assistant_message("safe-completion", SAFE_COMPLETION),
            ev_completed("worker-completed"),
        ]),
    )
    .await;

    let mut created = test.thread_manager.subscribe_thread_created();
    test.submit_text_turn("Spawn a worker to run the checks.")
        .await?;
    ThreadIdle::wait(&test.codex).await;
    let worker = test
        .thread_manager
        .get_thread(created.recv().await?)
        .await?;
    let mut warnings = Vec::new();
    let mut assessments = Vec::new();
    let mut safe_completion_reached = false;
    loop {
        match wait_for_event(&worker, |_| true).await {
            EventMsg::Warning(warning) => warnings.push(warning.message),
            EventMsg::GuardianAssessment(assessment) => assessments.push(assessment),
            EventMsg::AgentMessage(message) if message.message == SAFE_COMPLETION => {
                safe_completion_reached = true;
            }
            EventMsg::TurnAborted(aborted) => {
                panic!("approval lane closure unexpectedly interrupted the child: {aborted:?}");
            }
            EventMsg::Error(error) => panic!("child continuation failed: {error:?}"),
            EventMsg::TurnComplete(_) => break,
            _ => {}
        }
    }
    ThreadIdle::wait(&worker).await;
    assert!(
        safe_completion_reached,
        "child did not finish its safe update"
    );
    assert_eq!(
        worker.agent_status().await,
        AgentStatus::Completed(Some(SAFE_COMPLETION.to_string())),
    );
    assert_eq!(
        assessments
            .iter()
            .map(|event| event.status)
            .collect::<Vec<_>>(),
        vec![
            GuardianAssessmentStatus::InProgress,
            GuardianAssessmentStatus::Denied,
            GuardianAssessmentStatus::InProgress,
            GuardianAssessmentStatus::Denied,
            GuardianAssessmentStatus::InProgress,
            GuardianAssessmentStatus::Denied,
        ],
        "the fourth request must not start another Guardian review",
    );
    for pair in assessments.chunks_exact(2) {
        assert_eq!(pair[0].id, pair[1].id);
    }
    let closure_warnings = warnings
        .iter()
        .filter(|warning| warning.contains("Automatic approval lane closed for this turn"))
        .collect::<Vec<_>>();
    assert_eq!(closure_warnings.len(), 1);
    let warning = closure_warnings[0];
    assert!(warning.contains("3 consecutive, 3 in the last 50 reviews"));
    assert!(warning.contains("Nothing from this request was executed"));
    assert!(warning.contains("the turn remains active"));
    assert!(warnings.iter().any(|warning| {
        warning.contains("Automatic approval lane is closed for this turn")
            && warning.contains("rejected exec_command request without review")
    }));
    let threshold_rejection = fourth
        .function_call_output_text("third")
        .expect("third denial reaches the child model");
    assert!(threshold_rejection.contains("closed for the remainder of this turn"));
    assert!(threshold_rejection.contains("In your next user-visible update"));
    let fourth_rejection = worker_completed
        .function_call_output_text("fourth")
        .expect("closed-lane rejection reaches the child model");
    assert!(fourth_rejection.contains("closed for the remainder of this turn"));
    assert!(fourth_rejection.contains("nothing was executed"));

    // Ordinary completion is delivered to the parent for either action setting.
    // If mailbox delivery is pending, wait_agent observes it before the next request.
    let _wait = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            is_root(request) && !body_contains(request, SAFE_COMPLETION)
        },
        sse(vec![
            ev_function_call_with_namespace(
                "wait-worker",
                MULTI_AGENT_V2_NAMESPACE,
                "wait_agent",
                "{}",
            ),
            ev_completed("parent-wait"),
        ]),
    )
    .await;
    let continued = mount_sse_once_match(
        &server,
        move |request: &wiremock::Request| {
            is_root(request) && body_contains(request, SAFE_COMPLETION)
        },
        sse(vec![ev_completed("parent-continued")]),
    )
    .await;
    test.submit_text_turn("Report the worker's status.").await?;
    let continued_request = continued.single_request();
    let messages = continued_request.inputs_of_type("agent_message");
    let expected = vec![json!({
        "type": "agent_message",
        "author": "/root/worker",
        "recipient": "/root",
        "content": [{
            "type": "input_text",
            "text": format!(
                "Message Type: FINAL_ANSWER\nTask name: /root\nSender: /root/worker\nPayload:\n{SAFE_COMPLETION}"
            ),
        }],
    })];
    assert_eq!(
        strip_response_item_ids_from_json(strip_metadata_from_json(json!(messages))),
        json!(expected),
    );
    assert_eq!(
        spawn.single_request().body_json()["client_metadata"]["thread_id"],
        json!(root)
    );
    assert!(parent_idle.requests().iter().any(|request| {
        request.body_json()["client_metadata"]["thread_id"] == json!(root)
            && request.function_call_output(SPAWN_CALL_ID)["output"].is_string()
    }));
    assert_eq!(
        reviews
            .iter()
            .map(|review| review
                .requests()
                .into_iter()
                .filter(|request| {
                    request.body_json()["client_metadata"]["x-openai-subagent"] == "guardian"
                })
                .count())
            .collect::<Vec<_>>(),
        vec![1, 1, 1],
    );
    assert_eq!(
        calls
            .iter()
            .map(|call| call
                .requests()
                .into_iter()
                .filter(|request| {
                    request.body_json()["client_metadata"]["x-codex-parent-thread-id"]
                        == json!(root)
                        && request.body_json()["client_metadata"]["x-openai-subagent"] != "guardian"
                })
                .count())
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
    for response in [&fourth, &worker_completed] {
        assert_eq!(
            response
                .requests()
                .iter()
                .filter(|request| {
                    request.body_json()["client_metadata"]["x-codex-parent-thread-id"]
                        == json!(root)
                        && request.body_json()["client_metadata"]["x-openai-subagent"] != "guardian"
                })
                .count(),
            1,
        );
    }
    Ok(())
}
