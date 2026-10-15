use super::*;
use super::DataAdmission::Queued;
use super::DataAdmissionError as Refused;
use crate::session::tests::HeldStepTask;
use crate::session::tests::make_session_and_context_with_rx;
use crate::session::turn_context::TurnContext;
use crate::state::ActiveTurn;
use codex_protocol::protocol::TurnAbortReason;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::sync::Notify;
use codex_hooks::HookMcpCall;
use futures::future::BoxFuture;
use core_test_support::responses;
use core_test_support::streaming_sse::StreamingSseChunk;
use core_test_support::streaming_sse::start_streaming_sse_server;
use std::time::Duration;
use tokio::time::timeout;

async fn running(kind: TaskKind) -> (Arc<Session>, Arc<TurnContext>) {
    let (session, turn, _events) = make_session_and_context_with_rx().await;
    session.spawn_task(Arc::clone(&turn), Vec::new(), HeldStepTask {
        kind,
        finish: Arc::new(Notify::new()),
    }).await;
    (session, turn)
}

#[tokio::test]
async fn data_refusals_leave_queue_history_and_task_unchanged() {
    let (session, turn, _events) = make_session_and_context_with_rx().await;
    assert_eq!(session.admit_data(&turn.sub_id, "one", "data").await,
        Err(Refused::NoActiveTurn));
    *session.active_turn.lock().await = Some(ActiveTurn::default());
    assert_eq!(session.admit_data(&turn.sub_id, "one", "data").await,
        Err(Refused::NoActiveTurn));
    assert!(session.clone_history().await.into_annotated_items().is_empty());
    assert!(!session.input_queue.has_pending_input(&session.active_turn).await);
    assert!(session.active_turn.lock().await.as_ref().unwrap().task.is_none());

    for kind in [TaskKind::Regular, TaskKind::Review, TaskKind::Compact] {
        let (session, turn) = running(kind).await;
        let before = session.clone_history().await.into_annotated_items();
        let expected = if kind == TaskKind::Regular {
            Refused::ExpectedTurnMismatch
        } else {
            Refused::NotRegular
        };
        let turn_id = if kind == TaskKind::Regular { "wrong" } else { &turn.sub_id };
        assert_eq!(session.admit_data(turn_id, "one", "data").await, Err(expected));
        assert_eq!(session.clone_history().await.into_annotated_items(), before);
        assert!(!session.input_queue.has_pending_input(&session.active_turn).await);
        assert_eq!(session.active_turn.lock().await.as_ref().unwrap().task.as_ref().unwrap().turn_context.sub_id, turn.sub_id);
        session.abort_all_tasks(TurnAbortReason::Replaced).await;
    }
}

#[tokio::test]
#[expect(clippy::await_holding_invalid_type, reason = "test lock")]
async fn data_completion_fence_orders_admission_without_reopening_mailbox() {
    let (session, turn) = running(TaskKind::Regular).await;
    session.input_queue.defer_mailbox_delivery_to_next_turn(&session.active_turn, &turn.sub_id).await;
    assert_eq!(session.admit_data(&turn.sub_id, "one", "data").await, Ok(Queued));
    let (reply, decision) = tokio::sync::oneshot::channel();
    session.interrupt_turn_if_no_pending_input(&turn.sub_id, reply).await;
    assert_eq!(decision.await.unwrap(), false);
    assert!(!session.close_data_admission_if_idle(&turn.sub_id).await);
    assert!(session.input_queue.has_pending_input(&session.active_turn).await);
    assert!(session.input_queue.get_pending_input(&session.active_turn).await.0.is_empty());
    assert!(session.take_pending_data("stale-consumer").await.is_empty());
    let drained = session.take_pending_data(&turn.sub_id).await;
    assert_eq!(drained.len(), 1);
    assert!(!session.active_turn.lock().await.as_ref().unwrap().turn_state.lock().await.accepts_mailbox_delivery_for_current_turn());
    assert!(session.close_data_admission_if_idle(&turn.sub_id).await);
    assert_eq!(session.admit_data(&turn.sub_id, "two", "late").await, Err(Refused::Closed));
    assert!(!session.input_queue.has_pending_input(&session.active_turn).await);
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}

#[tokio::test]
#[expect(clippy::await_holding_invalid_type, reason = "test lock")]
async fn data_admission_and_completion_contend_on_the_same_fence() {
    for close_first in [false, true] {
        let (session, turn) = running(TaskKind::Regular).await;
        let guard = session.active_turn.lock().await;
        let admit = session.admit_data(&turn.sub_id, "one", "data");
        let close = session.close_data_admission_if_idle(&turn.sub_id);
        tokio::pin!(admit, close);
        if close_first {
            assert!(futures::poll!(close.as_mut()).is_pending());
            assert!(futures::poll!(admit.as_mut()).is_pending());
        } else {
            assert!(futures::poll!(admit.as_mut()).is_pending());
            assert!(futures::poll!(close.as_mut()).is_pending());
        }
        drop(guard);
        let result = tokio::join!(admit, close);
        let expected = if close_first { (Err(Refused::Closed), true) }
            else { (Ok(Queued), false) };
        assert_eq!(result, expected);
        session.abort_all_tasks(TurnAbortReason::Replaced).await;
    }
}

