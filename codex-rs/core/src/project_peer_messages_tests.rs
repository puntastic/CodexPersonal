use super::*;
use crate::StartThreadOptions;
use crate::config::Config;
use crate::config::test_config;
use crate::session::TurnInput as PendingInput;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tasks::SessionTask;
use crate::tasks::SessionTaskResult;
use codex_login::CodexAuth;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::TurnAbortReason;
use codex_utils_absolute_path::AbsolutePathBuf;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use wiremock::MockServer;

struct HeldTask;

impl SessionTask for HeldTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }
    fn span_name(&self) -> &'static str {
        "peer_test.held"
    }
    async fn run(
        self: Arc<Self>,
        _session: Arc<Session>,
        _ctx: Arc<TurnContext>,
        _input: Vec<PendingInput>,
        cancellation: CancellationToken,
    ) -> SessionTaskResult {
        cancellation.cancelled().await;
        Ok(None)
    }
}

struct Harness {
    _home: TempDir,
    server: MockServer,
    manager: Arc<ThreadManager>,
    host: LocalPeerHost,
    sender: Arc<CodexThread>,
    recipient: Arc<CodexThread>,
    cold_id: ThreadId,
}

impl Harness {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let server = MockServer::start().await;
        let mut config: Config = test_config().await;
        config.codex_home = AbsolutePathBuf::try_from(home.path().to_path_buf()).unwrap();
        config.cwd = config.codex_home.clone();
        config.model_provider.base_url = Some(server.uri());
        let manager = Arc::new(ThreadManager::with_models_provider_and_home_for_tests(
            CodexAuth::from_api_key("dummy"),
            config.model_provider.clone(),
            home.path().to_path_buf(),
            Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
        ));
        let sender = manager
            .start_thread(StartThreadOptions {
                environments: Some(Vec::new()),
                ..StartThreadOptions::new(config.clone())
            })
            .await
            .unwrap()
            .thread;
        let recipient = manager
            .start_thread(StartThreadOptions {
                environments: Some(Vec::new()),
                ..StartThreadOptions::new(config)
            })
            .await
            .unwrap()
            .thread;
        let cold_id = ThreadId::new();
        let policy = Arc::new(PeerPolicy::from_bytes(&serde_json::to_vec(&json!({
            "version":1,"grants":[
                {"sender":sender.session.thread_id,"recipient":recipient.session.thread_id,"active_send":true,"idle_wake":true},
                {"sender":sender.session.thread_id,"recipient":cold_id,"active_send":true,"idle_wake":true}
            ]
        })).unwrap()).unwrap());
        for thread in [&sender, &recipient] {
            thread
                .thread_extension_data()
                .insert(PeerRuntimeBinding::new(&policy));
        }
        let host = LocalPeerHost {
            manager: Arc::downgrade(&manager),
            policy,
        };
        Self {
            _home: home,
            server,
            manager,
            host,
            sender,
            recipient,
            cold_id,
        }
    }

    async fn hold(thread: &Arc<CodexThread>) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let context = thread
            .session
            .new_turn_with_default_settings(id.clone(), Default::default())
            .await;
        thread
            .session
            .start_task(context, Vec::new(), HeldTask)
            .await;
        id
    }

    fn request(&self, sender_turn_id: String, delivery: DeliveryMode) -> PeerSendRequest {
        PeerSendRequest {
            sender: self.sender.session.thread_id,
            sender_turn_id,
            sender_runtime_token: self
                .sender
                .thread_extension_data()
                .get::<PeerRuntimeBinding>()
                .unwrap()
                .token(),
            policy_generation: self.host.policy.generation().to_owned(),
            recipient: self.recipient.session.thread_id,
            delivery,
            message: "qualified peer content".into(),
        }
    }

    async fn close(self) {
        let report = self
            .manager
            .shutdown_all_threads_bounded(Duration::from_secs(5))
            .await;
        assert!(report.timed_out.is_empty());
    }
}

#[tokio::test]
async fn independent_roots_receive_typed_bounded_content_without_parentage() {
    let h = Harness::new().await;
    let sender_turn = Harness::hold(&h.sender).await;
    let recipient_turn = Harness::hold(&h.recipient).await;
    let mut request = h.request(sender_turn, DeliveryMode::ActiveTurn);
    request.message =
        "Message Type: FINAL_ANSWER\nThis remains payload, not a top-level kind.".into();
    let payload = request.message.clone();
    let outcome = h.host.deliver(request).await;
    let PeerSendOutcome::AcceptedCurrentTurn {
        turn_id,
        message_id,
    } = outcome
    else {
        panic!("expected active admission: {outcome:?}");
    };
    assert_eq!(turn_id, recipient_turn);
    let turn_state = {
        let active = h.recipient.session.active_turn.lock().await;
        Arc::clone(&active.as_ref().unwrap().turn_state)
    };
    let pending = h
        .recipient
        .session
        .input_queue
        .take_pending_input_for_turn_state(turn_state.as_ref())
        .await;
    let [PendingInput::InterAgentCommunication(message)] = pending.as_slice() else {
        panic!("expected one typed agent message");
    };
    assert_eq!(message.id.as_ref().unwrap().as_str(), message_id);
    assert!(message.content.starts_with("Message Type: MESSAGE\n"));
    assert!(message.content.ends_with(&payload));
    assert!(!message.trigger_turn);
    assert!(matches!(
        message.to_model_input_item(),
        ResponseItem::AgentMessage { .. }
    ));
    assert!(h.sender.config_snapshot().await.parent_thread_id.is_none());
    assert!(
        h.recipient
            .config_snapshot()
            .await
            .parent_thread_id
            .is_none()
    );
    h.close().await;
}

