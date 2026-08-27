use codex_protocol::ResponseItemId;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadMemoryMode;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnContextItem;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::user_input::UserInput;
use codex_rollout::CompactedHistoryEntry;
use codex_rollout::CompactedItem;
use codex_rollout::RolloutItem;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::super::LocalThreadStore;
use super::super::test_support::test_config;
use crate::AppendThreadItemsParams;
use crate::ArchiveThreadParams;
use crate::CreateThreadParams;
use crate::DeleteThreadParams;
use crate::ListTurnsParams;
use crate::LoadThreadHistoryParams;
use crate::RevertThreadParams;
use crate::SortDirection;
use crate::StoredTurnItemsView;
use crate::ThreadPersistenceMetadata;
use crate::ThreadStore;

#[tokio::test]
async fn revert_keeps_thread_id_and_hides_suffix_across_repeated_reverts() {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let state_db = codex_state::StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("initialize state database");
    let store = LocalThreadStore::new(config, Some(state_db.clone()));
    let thread_id = ThreadId::new();
    create_paginated_thread(&store, thread_id, ThreadHistoryMode::Paginated).await;
    store
        .append_items(AppendThreadItemsParams {
            thread_id,
            items: vec![
                turn_started("turn-1"),
                turn_completed("turn-1"),
                turn_started("turn-2"),
                turn_completed("turn-2"),
            ],
        })
        .await
        .expect("append turns");
    let original_path = store
        .live_rollout_path(thread_id)
        .await
        .expect("source rollout path");
    store
        .shutdown_thread(thread_id)
        .await
        .expect("close source writer");
    codex_rollout::state_db::reconcile_rollout(
        Some(state_db.as_ref()),
        original_path.as_path(),
        "test-provider",
        /*builder*/ None,
        &[],
        /*archived_only*/ Some(false),
        /*new_thread_memory_mode*/ None,
    )
    .await;
    compress_rollout(original_path.as_path());

    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-2".to_string(),
        })
        .await
        .expect("revert before second turn");
    let first_replacement_path = state_db
        .get_thread(thread_id)
        .await
        .expect("read metadata")
        .expect("thread metadata")
        .rollout_path;
    assert_ne!(first_replacement_path, original_path);
    assert_ne!(
        codex_rollout::rollout_id_from_path(first_replacement_path.as_path()),
        Some(thread_id)
    );
    let replacement_meta = codex_rollout::read_session_meta_line(first_replacement_path.as_path())
        .await
        .expect("read replacement metadata")
        .meta;
    assert_eq!(replacement_meta.id, thread_id);
    assert_eq!(replacement_meta.memory_mode, None);
    assert_eq!(turn_ids(&store, thread_id).await, vec!["turn-1"]);

    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-1".to_string(),
        })
        .await
        .expect("revert before first turn");
    assert_eq!(turn_ids(&store, thread_id).await, Vec::<String>::new());

    store
        .archive_thread(ArchiveThreadParams { thread_id })
        .await
        .expect("archive reverted thread");
    assert!(
        rollout_paths_for_thread(home.path(), thread_id)
            .await
            .iter()
            .all(|path| path.starts_with(home.path().join("archived_sessions")))
    );
    store
        .unarchive_thread(ArchiveThreadParams { thread_id })
        .await
        .expect("unarchive reverted thread");
    let owned_rollout_paths = rollout_paths_for_thread(home.path(), thread_id).await;
    assert_eq!(owned_rollout_paths.len(), 3);
    assert!(
        owned_rollout_paths
            .iter()
            .all(|path| path.starts_with(home.path().join("sessions")))
    );

    store
        .delete_thread(DeleteThreadParams { thread_id })
        .await
        .expect("delete reverted thread");
    for rollout_path in owned_rollout_paths {
        assert!(!rollout_path.exists());
    }
}