#[tokio::test]
async fn data_ids_survive_drain_and_concurrent_duplicates_and_limits_are_bounded() {
    let (session, turn) = running(TaskKind::Regular).await;
    let (first, duplicate) = tokio::join!(
        session.admit_data(&turn.sub_id, "one", "data"),
        session.admit_data(&turn.sub_id, "one", "data"),
    );
    assert!(matches!((first, duplicate),
        (Ok(Queued), Err(Refused::Duplicate))
        | (Err(Refused::Duplicate), Ok(Queued))));
    assert_eq!(session.take_pending_data(&turn.sub_id).await.len(), 1);
    assert_eq!(session.admit_data(&turn.sub_id, "one", "data").await, Err(Refused::Duplicate));
    for id in ["two", "three", "four"] {
        assert_eq!(session.admit_data(&turn.sub_id, id, "data").await, Ok(Queued));
    }
    assert_eq!(session.admit_data(&turn.sub_id, "five", "data").await, Err(Refused::LimitReached));
    assert_eq!(session.take_pending_data(&turn.sub_id).await.len(), 3);
    assert_eq!(session.admit_data(&turn.sub_id, "five", "data").await, Err(Refused::LimitReached));
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}

#[tokio::test]
async fn data_cancelled_and_completion_tails_never_become_history_only_input() {
    for cancel in [false, true] {
        let (session, turn) = running(TaskKind::Regular).await;
        let before = session.clone_history().await.into_annotated_items();
        assert_eq!(session.admit_data(&turn.sub_id, "one", "unsampled").await, Ok(Queued));
        let state = {
            let active = session.active_turn.lock().await;
            let active = active.as_ref().unwrap();
            if cancel {
                active.task.as_ref().unwrap().cancellation_token.cancel();
            }
            Arc::clone(&active.turn_state)
        };
        if cancel {
            assert_eq!(session.admit_data(&turn.sub_id, "two", "late").await, Err(Refused::Cancelled));
            assert!(session.take_pending_data(&turn.sub_id).await.is_empty());
        }
        assert!(session.input_queue.take_pending_input_for_turn_state(&state).await.is_empty());
        assert!(state.lock().await.data_admission.pending.is_empty());
        assert_eq!(session.clone_history().await.into_annotated_items(), before);
        session.abort_all_tasks(TurnAbortReason::Replaced).await;
    }
}

#[tokio::test]
async fn data_input_caps_are_utf8_bytes_and_refusals_do_not_reserve_ids() {
    let (session, turn) = running(TaskKind::Regular).await;
    for (id, text) in [("".to_owned(), "data".to_owned()), ("x".repeat(129), "data".to_owned()),
        ("one".to_owned(), String::new()), ("one".to_owned(), "é".repeat(257))] {
        assert_eq!(session.admit_data(&turn.sub_id, &id, &text).await, Err(Refused::InvalidInput));
    }
    assert_eq!(session.admit_data(&turn.sub_id, "one", &"é".repeat(256)).await, Ok(Queued));
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}

#[derive(Default)]
struct HeldStopHook { entered: Notify, release: Notify }

impl codex_hooks::HookMcpExecutor for HeldStopHook {
    fn execute(&self, _call: HookMcpCall) -> BoxFuture<'_, anyhow::Result<String>> {
        Box::pin(async {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(r#"{"continue":false,"stopReason":"test veto"}"#.to_owned())
        })
    }
}

#[tokio::test]
async fn data_pending_at_hook_veto_does_not_restart_or_enter_history() -> anyhow::Result<()> {
    let (release_response, response_gate) = tokio::sync::oneshot::channel();
    let (server, _) = start_streaming_sse_server(vec![vec![StreamingSseChunk {
        gate: Some(response_gate), body: responses::sse_completed("first"),
    }]]).await;
    let (session, turn, events) = crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
        codex_login::CodexAuth::from_api_key("Test API Key"), Vec::new(), |config| {
            config.model_provider.base_url = Some(format!("{}/v1", server.uri()));
            config.model_provider.supports_websockets = false;
            config.model_provider.stream_max_retries = Some(0);
            config.model_provider.request_max_retries = Some(0);
        },
    ).await;
    session.spawn_task(Arc::clone(&turn), Vec::new(), crate::tasks::RegularTask::new()).await;
    timeout(Duration::from_secs(/*secs*/ 30), server.wait_for_request_count(/*count*/ 1)).await?;
    let gate = Arc::new(HeldStopHook::default());
    let root = turn.config.codex_home.clone();
    let (hooks, _results) = codex_hooks::Hooks::new(codex_hooks::HooksConfig {
        feature_enabled: true, bypass_hook_trust: true,
        plugin_hook_sources: vec![codex_plugin::PluginHookSource {
            plugin_id: codex_plugin::PluginId::parse("admission-test@fixture")?,
            plugin_data_root: root.join("data"), source_path: root.join("hooks.json"),
            source_relative_path: "hooks.json".to_owned(), plugin_root: root,
            hooks: serde_json::from_value(serde_json::json!({"Stop":[{"hooks":[{
                "type":"mcp_tool", "server":"test", "tool":"veto"
            }]}]}))?,
        }],
        ..Default::default()
    }, session.thread_id(), gate.clone())?;
    session.services.hooks.store(Arc::new(hooks));
    release_response.send(()).expect("response gate is live");
    timeout(Duration::from_secs(/*secs*/ 30), gate.entered.notified()).await?;
    assert_eq!(session.admit_data(&turn.sub_id, "one", "vetoed DATA").await, Ok(Queued));
    gate.release.notify_one();
    let completed = timeout(Duration::from_secs(/*secs*/ 30), async {
        loop {
            if let codex_protocol::protocol::EventMsg::TurnComplete(completed) = events.recv().await?.msg {
                break Ok::<_, anyhow::Error>(completed);
            }
        }
    }).await??;
    assert!(completed.error.is_none());
    assert_eq!(completed.turn_id, turn.sub_id);
    assert_eq!(server.requests().await.len(), 1);
    assert!(!serde_json::to_string(&session.clone_history().await.into_annotated_items())?.contains("vetoed DATA"));
    server.shutdown().await;
    Ok(())
}