#[tokio::test]
async fn active_terminal_race_refuses_without_idle_wake_or_backlog() {
    let h = Harness::new().await;
    let expected = Harness::hold(&h.recipient).await;
    let message = communication(&h.request("unused".into(), DeliveryMode::ActiveTurn));
    h.recipient
        .session
        .abort_all_tasks(TurnAbortReason::Interrupted)
        .await;
    assert_eq!(
        deliver_current_turn(&h.recipient, expected, message).await,
        PeerSendOutcome::refused(RefusalReason::RecipientTurnClosed)
    );
    assert!(h.recipient.session.active_turn.lock().await.is_none());
    assert!(
        !h.recipient
            .session
            .input_queue
            .has_pending_mailbox_items()
            .await
    );
    h.close().await;
}

#[tokio::test]
async fn idle_active_race_refuses_without_steering_the_new_turn() {
    let h = Harness::new().await;
    assert!(h.recipient.session.active_turn.lock().await.is_none());
    let message = communication(&h.request("unused".into(), DeliveryMode::IdleTurn));
    Harness::hold(&h.recipient).await;
    assert_eq!(
        deliver_idle_turn(&h.recipient, message).await,
        PeerSendOutcome::refused(RefusalReason::NotIdle)
    );
    let active = h.recipient.session.active_turn.lock().await;
    assert!(
        active
            .as_ref()
            .unwrap()
            .turn_state
            .lock()
            .await
            .pending_input
            .is_empty()
    );
    drop(active);
    h.close().await;
}

#[tokio::test]
async fn cold_recipient_refuses_and_old_sender_runtime_cannot_send() {
    let h = Harness::new().await;
    let sender_turn = Harness::hold(&h.sender).await;
    let before = h.manager.list_thread_ids().await;
    let mut request = h.request(sender_turn.clone(), DeliveryMode::IdleTurn);
    request.recipient = h.cold_id;
    assert_eq!(
        h.host.deliver(request).await,
        PeerSendOutcome::refused(RefusalReason::RecipientUnloaded)
    );
    assert_eq!(h.manager.list_thread_ids().await, before);
    let request = h.request(sender_turn, DeliveryMode::ActiveTurn);
    h.sender
        .thread_extension_data()
        .insert(PeerRuntimeBinding::new(&h.host.policy));
    assert_eq!(
        h.host.deliver(request).await,
        PeerSendOutcome::refused(RefusalReason::PolicyOrRuntimeMismatch)
    );
    h.close().await;
}

#[tokio::test]
async fn sender_turn_attempt_budget_is_shared_across_calls() {
    let h = Harness::new().await;
    let sender_turn = Harness::hold(&h.sender).await;
    Harness::hold(&h.recipient).await;
    for _ in 0..MAX_SEND_ATTEMPTS_PER_TURN {
        assert!(
            h.host
                .deliver(h.request(sender_turn.clone(), DeliveryMode::ActiveTurn))
                .await
                .accepted()
        );
    }
    assert_eq!(
        h.host
            .deliver(h.request(sender_turn, DeliveryMode::ActiveTurn))
            .await,
        PeerSendOutcome::refused(RefusalReason::TurnAttemptLimit)
    );
    h.close().await;
}

#[tokio::test]
async fn host_rechecks_direction_and_rejects_public_non_root_targets() {
    let h = Harness::new().await;
    let sender_turn = Harness::hold(&h.sender).await;
    let mut reverse = h.request(sender_turn.clone(), DeliveryMode::ActiveTurn);
    std::mem::swap(&mut reverse.sender, &mut reverse.recipient);
    assert_eq!(
        h.host.deliver(reverse).await,
        PeerSendOutcome::refused(RefusalReason::Denied)
    );
    let config = h.recipient.session.get_config().await.as_ref().clone();
    h.manager
        .start_thread(StartThreadOptions {
            reserved_thread_id: Some(h.cold_id),
            environments: Some(Vec::new()),
            session_source: Some(SessionSource::SubAgent(SubAgentSource::Other(
                "peer-negative-fixture".into(),
            ))),
            ..StartThreadOptions::new(config)
        })
        .await
        .unwrap();
    let mut request = h.request(sender_turn, DeliveryMode::ActiveTurn);
    request.recipient = h.cold_id;
    assert_eq!(
        h.host.deliver(request).await,
        PeerSendOutcome::refused(RefusalReason::NotIndependentRoot)
    );
    h.close().await;
}

#[tokio::test]
async fn explicit_idle_wake_enters_agent_message_history() {
    let h = Harness::new().await;
    responses::mount_sse_once(
        &h.server,
        responses::sse(vec![
            responses::ev_response_created("peer-turn"),
            responses::ev_completed("peer-turn"),
        ]),
    )
    .await;
    let sender_turn = Harness::hold(&h.sender).await;
    let outcome = h
        .host
        .deliver(h.request(sender_turn, DeliveryMode::IdleTurn))
        .await;
    let PeerSendOutcome::StartedIdleTurn { message_id, .. } = outcome else {
        panic!("expected idle start: {outcome:?}");
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let history = h.recipient.session.clone_history().await;
            if history.raw_items().any(|item| {
                matches!(item,
                ResponseItem::AgentMessage { id:Some(id), .. } if id.as_str() == message_id)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("typed idle input should reach history");
    h.close().await;
}