#[tokio::test]
async fn revert_preserves_refs_v1_and_resolves_bounded_model_context() {
    let home = TempDir::new().expect("temp dir");
    let config = test_config(home.path());
    let state_db = codex_state::StateRuntime::init(
        config.sqlite.clone(),
        config.default_model_provider_id.clone(),
    )
    .await
    .expect("initialize state database");
    let store = LocalThreadStore::new(config, Some(state_db.clone()));
    let thread_id = ThreadId::new();
    create_paginated_thread(&store, thread_id, ThreadHistoryMode::PaginatedRefsV1).await;

    let selected_source = source_response("selected-source", "selected source");
    let selected_turn_started = turn_started("turn-1");
    let selected_turn_context = RolloutItem::TurnContext(TurnContextItem {
        turn_id: Some("turn-1".to_string()),
        cwd: serde_json::from_value(serde_json::json!(home.path())).expect("absolute cwd"),
        workspace_roots: None,
        current_date: None,
        timezone: None,
        approval_policy: AskForApproval::Never,
        approvals_reviewer: None,
        sandbox_policy: SandboxPolicy::new_read_only_policy(),
        permission_profile: None,
        active_permission_profile: None,
        network: None,
        file_system_sandbox_policy: None,
        model: "test-model".to_string(),
        comp_hash: None,
        personality: None,
        collaboration_mode: None,
        multi_agent_version: None,
        multi_agent_mode: None,
        realtime_active: None,
        cyber_access_program: None,
        effort: None,
        summary: ReasoningSummary::Auto,
    });
    let selected_checkpoint = RolloutItem::Compacted(CompactedItem {
        message: "selected checkpoint".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
            item_id: "selected-source".to_string(),
        }]),
        mcp_resource_origins: None,
        window_number: Some(1),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    });
    let selected_source_envelope = match &selected_source {
        RolloutItem::ResponseItem(envelope) => envelope.clone(),
        _ => unreachable!("source_response always produces a response item"),
    };
    let selected_turn_completed = turn_completed("turn-1");
    store
        .append_items(AppendThreadItemsParams {
            thread_id,
            items: vec![
                source_response("unrelated-source", "must be dropped by the bounded scan"),
                selected_source.clone(),
                selected_turn_started.clone(),
                RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
                    thread_id,
                    turn_id: "turn-1".to_string(),
                    item: TurnItem::UserMessage(UserMessageItem {
                        id: "user-turn-1".to_string(),
                        client_id: None,
                        content: vec![UserInput::Text {
                            text: "selected user turn".to_string(),
                            text_elements: Vec::new(),
                        }],
                    }),
                    started_at_ms: Some(0),
                    completed_at_ms: 0,
                })),
                selected_turn_context.clone(),
                selected_checkpoint.clone(),
                selected_turn_completed.clone(),
                turn_started("turn-2"),
                turn_completed("turn-2"),
            ],
        })
        .await
        .expect("append reference-backed history");
    let original_path = store
        .live_rollout_path(thread_id)
        .await
        .expect("source rollout path");
    let original_rollout_id =
        codex_rollout::rollout_id_from_path(original_path.as_path()).expect("source rollout id");
    store
        .shutdown_thread(thread_id)
        .await
        .expect("close source writer");
    codex_rollout::state_db::reconcile_rollout(
        Some(state_db.as_ref()),
        original_path.as_path(),
        "test-provider",
        /*builder*/ None,
        &[],
        /*archived_only*/ Some(false),
        /*new_thread_memory_mode*/ None,
    )
    .await;

    store
        .revert_thread(RevertThreadParams {
            thread_id,
            before_turn_id: "turn-2".to_string(),
        })
        .await
        .expect("revert reference-backed thread");
    let replacement_path = state_db
        .get_thread(thread_id)
        .await
        .expect("read replacement metadata")
        .expect("replacement metadata")
        .rollout_path;
    let replacement_session_meta = codex_rollout::read_session_meta_line(&replacement_path)
        .await
        .expect("read replacement session metadata");
    assert_eq!(
        replacement_session_meta.meta.history_mode,
        ThreadHistoryMode::PaginatedRefsV1
    );
    let history_base = replacement_session_meta
        .meta
        .history_base
        .as_ref()
        .expect("replacement retains a frozen source prefix");
    assert_eq!(history_base.thread_id, original_rollout_id);

    let context = store
        .load_latest_model_context(LoadThreadHistoryParams {
            thread_id,
            include_archived: false,
        })
        .await
        .expect("load reference-backed bounded model context");
    let expected = vec![
        RolloutItem::SessionMeta(replacement_session_meta),
        selected_source,
        selected_turn_started,
        selected_turn_context,
        selected_checkpoint,
        selected_turn_completed,
    ];
    assert_eq!(
        serde_json::to_value(&context.items).expect("serialize reconstructed context"),
        serde_json::to_value(&expected).expect("serialize expected context")
    );

    let mut materialized_items = context.items;
    let mut resolver = codex_rollout::CompactedHistoryResolver::default();
    for item in &mut materialized_items {
        resolver
            .materialize_item(item)
            .expect("selected checkpoint reference resolves from the bounded replay");
    }
    let resolved_checkpoint = materialized_items
        .iter()
        .find_map(|item| match item {
            RolloutItem::Compacted(compacted) if compacted.message == "selected checkpoint" => {
                Some(compacted)
            }
            _ => None,
        })
        .expect("selected checkpoint remains in reconstructed context");
    assert_eq!(resolved_checkpoint.replacement_history_entries, None);
    assert_eq!(
        resolved_checkpoint.replacement_history.as_deref(),
        Some(std::slice::from_ref(&selected_source_envelope))
    );
}

