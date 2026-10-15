use super::*;
use crate::session::tests::HeldStepTask;
use crate::session::tests::make_session_and_context_with_rx;
use crate::session::turn_context::TurnContext;
use crate::state::ActiveTurn;
use codex_protocol::protocol::TurnAbortReason;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::sync::Notify;

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
        Err(DataAdmissionError::NoActiveTurn));
    *session.active_turn.lock().await = Some(ActiveTurn::default());
    assert_eq!(session.admit_data(&turn.sub_id, "one", "data").await,
        Err(DataAdmissionError::NoActiveTurn));
    assert!(session.clone_history().await.into_annotated_items().is_empty());
    assert!(!session.input_queue.has_pending_input(&session.active_turn).await);
    assert!(session.active_turn.lock().await.as_ref().unwrap().task.is_none());

    for kind in [TaskKind::Regular, TaskKind::Review, TaskKind::Compact] {
        let (session, turn) = running(kind).await;
        let before = session.clone_history().await.into_annotated_items();
        let expected = if kind == TaskKind::Regular {
            DataAdmissionError::ExpectedTurnMismatch
        } else {
            DataAdmissionError::NotRegular
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
async fn data_completion_fence_orders_admission_without_reopening_mailbox() {
    let (session, turn) = running(TaskKind::Regular).await;
    session.input_queue.defer_mailbox_delivery_to_next_turn(&session.active_turn, &turn.sub_id).await;
    assert_eq!(session.admit_data(&turn.sub_id, "one", "data").await, Ok(DataAdmission::Queued));
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
    assert_eq!(session.admit_data(&turn.sub_id, "two", "late").await, Err(DataAdmissionError::Closed));
    assert!(!session.input_queue.has_pending_input(&session.active_turn).await);
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}

#[tokio::test]
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
        let expected = if close_first { (Err(DataAdmissionError::Closed), true) }
            else { (Ok(DataAdmission::Queued), false) };
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
        (Ok(DataAdmission::Queued), Err(DataAdmissionError::Duplicate))
        | (Err(DataAdmissionError::Duplicate), Ok(DataAdmission::Queued))));
    assert_eq!(session.take_pending_data(&turn.sub_id).await.len(), 1);
    assert_eq!(session.admit_data(&turn.sub_id, "one", "data").await, Err(DataAdmissionError::Duplicate));
    for id in ["two", "three", "four"] {
        assert_eq!(session.admit_data(&turn.sub_id, id, "data").await, Ok(DataAdmission::Queued));
    }
    assert_eq!(session.admit_data(&turn.sub_id, "five", "data").await, Err(DataAdmissionError::LimitReached));
    assert_eq!(session.take_pending_data(&turn.sub_id).await.len(), 3);
    assert_eq!(session.admit_data(&turn.sub_id, "five", "data").await, Err(DataAdmissionError::LimitReached));
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}

#[tokio::test]
async fn data_cancelled_and_completion_tails_never_become_history_only_input() {
    for cancel in [false, true] {
        let (session, turn) = running(TaskKind::Regular).await;
        let before = session.clone_history().await.into_annotated_items();
        assert_eq!(session.admit_data(&turn.sub_id, "one", "unsampled").await, Ok(DataAdmission::Queued));
        let state = {
            let active = session.active_turn.lock().await;
            let active = active.as_ref().unwrap();
            if cancel {
                active.task.as_ref().unwrap().cancellation_token.cancel();
            }
            Arc::clone(&active.turn_state)
        };
        if cancel {
            assert_eq!(session.admit_data(&turn.sub_id, "two", "late").await, Err(DataAdmissionError::Cancelled));
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
        assert_eq!(session.admit_data(&turn.sub_id, &id, &text).await, Err(DataAdmissionError::InvalidInput));
    }
    assert_eq!(session.admit_data(&turn.sub_id, "one", &"é".repeat(256)).await, Ok(DataAdmission::Queued));
    session.abort_all_tasks(TurnAbortReason::Replaced).await;
}
