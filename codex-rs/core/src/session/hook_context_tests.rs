use super::*;
use crate::session::tests::HeldStepTask;
use crate::session::tests::make_session_and_context_with_rx;
use crate::state::TaskKind;
use codex_protocol::models::ContentItem;
use codex_protocol::protocol::TurnAbortReason;
use pretty_assertions::assert_eq;

fn contexts() -> Vec<PostToolUseContext> {
    [
        ("historical before", HookContextAffinity::Conversation),
        ("task-local candidate", HookContextAffinity::OriginatingTask),
        ("historical after", HookContextAffinity::Conversation),
    ]
    .into_iter()
    .map(|(text, affinity)| PostToolUseContext {
        text: text.into(),
        affinity,
    })
    .collect()
}

async fn held_session() -> (Arc<Session>, Arc<TurnContext>) {
    let (session, turn, _events) = make_session_and_context_with_rx().await;
    session
        .spawn_task(
            Arc::clone(&turn),
            Vec::new(),
            HeldStepTask {
                kind: TaskKind::Compact,
                finish: Arc::new(Notify::new()),
            },
        )
        .await;
    (session, turn)
}

async fn context_texts(session: &Session) -> Vec<String> {
    session
        .state
        .lock()
        .await
        .history
        .raw_items()
        .filter_map(|item| match item {
            ResponseItem::Message { role, content, .. } if role == "developer" => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|content| match content {
            ContentItem::InputText { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test_case::test_case("current")]
#[test_case::test_case("cancelled")]
#[test_case::test_case("finished")]
#[test_case::test_case("replacement")]
#[test_case::test_case("reused_context")]
#[tokio::test]
async fn delayed_hook_context_only_targets_the_original_task(change: &str) {
    let (session, turn) = held_session().await;
    let target = {
        let active = session.active_turn.lock().await;
        OriginatingTask::capture(active.as_ref().unwrap().task.as_ref().unwrap(), &turn)
    };
    match change {
        "current" => {}
        "cancelled" => session
            .active_turn
            .lock()
            .await
            .as_ref()
            .unwrap()
            .task
            .as_ref()
            .unwrap()
            .cancellation_token
            .cancel(),
        "finished" => session.abort_all_tasks(TurnAbortReason::Replaced).await,
        "replacement" | "reused_context" => {
            session.abort_all_tasks(TurnAbortReason::Replaced).await;
            let replacement = if change == "reused_context" {
                Arc::clone(&turn)
            } else {
                session
                    .new_turn_with_default_settings("replacement".into(), Default::default())
                    .await
            };
            session
                .spawn_task(
                    replacement,
                    Vec::new(),
                    HeldStepTask {
                        kind: TaskKind::Compact,
                        finish: Arc::new(Notify::new()),
                    },
                )
                .await;
        }
        _ => unreachable!("test case"),
    }
    session
        .record_post_tool_use_contexts(&turn, target, contexts())
        .await;
    let expected = if change == "current" {
        vec![
            "historical before",
            "task-local candidate",
            "historical after",
        ]
    } else {
        vec!["historical before", "historical after"]
    };
    assert_eq!(context_texts(&session).await, expected);
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}

#[expect(
    clippy::await_holding_invalid_type,
    reason = "poll publication once while deliberately holding its history lock; release before awaiting completion"
)]
#[tokio::test]
async fn publication_holds_task_lock_and_rechecks_cancellation_after_history_lock_wait() {
    let (session, turn) = held_session().await;
    let (target, cancellation) = {
        let active = session.active_turn.lock().await;
        let task = active.as_ref().unwrap().task.as_ref().unwrap();
        (
            OriginatingTask::capture(task, &turn),
            task.cancellation_token.clone(),
        )
    };
    let state = session.state.lock().await;
    let publication = session.record_post_tool_use_contexts(&turn, target, contexts());
    tokio::pin!(publication);
    assert!(futures::poll!(&mut publication).is_pending());
    assert!(
        session.active_turn.try_lock().is_err(),
        "task lock must cover the history-lock wait"
    );
    cancellation.cancel();
    drop(state);
    publication.await;
    assert_eq!(
        context_texts(&session).await,
        vec!["historical before", "historical after"]
    );
    assert!(session.active_turn.try_lock().is_ok());
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}

#[tokio::test]
async fn delayed_callback_cannot_capture_replacement_before_its_dispatch() {
    let (session, turn) = held_session().await;
    // Production capture happens before a cell or a queued tool can retain this step.
    let step = session
        .capture_step_context(
            Arc::clone(&turn),
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("capture original step");
    assert!(step.hook_context_target.is_some());
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
    session
        .spawn_task(
            Arc::clone(&turn),
            Vec::new(),
            HeldStepTask {
                kind: TaskKind::Compact,
                finish: Arc::new(Notify::new()),
            },
        )
        .await;
    // The callback reaches the registry only now, with the same context and ID
    // currently active again. It must carry the old identity, not recapture.
    session
        .record_post_tool_use_contexts(&step.turn, step.hook_context_target.clone(), contexts())
        .await;
    assert_eq!(
        context_texts(&session).await,
        vec!["historical before", "historical after"]
    );
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}