async fn rollout_paths_for_thread(
    home: &std::path::Path,
    thread_id: ThreadId,
) -> Vec<std::path::PathBuf> {
    codex_rollout::RolloutReferenceIndex::scan(home)
        .await
        .expect("scan rollout references")
        .rollouts_for_thread(thread_id)
        .map(|(_, path)| path.to_path_buf())
        .collect()
}

async fn create_paginated_thread(
    store: &LocalThreadStore,
    thread_id: ThreadId,
    history_mode: ThreadHistoryMode,
) {
    store
        .create_thread(CreateThreadParams {
            session_id: thread_id.into(),
            thread_id,
            extra_config: None,
            forked_from_id: None,
            parent_thread_id: None,
            source: SessionSource::Exec,
            thread_source: None,
            originator: "test_originator".to_string(),
            base_instructions: BaseInstructions::default(),
            dynamic_tools: Vec::new(),
            selected_capability_roots: Vec::new(),
            multi_agent_version: None,
            history_mode,
            history_base: None,
            subagent_history_start_ordinal: None,
            initial_window_id: "window-1".to_string(),
            metadata: ThreadPersistenceMetadata {
                cwd: Some(std::env::current_dir().expect("cwd")),
                model_provider: "test-provider".to_string(),
                memory_mode: ThreadMemoryMode::Enabled,
            },
        })
        .await
        .expect("create paginated thread");
}

async fn turn_ids(store: &LocalThreadStore, thread_id: ThreadId) -> Vec<String> {
    store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::NotLoaded,
        })
        .await
        .expect("list turns")
        .turns
        .into_iter()
        .map(|turn| turn.turn_id)
        .collect()
}

fn turn_started(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_id.to_string(),
        trace_id: None,
        started_at: Some(10),
        model_context_window: None,
        collaboration_mode_kind: Default::default(),
    }))
}

fn turn_completed(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
        turn_id: turn_id.to_string(),
        last_agent_message: None,
        error: None,
        started_at: Some(10),
        completed_at: Some(20),
        duration_ms: Some(10_000),
        time_to_first_token_ms: None,
    }))
}

fn source_response(item_id: &str, text: &str) -> RolloutItem {
    RolloutItem::ResponseItem(
        ResponseItem::Message {
            id: Some(ResponseItemId::from_server(item_id.to_string())),
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
        .into(),
    )
}

fn compress_rollout(path: &std::path::Path) {
    let contents = std::fs::read(path).expect("read rollout");
    let compressed = zstd::stream::encode_all(contents.as_slice(), 3).expect("compress rollout");
    std::fs::write(path.with_extension("jsonl.zst"), compressed).expect("write compressed rollout");
    std::fs::remove_file(path).expect("remove plain rollout");
}
