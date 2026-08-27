use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use codex_app_server_protocol::build_turns_from_rollout_items;
use codex_extension_items::ExtensionItem;
use codex_extension_items::image_generation::ImageGenerationFailure;
use codex_extension_items::image_generation::ImageGenerationItem;
use codex_protocol::AgentPath;
use codex_protocol::ResponseItemId;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::items::ReasoningItem;
use codex_protocol::items::TurnItem;
use codex_protocol::mcp::McpResourceOrigin;
use codex_protocol::mcp::McpResourceOriginCheckpoint;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::AgentMessageEvent;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::HistoryPosition;
use codex_protocol::protocol::ImageGenerationEndEvent;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::SessionMetaLine;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::protocol::ThreadRolledBackEvent;
use codex_protocol::protocol::TurnCompleteEvent;
use codex_protocol::protocol::TurnContextItem;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::protocol::UserMessageEvent;
use codex_rollout::CompactedHistoryEntry;
use codex_rollout::CompactedHistoryResolver;
use codex_rollout::CompactedItem;
use codex_rollout::RolloutConfig;
use codex_rollout::RolloutItem;
use codex_rollout::RolloutLine;
use codex_rollout::RolloutRecorder;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::LocalThreadStore;
use super::PreservedSourceRetirementStatus;
use super::RolloutMigrationFailureReason;
use super::RolloutMigrationMode;
use super::RolloutMigrationOptions;
use super::RolloutMigrationPaths;
use super::RolloutMigrationProgress;
use super::RolloutMigrationStatus;
use super::RolloutRecordCursor;
use super::RolloutRecordValidationError;
use super::TrailingPartialPolicy;
#[cfg(unix)]
use super::decompress_rollout_to_path;
use super::migration_journal_path;
use super::preserved_legacy_rollout_path;
use super::retained_migration_recovery_path;
use super::telemetry::RolloutMigrationTrigger;
use super::thread_history;
use super::write_migration_journal;
use crate::ItemSortKey;
use crate::ListItemsParams;
use crate::ListThreadsParams;
use crate::ListTurnsParams;
use crate::LoadThreadHistoryParams;
use crate::SortDirection;
use crate::StoredTurnItemsView;
use crate::ThreadMetadataPatch;
use crate::ThreadSortKey;
use crate::ThreadStore;
use crate::ThreadStoreError;
use crate::TurnPage;
use crate::UpdateThreadMetadataParams;
use crate::local::test_support::test_config;

const TIMESTAMP: &str = "2025-01-03T12:00:00Z";

#[test]
fn trailing_partial_policy_only_discards_truncated_legacy_json() {
    assert!(
        super::parse_rollout_record(
            br#"{"timestamp":"unterminated""#,
            /*terminated*/ false,
            br#"{"timestamp":"unterminated""#.len() as u64,
            RolloutRecordCursor::default(),
            TrailingPartialPolicy::DiscardIncomplete,
            /*require_rollout_line*/ false,
        )
        .expect("discard truncated legacy tail")
        .is_none()
    );
    assert!(matches!(
        super::parse_rollout_record(
            b"{}",
            /*terminated*/ false,
            b"{}".len() as u64,
            RolloutRecordCursor::default(),
            TrailingPartialPolicy::DiscardIncomplete,
            /*require_rollout_line*/ false,
        ),
        Err(RolloutRecordValidationError::Malformed { .. })
    ));
    assert!(matches!(
        super::parse_rollout_record(
            br#"{"timestamp":"unterminated""#,
            /*terminated*/ false,
            br#"{"timestamp":"unterminated""#.len() as u64,
            RolloutRecordCursor::default(),
            TrailingPartialPolicy::Reject,
            /*require_rollout_line*/ false,
        ),
        Err(RolloutRecordValidationError::Malformed { .. })
    ));
}

fn write_rollout(
    home: &Path,
    thread_id: ThreadId,
    source: SessionSource,
    items: Vec<RolloutItem>,
) -> PathBuf {
    write_rollout_with_fork(home, thread_id, source, /*forked_from_id*/ None, items)
}

fn write_rollout_with_fork(
    home: &Path,
    thread_id: ThreadId,
    source: SessionSource,
    forked_from_id: Option<ThreadId>,
    items: Vec<RolloutItem>,
) -> PathBuf {
    let directory = home.join("sessions/2025/01/03");
    fs::create_dir_all(&directory).expect("create rollout directory");
    let path = directory.join(format!("rollout-2025-01-03T12-00-00-{thread_id}.jsonl"));
    let mut file = fs::File::create(&path).expect("create legacy rollout");
    let metadata = SessionMeta {
        session_id: thread_id.into(),
        id: thread_id,
        forked_from_id,
        timestamp: TIMESTAMP.to_string(),
        cwd: home.to_path_buf(),
        originator: "test-originator".to_string(),
        cli_version: "0.0.0".to_string(),
        source,
        model_provider: Some("test-provider".to_string()),
        ..SessionMeta::default()
    };
    let items = std::iter::once(RolloutItem::SessionMeta(SessionMetaLine {
        meta: metadata,
        git: None,
    }))
    .chain(items);
    for item in items {
        let line = RolloutLine {
            timestamp: TIMESTAMP.to_string(),
            ordinal: None,
            item,
        };
        writeln!(
            file,
            "{}",
            serde_json::to_string(&line).expect("serialize legacy record")
        )
        .expect("write legacy record");
    }
    path
}

fn write_inline_paginated_rollout(
    home: &Path,
    thread_id: ThreadId,
    source: SessionSource,
    history_base: Option<HistoryPosition>,
    subagent_history_start_ordinal: Option<u64>,
    items: Vec<RolloutItem>,
) -> PathBuf {
    let directory = home.join("sessions/2025/01/03");
    fs::create_dir_all(&directory).expect("create rollout directory");
    let path = directory.join(format!("rollout-2025-01-03T12-00-00-{thread_id}.jsonl"));
    let mut file = fs::File::create(&path).expect("create inline paginated rollout");
    let first_ordinal = history_base.map_or(0, |base| base.end_ordinal_exclusive);
    let metadata = SessionMeta {
        session_id: thread_id.into(),
        id: thread_id,
        timestamp: TIMESTAMP.to_string(),
        cwd: home.to_path_buf(),
        originator: "test-originator".to_string(),
        cli_version: "0.0.0".to_string(),
        source,
        model_provider: Some("test-provider".to_string()),
        history_mode: ThreadHistoryMode::Paginated,
        history_base,
        subagent_history_start_ordinal,
        ..SessionMeta::default()
    };
    let items = std::iter::once(RolloutItem::SessionMeta(SessionMetaLine {
        meta: metadata,
        git: None,
    }))
    .chain(items);
    for (index, item) in items.enumerate() {
        let line = RolloutLine {
            timestamp: TIMESTAMP.to_string(),
            ordinal: Some(first_ordinal + index as u64),
            item,
        };
        writeln!(
            file,
            "{}",
            serde_json::to_string(&line).expect("serialize paginated record")
        )
        .expect("write paginated record");
    }
    path
}

fn move_to_archived(home: &Path, path: PathBuf) -> PathBuf {
    let directory = home.join(codex_rollout::ARCHIVED_SESSIONS_SUBDIR);
    fs::create_dir_all(&directory).expect("create archived rollout directory");
    let archived_path = directory.join(path.file_name().expect("rollout filename"));
    fs::rename(path, &archived_path).expect("archive rollout");
    archived_path
}

fn compress_rollout(path: &Path) -> PathBuf {
    let compressed_path = path.with_extension("jsonl.zst");
    let compressed = zstd::stream::encode_all(
        fs::File::open(path).expect("open rollout"),
        /*level*/ 3,
    )
    .expect("compress rollout");
    fs::write(&compressed_path, compressed).expect("write compressed rollout");
    fs::remove_file(path).expect("remove plain rollout");
    compressed_path
}

fn user_message(text: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::UserMessage(UserMessageEvent {
        message: text.to_string(),
        ..UserMessageEvent::default()
    }))
}

fn agent_message(text: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::AgentMessage(AgentMessageEvent {
        message: text.to_string(),
        phase: None,
        memory_citation: None,
        delivery: None,
    }))
}

fn input_response_message(role: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn input_response_message_with_id(role: &str, id: &str, content: Vec<ContentItem>) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::with_suffix("msg", id)),
        role: role.to_string(),
        content,
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn rollout_response_item(item: ResponseItem) -> RolloutItem {
    RolloutItem::ResponseItem(item.into())
}

fn response_item_without_id(mut item: ResponseItem) -> ResponseItem {
    item.set_id(None);
    item
}

fn envelopes_without_ids(
    mut envelopes: Vec<codex_rollout::ResponseItemEnvelope>,
) -> Vec<codex_rollout::ResponseItemEnvelope> {
    for envelope in &mut envelopes {
        envelope.item.set_id(None);
    }
    envelopes
}

fn serialized_response_record(item: ResponseItem) -> Vec<u8> {
    let line = RolloutLine {
        timestamp: TIMESTAMP.to_string(),
        ordinal: None,
        item: rollout_response_item(item),
    };
    let mut bytes = serde_json::to_vec(&line).expect("serialize response record");
    bytes.push(b'\n');
    bytes
}

fn fracture_json_string_newlines(record: &[u8]) -> Vec<u8> {
    let mut fractured = Vec::with_capacity(record.len());
    let mut index = 0;
    let mut fracture_count = 0;
    while index < record.len() {
        if record[index..].starts_with(br"\n") {
            fractured.push(b'\n');
            fracture_count += 1;
            index += 2;
        } else {
            fractured.push(record[index]);
            index += 1;
        }
    }
    assert!(fracture_count > 0, "record contains an escaped newline");
    fractured
}

fn exec_completion(turn_id: &str, call_id: &str) -> RolloutItem {
    serde_json::from_value(json!({
        "type": "event_msg",
        "payload": {
            "type": "exec_command_end",
            "call_id": call_id,
            "turn_id": turn_id,
            "command": ["echo", "ok"],
            "cwd": "file:///tmp",
            "parsed_cmd": [],
            "source": "agent",
            "stdout": "ok",
            "stderr": "",
            "aggregated_output": "ok",
            "exit_code": 0,
            "duration": {"secs": 0, "nanos": 0},
            "formatted_output": "ok",
            "status": "completed"
        }
    }))
    .expect("build legacy exec completion")
}

fn item_completed(turn_id: &str, item_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id: ThreadId::new(),
        turn_id: turn_id.to_string(),
        item: TurnItem::Reasoning(ReasoningItem {
            id: item_id.to_string(),
            summary_text: vec!["summary".to_string()],
            raw_content: Vec::new(),
        }),
        started_at_ms: None,
        completed_at_ms: 1_735_905_601_000,
    }))
}

fn started(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_id.to_string(),
        trace_id: None,
        started_at: Some(1_735_905_600),
        model_context_window: None,
        collaboration_mode_kind: Default::default(),
    }))
}

fn compacted(replacement_history: Vec<ResponseItem>) -> RolloutItem {
    RolloutItem::Compacted(CompactedItem {
        message: "checkpoint".to_string(),
        replacement_history: Some(replacement_history.into_iter().map(Into::into).collect()),
        replacement_history_entries: None,
        mcp_resource_origins: None,
        window_number: Some(1),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    })
}

fn completed(turn_id: &str) -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnComplete(TurnCompleteEvent {
        turn_id: turn_id.to_string(),
        last_agent_message: None,
        error: None,
        started_at: Some(1_735_905_600),
        completed_at: Some(1_735_905_601),
        duration_ms: Some(1_000),
        time_to_first_token_ms: None,
    }))
}

fn read_rollout(path: &Path) -> Vec<RolloutLine> {
    fs::read_to_string(path)
        .expect("read migrated rollout")
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse migrated rollout"))
        .collect()
}

fn apply_options() -> RolloutMigrationOptions {
    RolloutMigrationOptions {
        mode: RolloutMigrationMode::Apply,
        max_mib_per_second: Some(1024),
        ..RolloutMigrationOptions::default()
    }
}

struct SourceSnapshot {
    bytes: Vec<u8>,
    readonly: bool,
    #[cfg(unix)]
    mode: u32,
}

fn source_snapshot(path: &Path) -> SourceSnapshot {
    let metadata = fs::metadata(path).expect("read source rollout metadata");
    SourceSnapshot {
        bytes: fs::read(path).expect("read source rollout"),
        readonly: metadata.permissions().readonly(),
        #[cfg(unix)]
        mode: metadata.permissions().mode(),
    }
}

fn copy_directory_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create copied Codex home directory");
    for entry in fs::read_dir(source).expect("list source Codex home") {
        let entry = entry.expect("read source Codex home entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().expect("read copied entry type");
        if file_type.is_dir() {
            copy_directory_tree(&source_path, &destination_path);
        } else if file_type.is_file() {
            match fs::copy(&source_path, &destination_path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // SQLite can remove a WAL/SHM sidecar after the directory entry was observed.
                }
                Err(error) => panic!("copy Codex home file {source_path:?}: {error}"),
            }
        } else {
            panic!("unexpected non-file Codex home entry: {source_path:?}");
        }
    }
}

fn file_tree_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn collect(root: &Path, directory: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(directory).expect("list snapshotted Codex home") {
            let entry = entry.expect("read snapshotted Codex home entry");
            let path = entry.path();
            let file_type = entry.file_type().expect("read snapshotted entry type");
            if file_type.is_dir() {
                collect(root, &path, files);
            } else if file_type.is_file() {
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();
                if file_name.ends_with("-wal") || file_name.ends_with("-shm") {
                    continue;
                }
                files.push((
                    path.strip_prefix(root)
                        .expect("snapshotted file belongs to Codex home")
                        .to_path_buf(),
                    fs::read(&path).expect("read snapshotted Codex home file"),
                ));
            } else {
                panic!("unexpected non-file Codex home entry: {path:?}");
            }
        }
    }

    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn sized_retired_record(record_bytes: usize) -> Vec<u8> {
    let prefix = format!(
        r#"{{"timestamp":"{TIMESTAMP}","type":"event_msg","payload":{{"type":"thread_name_updated","name":""#
    );
    let suffix = b"\"}}\n";
    let name_bytes = record_bytes
        .checked_sub(prefix.len() + suffix.len())
        .expect("record size can contain the valid rollout envelope");
    let mut record = Vec::with_capacity(record_bytes);
    record.extend_from_slice(prefix.as_bytes());
    record.extend(std::iter::repeat_n(b'x', name_bytes));
    record.extend_from_slice(suffix);
    assert_eq!(record.len(), record_bytes);
    record
}

async fn assert_failed_migration_preserved_source(
    store: &LocalThreadStore,
    home: &Path,
    thread_id: ThreadId,
    path: &Path,
    snapshot: &SourceSnapshot,
) {
    let current = source_snapshot(path);
    assert_eq!(current.bytes, snapshot.bytes);
    assert_eq!(current.readonly, snapshot.readonly);
    #[cfg(unix)]
    assert_eq!(current.mode, snapshot.mode);
    assert_eq!(
        codex_rollout::read_session_meta_line(path)
            .await
            .expect("read preserved session metadata")
            .meta
            .history_mode,
        ThreadHistoryMode::Legacy
    );
    assert_eq!(
        store
            .state_db
            .as_ref()
            .expect("state db")
            .get_thread(thread_id)
            .await
            .expect("read preserved thread metadata")
            .expect("thread metadata")
            .history_mode,
        ThreadHistoryMode::Legacy
    );
    assert!(
        thread_history::projection_state(store, thread_id)
            .await
            .expect("read projection state")
            .is_none()
    );
    let staged_path = super::staged_rollout_path(path).expect("derive staged path");
    for residue in [
        staged_path.clone(),
        super::rewritten_staged_rollout_path(&staged_path).expect("derive rewritten staged path"),
        super::compressed_staged_rollout_path(path).expect("derive compressed staged path"),
        super::decompressed_staged_rollout_path(path).expect("derive decompressed staged path"),
        migration_journal_path(home, thread_id),
    ] {
        assert!(
            !residue.exists(),
            "unexpected migration residue: {residue:?}"
        );
    }
}

fn assert_failed_with_reason(
    outcome: &super::RolloutMigrationOutcome,
    failure_reason: RolloutMigrationFailureReason,
) {
    assert_eq!(
        (outcome.status, outcome.failure_reason),
        (RolloutMigrationStatus::Failed, Some(failure_reason))
    );
}

async fn indexed_store(home: &Path) -> LocalThreadStore {
    let config = test_config(home);
    let rollout_config = RolloutConfig {
        codex_home: config.codex_home.clone(),
        sqlite: config.sqlite.clone(),
        cwd: home.to_path_buf(),
        model_provider_id: config.default_model_provider_id.clone(),
        generate_memories: false,
    };
    let state_db = codex_rollout::state_db::try_init(&rollout_config)
        .await
        .expect("backfill legacy thread metadata");
    LocalThreadStore::new(config, Some(state_db))
}

async fn list_active_summary_turns(store: &LocalThreadStore, thread_id: ThreadId) -> TurnPage {
    store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::Summary,
        })
        .await
        .expect("read projected turns")
}

#[tokio::test]
async fn migration_publishes_canonical_projected_history_and_is_idempotent() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("first question"),
            agent_message("first answer"),
        ],
    );
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate legacy rollout");
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);

    let lines = read_rollout(&path);
    assert_eq!(
        lines.iter().map(|line| line.ordinal).collect::<Vec<_>>(),
        (0..lines.len() as u64).map(Some).collect::<Vec<_>>()
    );
    assert!(matches!(
        &lines[0].item,
        RolloutItem::SessionMeta(metadata)
            if metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
                && metadata.meta.id == thread_id
                && metadata.meta.history_base.is_none()
    ));
    assert_eq!(
        lines
            .iter()
            .filter(|line| matches!(line.item, RolloutItem::EventMsg(EventMsg::ItemCompleted(_))))
            .count(),
        2
    );

    let turns = list_active_summary_turns(&store, thread_id).await;
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].items.len(), 2);

    let bytes = fs::read(&path).expect("read first migration");
    let second = store
        .migrate_rollouts(apply_options())
        .await
        .expect("rerun migration");
    assert_eq!(
        second.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
    assert_eq!(fs::read(&path).expect("read idempotent rollout"), bytes);
}

#[tokio::test]
async fn migration_contains_inline_paginated_history_byte_for_byte_without_sqlite_promotion() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let history_base = HistoryPosition {
        thread_id: ThreadId::new(),
        end_ordinal_exclusive: 41,
        end_byte_offset: 8_192,
    };
    let source = input_response_message_with_id(
        "user",
        "inherited-image",
        vec![ContentItem::InputImage {
            image_url: "data:image/png;base64,paginated-pass-through".to_string(),
            detail: None,
        }],
    );
    let path = write_inline_paginated_rollout(
        home.path(),
        thread_id,
        SessionSource::SubAgent(SubAgentSource::Other("test".to_string())),
        Some(history_base),
        Some(43),
        vec![
            rollout_response_item(source.clone()),
            compacted(vec![source.clone()]),
            agent_message("record that must not be synthesized or dropped"),
        ],
    );
    let before = fs::read(&path).expect("read inline paginated source");
    let store = indexed_store(home.path()).await;
    let state_before = store
        .state_db
        .as_ref()
        .expect("state db")
        .get_thread(thread_id)
        .await
        .expect("read inline paginated metadata")
        .expect("thread metadata");
    assert_eq!(state_before.history_mode, ThreadHistoryMode::Paginated);

    let report = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("contain inline paginated rollout");

    assert_eq!(
        report.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
    assert_eq!(report.outcomes[0].preserved_source_path, None);
    assert_eq!(fs::read(&path).expect("read contained rollout"), before);
    let state_after = store
        .state_db
        .as_ref()
        .expect("state db")
        .get_thread(thread_id)
        .await
        .expect("read metadata")
        .expect("thread metadata");
    assert_eq!(state_after.history_mode, ThreadHistoryMode::Paginated);
    assert_eq!(state_after.rollout_path, state_before.rollout_path);

    let second = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("rerun contained paginated rollout");
    assert_eq!(
        second.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
    assert_eq!(fs::read(&path).expect("read idempotent rollout"), before);
    assert_eq!(second.outcomes[0].preserved_source_path, None);
}

#[tokio::test]
async fn paginated_containment_does_not_resolve_or_rewrite_a_dangling_checkpoint() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let checkpoint = CompactedItem {
        message: "dangling checkpoint".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
            item_id: "missing-response-item".to_string(),
        }]),
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    };
    let path = write_inline_paginated_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        None,
        None,
        vec![RolloutItem::Compacted(checkpoint.clone())],
    );
    let before = fs::read(&path).expect("read source before failed upgrade");
    let store = indexed_store(home.path()).await;
    let mut limiter = super::RolloutMigrationRateLimiter::new(None).expect("create rate limiter");
    store
        .project_rollout_in_batches(thread_id, &path, &mut limiter)
        .await
        .expect("seed the usable old paginated projection");
    let projection_before = thread_history::projection_state(&store, thread_id)
        .await
        .expect("read initial projection")
        .expect("initial projection exists");

    let report = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("contain paginated rollout with dangling checkpoint");

    assert_eq!(
        report.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
    assert_eq!(fs::read(&path).expect("read preserved source"), before);
    assert_eq!(
        codex_rollout::read_session_meta_line(&path)
            .await
            .expect("read preserved metadata")
            .meta
            .history_mode,
        ThreadHistoryMode::Paginated
    );
    let projection = thread_history::projection_state(&store, thread_id)
        .await
        .expect("read retained projection")
        .expect("old paginated projection stays live");
    assert_eq!(
        (projection.next_byte_offset, projection.next_ordinal),
        (
            projection_before.next_byte_offset,
            projection_before.next_ordinal
        )
    );
    assert!(!migration_journal_path(home.path(), thread_id).exists());
    assert!(
        !preserved_legacy_rollout_path(&path)
            .expect("derive preserved path")
            .exists()
    );
}

#[tokio::test]
async fn paginated_containment_leaves_a_trailing_partial_and_live_projection_untouched() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_inline_paginated_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        None,
        None,
        vec![user_message("complete paginated record")],
    );
    let store = indexed_store(home.path()).await;
    let mut limiter = super::RolloutMigrationRateLimiter::new(None).expect("create rate limiter");
    store
        .project_rollout_in_batches(thread_id, &path, &mut limiter)
        .await
        .expect("seed old paginated projection");
    let projection_before = thread_history::projection_state(&store, thread_id)
        .await
        .expect("read initial projection")
        .expect("initial projection exists");
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open paginated rollout")
        .write_all(br#"{"timestamp":"unterminated""#)
        .expect("append partial record");
    let source = fs::read(&path).expect("read source with partial tail");

    let report = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("contain selected paginated rollout");

    assert_eq!(
        report.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
    assert_eq!(fs::read(&path).expect("read unchanged rollout"), source);
    let projection_after = thread_history::projection_state(&store, thread_id)
        .await
        .expect("read retained projection")
        .expect("projection remains usable");
    assert_eq!(
        (
            projection_after.next_byte_offset,
            projection_after.next_ordinal
        ),
        (
            projection_before.next_byte_offset,
            projection_before.next_ordinal
        )
    );
    assert!(!migration_journal_path(home.path(), thread_id).exists());
    assert!(
        !preserved_legacy_rollout_path(&path)
            .expect("derive preserved path")
            .exists()
    );
}

#[tokio::test]
async fn migration_stores_repeated_compaction_payload_once_without_changing_context() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let image_url = "data:image/png;base64,unique-compaction-payload";
    let source = input_response_message_with_id(
        "user",
        "image-source",
        vec![ContentItem::InputImage {
            image_url: image_url.to_string(),
            detail: None,
        }],
    );
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(source.clone()),
            compacted(vec![source.clone()]),
        ],
    );
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate repeated compaction payload");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    let output = fs::read_to_string(&path).expect("read migrated rollout");
    assert_eq!(output.matches(image_url).count(), 1);

    let mut lines = read_rollout(&path);
    let checkpoint = lines
        .iter()
        .find_map(|line| match &line.item {
            RolloutItem::Compacted(checkpoint) => Some(checkpoint),
            _ => None,
        })
        .expect("reference-backed checkpoint");
    assert_eq!(checkpoint.replacement_history, None);
    assert_eq!(
        checkpoint.replacement_history_entries,
        Some(vec![CompactedHistoryEntry::Reference {
            item_id: source.id().expect("source id").as_str().to_string(),
        }])
    );

    let mut resolver = CompactedHistoryResolver::default();
    for line in &mut lines {
        resolver
            .materialize_item(&mut line.item)
            .expect("materialize migrated checkpoint");
    }
    let materialized = lines
        .into_iter()
        .find_map(|line| match line.item {
            RolloutItem::Compacted(checkpoint) => checkpoint.replacement_history,
            _ => None,
        })
        .expect("materialized replacement history");
    assert_eq!(materialized, vec![source.into()]);
}

#[tokio::test]
async fn migration_backfills_occurrence_id_for_repeated_no_id_image() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let image_url = "data:image/png;base64,historical-no-id-image";
    let source = ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputImage {
            image_url: image_url.to_string(),
            detail: None,
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(source.clone()),
            compacted(vec![source.clone()]),
            compacted(vec![source.clone()]),
        ],
    );
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate repeated no-ID image");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    let output = fs::read_to_string(&path).expect("read migrated rollout");
    assert_eq!(output.matches(image_url).count(), 1);
    let mut lines = read_rollout(&path);
    let assigned_id = lines
        .iter()
        .find_map(|line| match &line.item {
            RolloutItem::ResponseItem(envelope) => envelope.item.id(),
            _ => None,
        })
        .expect("top-level image received a stable ID")
        .clone();
    assert!(assigned_id.is_prefixed());
    let checkpoint_entries = lines
        .iter()
        .filter_map(|line| match &line.item {
            RolloutItem::Compacted(checkpoint) => checkpoint.replacement_history_entries.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        checkpoint_entries,
        vec![
            vec![CompactedHistoryEntry::Reference {
                item_id: assigned_id.to_string(),
            }],
            vec![CompactedHistoryEntry::Reference {
                item_id: assigned_id.to_string(),
            }],
        ]
    );
    let mut resolver = CompactedHistoryResolver::default();
    for line in &mut lines {
        resolver
            .materialize_item(&mut line.item)
            .expect("materialize no-ID checkpoints");
    }
    let materialized_histories = lines
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(checkpoint) => checkpoint.replacement_history,
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(materialized_histories.len(), 2);
    for history in materialized_histories {
        assert_eq!(envelopes_without_ids(history), vec![source.clone().into()]);
    }
}

#[tokio::test]
async fn migration_assigns_distinct_ids_to_duplicate_equal_no_id_occurrences() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let source = input_response_message("user", "duplicate but separately ordered");
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(source.clone()),
            rollout_response_item(source.clone()),
            compacted(vec![source.clone(), source]),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate duplicate equal occurrences");

    let lines = read_rollout(&path);
    let assigned_ids = lines
        .iter()
        .filter_map(|line| match &line.item {
            RolloutItem::ResponseItem(envelope) => envelope.item.id().cloned(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(assigned_ids.len(), 2);
    assert_ne!(assigned_ids[0], assigned_ids[1]);
    let entries = lines
        .iter()
        .find_map(|line| match &line.item {
            RolloutItem::Compacted(checkpoint) => checkpoint.replacement_history_entries.as_ref(),
            _ => None,
        })
        .expect("reference-backed checkpoint");
    assert_eq!(
        entries,
        &vec![
            CompactedHistoryEntry::Reference {
                item_id: assigned_ids[0].to_string(),
            },
            CompactedHistoryEntry::Reference {
                item_id: assigned_ids[1].to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn migration_occurrence_ids_are_deterministic_for_byte_identical_rollout_copies() {
    let left_home = TempDir::new().expect("create first Codex home");
    let right_home = TempDir::new().expect("create second Codex home");
    let thread_id = ThreadId::from_u128(0x1234);
    let source = input_response_message("user", "same historical occurrence");
    let left_path = write_rollout(
        left_home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(source.clone()),
            compacted(vec![source]),
        ],
    );
    let relative_path = left_path
        .strip_prefix(left_home.path())
        .expect("rollout belongs to first home");
    let right_path = right_home.path().join(relative_path);
    fs::create_dir_all(right_path.parent().expect("rollout parent"))
        .expect("create copied rollout directory");
    fs::copy(&left_path, &right_path).expect("copy rollout byte-for-byte");
    assert_eq!(
        fs::read(&left_path).expect("read first source"),
        fs::read(&right_path).expect("read copied source")
    );
    let left_store = indexed_store(left_home.path()).await;
    let right_store = indexed_store(right_home.path()).await;

    left_store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate first copy");
    right_store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate second copy");

    let assigned_id = |path: &Path| {
        read_rollout(path)
            .into_iter()
            .find_map(|line| match line.item {
                RolloutItem::ResponseItem(envelope) => envelope.item.id().cloned(),
                _ => None,
            })
            .expect("migrated source has assigned ID")
    };
    assert_eq!(assigned_id(&left_path), assigned_id(&right_path));
}

#[tokio::test]
async fn migration_recovers_missing_checkpoint_id_from_exact_explicit_occurrence() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let source = input_response_message_with_id(
        "user",
        "explicit-occurrence",
        vec![ContentItem::InputText {
            text: "checkpoint lost only its ID".to_string(),
        }],
    );
    let mut checkpoint_copy = source.clone();
    checkpoint_copy.set_id(None);
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(source.clone()),
            compacted(vec![checkpoint_copy]),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate missing checkpoint ID");

    let checkpoint = read_rollout(&path)
        .into_iter()
        .find_map(|line| match line.item {
            RolloutItem::Compacted(checkpoint) => Some(checkpoint),
            _ => None,
        })
        .expect("migrated checkpoint");
    assert_eq!(
        checkpoint.replacement_history_entries,
        Some(vec![CompactedHistoryEntry::Reference {
            item_id: source.id().expect("explicit source ID").to_string(),
        }])
    );
}

#[tokio::test]
async fn migration_keeps_first_unmatched_no_id_checkpoint_carrier_inline_then_references_it() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let image_url = "data:image/png;base64,checkpoint-first-carrier";
    let source = ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputImage {
            image_url: image_url.to_string(),
            detail: None,
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![compacted(vec![source.clone()]), compacted(vec![source])],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate checkpoint-first carrier");

    assert_eq!(
        fs::read_to_string(&path)
            .expect("read migrated rollout")
            .matches(image_url)
            .count(),
        1
    );
    let checkpoints = read_rollout(&path)
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(checkpoint) => Some(checkpoint),
            _ => None,
        })
        .collect::<Vec<_>>();
    let carrier = checkpoints[0]
        .replacement_history
        .as_ref()
        .expect("first checkpoint retains its new carrier inline");
    let assigned_id = carrier[0]
        .item
        .id()
        .expect("inline carrier received a stable ID")
        .to_string();
    assert_eq!(checkpoints[0].replacement_history_entries, None);
    assert_eq!(
        checkpoints[1].replacement_history_entries,
        Some(vec![CompactedHistoryEntry::Reference {
            item_id: assigned_id,
        }])
    );
}

#[tokio::test]
async fn migration_uses_only_older_sources_and_reuses_prior_checkpoint_inlines() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let first = input_response_message_with_id(
        "user",
        "first",
        vec![ContentItem::InputText {
            text: "first".to_string(),
        }],
    );
    let second = input_response_message_with_id(
        "assistant",
        "second",
        vec![ContentItem::OutputText {
            text: "second".to_string(),
        }],
    );
    let checkpoint = |message: &str| CompactedItem {
        message: message.to_string(),
        replacement_history: None,
        replacement_history_entries: None,
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    };
    let mut first_checkpoint = checkpoint("first checkpoint");
    first_checkpoint.replacement_history_entries = Some(vec![
        CompactedHistoryEntry::Reference {
            item_id: first.id().expect("first id").as_str().to_string(),
        },
        CompactedHistoryEntry::Inline {
            item: Box::new(second.clone()),
            metadata: None,
        },
    ]);
    let mut second_checkpoint = checkpoint("second checkpoint");
    second_checkpoint.replacement_history = Some(vec![first.clone().into(), second.clone().into()]);
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(first.clone()),
            RolloutItem::Compacted(first_checkpoint),
            RolloutItem::Compacted(second_checkpoint),
        ],
    );
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate chained compaction checkpoints");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    let checkpoints = read_rollout(&path)
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(checkpoint) => Some(checkpoint),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(
        checkpoints[0].replacement_history_entries,
        Some(vec![
            CompactedHistoryEntry::Reference {
                item_id: first.id().expect("first id").as_str().to_string(),
            },
            CompactedHistoryEntry::Inline {
                item: Box::new(second.clone()),
                metadata: None,
            },
        ])
    );
    assert_eq!(
        checkpoints[1].replacement_history_entries,
        Some(vec![
            CompactedHistoryEntry::Reference {
                item_id: first.id().expect("first id").as_str().to_string(),
            },
            CompactedHistoryEntry::Reference {
                item_id: second.id().expect("second id").as_str().to_string(),
            },
        ])
    );
}

#[tokio::test]
async fn migration_projects_explicit_and_implicit_legacy_completed_items() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let exec = exec_completion("explicit", "call-1");
    let reasoning = serde_json::from_value(json!({
        "type": "event_msg",
        "payload": {"type": "agent_reasoning", "text": "summary"}
    }))
    .expect("build legacy reasoning summary");
    let raw_reasoning = serde_json::from_value(json!({
        "type": "event_msg",
        "payload": {"type": "agent_reasoning_raw_content", "text": "raw"}
    }))
    .expect("build legacy reasoning content");
    write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("explicit"),
            exec,
            completed("explicit"),
            reasoning,
            raw_reasoning,
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate legacy completion events");

    let turns = list_active_summary_turns(&store, thread_id).await;
    assert_eq!(turns.turns.len(), 2);
    assert_eq!(turns.turns[0].turn_id, "explicit");
    let items = store
        .list_items(ListItemsParams {
            thread_id,
            turn_id: None,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            sort_key: ItemSortKey::CreatedAtOrdinal,
            after_updated_at_ordinal: None,
        })
        .await
        .expect("read projected items");
    assert_eq!(items.items.len(), 2);
    assert_eq!(items.items[0].turn_id, "explicit");
    assert_eq!(items.items[1].turn_id, turns.turns[1].turn_id);
    let command: serde_json::Value =
        serde_json::from_slice(&items.items[0].item_json).expect("parse projected command");
    let reasoning: serde_json::Value =
        serde_json::from_slice(&items.items[1].item_json).expect("parse projected reasoning");
    assert_eq!(command["type"], "commandExecution");
    assert_eq!(reasoning["type"], "reasoning");
    assert_eq!(reasoning["summary"], json!(["summary"]));
    assert_eq!(reasoning["content"], json!(["raw"]));
}

#[tokio::test]
async fn migration_preserves_image_generation_failure_metadata() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let expected_item = ImageGenerationItem {
        id: "image-call".to_string(),
        status: "failed".to_string(),
        revised_prompt: Some("paint a blue whale".to_string()),
        result: String::new(),
        transparent_background: None,
        failure: Some(ImageGenerationFailure::UsageLimitExceeded {
            limit_id: "image_gen".to_string(),
            resets_at: Some(1_786_150_800),
        }),
        saved_path: None,
        imagegen_request_id: None,
    };
    let image_completion =
        RolloutItem::EventMsg(EventMsg::ImageGenerationEnd(ImageGenerationEndEvent {
            call_id: expected_item.id.clone(),
            status: expected_item.status.clone(),
            revised_prompt: expected_item.revised_prompt.clone(),
            result: expected_item.result.clone(),
            transparent_background: expected_item.transparent_background,
            failure: expected_item.failure.clone(),
            saved_path: expected_item.saved_path.clone(),
        }));
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("image-turn"),
            image_completion,
            completed("image-turn"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate legacy image completion");

    let migrated_item = read_rollout(&path)
        .into_iter()
        .find_map(|line| match line.item {
            RolloutItem::EventMsg(EventMsg::ItemCompleted(event)) => Some(event.item),
            _ => None,
        })
        .expect("migrated image completion");
    let TurnItem::Extension(ExtensionItem::ImageGeneration(migrated_item)) = migrated_item else {
        panic!("expected migrated extension image-generation item");
    };
    assert_eq!(migrated_item, expected_item);
}

#[tokio::test]
async fn migration_keeps_late_completions_in_their_original_turn() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("old"),
            user_message("old question"),
            started("current"),
            user_message("current question"),
            exec_completion("old", "call-old"),
            agent_message("current answer"),
            completed("current"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate late completion");

    let started_turn_ids = read_rollout(&path)
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::EventMsg(EventMsg::TurnStarted(event)) => Some(event.turn_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(started_turn_ids, vec!["old", "current"]);

    let items = store
        .list_items(ListItemsParams {
            thread_id,
            turn_id: None,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            sort_key: ItemSortKey::CreatedAtOrdinal,
            after_updated_at_ordinal: None,
        })
        .await
        .expect("read projected late completion");
    let item_turn_ids = items
        .items
        .iter()
        .map(|item| item.turn_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(item_turn_ids, vec!["old", "current", "old", "current"]);
}

#[tokio::test]
async fn migration_hoists_delayed_session_meta_before_paginated_history() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(home.path(), thread_id, SessionSource::Cli, Vec::new());
    let existing = fs::read_to_string(&path).expect("read legacy rollout");
    let pre_header = RolloutLine {
        timestamp: TIMESTAMP.to_string(),
        ordinal: None,
        item: user_message("before metadata"),
    };
    fs::write(
        &path,
        format!(
            "{}\n{existing}",
            serde_json::to_string(&pre_header).expect("serialize pre-header record")
        ),
    )
    .expect("write pre-header rollout");
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate delayed session metadata");

    let lines = read_rollout(&path);
    assert_eq!(lines[0].ordinal, Some(0));
    assert!(matches!(
        &lines[0].item,
        RolloutItem::SessionMeta(metadata)
            if metadata.meta.id == thread_id
                && metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
    ));
    let turns = store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::NotLoaded,
        })
        .await
        .expect("read migrated turns");
    assert_eq!(turns.turns.len(), 1);

    let second = store
        .migrate_rollouts(apply_options())
        .await
        .expect("rerun migration");
    assert_eq!(
        second.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
}

#[tokio::test]
async fn migration_preserves_valid_final_record_without_newline() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("question")],
    );
    let final_line = RolloutLine {
        timestamp: TIMESTAMP.to_string(),
        ordinal: None,
        item: agent_message("answer"),
    };
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open legacy rollout")
        .write_all(
            serde_json::to_string(&final_line)
                .expect("serialize final record")
                .as_bytes(),
        )
        .expect("append final record");
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate legacy rollout");

    assert_eq!(
        read_rollout(&path)
            .iter()
            .filter(|line| matches!(line.item, RolloutItem::EventMsg(EventMsg::ItemCompleted(_))))
            .count(),
        2
    );
}

#[tokio::test]
async fn migration_applies_historical_rollbacks_before_sqlite_projection() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("keep"),
            user_message("keep question"),
            agent_message("keep answer"),
            completed("keep"),
            started("remove"),
            user_message("remove question"),
            agent_message("remove answer"),
            completed("remove"),
            started("shell"),
            completed("shell"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            started("replacement"),
            user_message("replacement question"),
            agent_message("replacement answer"),
            completed("replacement"),
        ],
    );
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate rolled-back thread");
    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);

    let lines = read_rollout(&path);
    assert!(!lines.iter().any(|line| matches!(
        line.item,
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(_))
    )));
    let turns = list_active_summary_turns(&store, thread_id).await;
    assert_eq!(
        turns
            .turns
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["keep", "replacement"]
    );
}

#[tokio::test]
async fn migration_rolls_back_response_and_inter_agent_user_boundaries() {
    let home = TempDir::new().expect("create Codex home");
    let response_thread_id = ThreadId::new();
    let response_path = write_rollout(
        home.path(),
        response_thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(input_response_message("user", "remove response boundary")),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            user_message("replacement question"),
        ],
    );
    let communication_thread_id = ThreadId::new();
    let communication_path = write_rollout(
        home.path(),
        communication_thread_id,
        SessionSource::Cli,
        vec![
            RolloutItem::InterAgentCommunication(InterAgentCommunication::new(
                AgentPath::root(),
                AgentPath::root().join("worker").expect("worker path"),
                Vec::new(),
                "remove communication boundary".to_string(),
                /*trigger_turn*/ true,
            )),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            user_message("replacement question"),
        ],
    );
    let contextual_thread_id = ThreadId::new();
    let contextual_path = write_rollout(
        home.path(),
        contextual_thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(input_response_message("user", "keep first boundary")),
            rollout_response_item(input_response_message(
                "developer",
                "<managed_developer_instructions>context only</managed_developer_instructions>",
            )),
            rollout_response_item(input_response_message(
                "developer",
                "<permissions instructions>context only</permissions instructions>",
            )),
            rollout_response_item(input_response_message(
                "user",
                "<environment_context>context only</environment_context>",
            )),
            rollout_response_item(input_response_message("user", "remove real user boundary")),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate legacy rollback boundaries");

    assert!(
        !read_rollout(&response_path)
            .iter()
            .any(|line| matches!(line.item, RolloutItem::ResponseItem(_)))
    );
    assert!(
        !read_rollout(&communication_path)
            .iter()
            .any(|line| matches!(line.item, RolloutItem::InterAgentCommunication(_)))
    );
    assert_eq!(
        read_rollout(&contextual_path)
            .into_iter()
            .filter_map(|line| match line.item {
                RolloutItem::ResponseItem(response) => {
                    Some(response_item_without_id(response.into_item()))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![input_response_message("user", "keep first boundary")]
    );
}

#[tokio::test]
async fn migration_drops_trailing_context_when_rollback_arrives_before_next_turn() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(input_response_message("user", "keep question")),
            rollout_response_item(input_response_message("user", "remove question")),
            rollout_response_item(input_response_message(
                "user",
                "<turn_aborted>remove this context too</turn_aborted>",
            )),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate trailing rollback context");

    assert_eq!(
        read_rollout(&path)
            .into_iter()
            .filter_map(|line| match line.item {
                RolloutItem::ResponseItem(response) => {
                    Some(response_item_without_id(response.into_item()))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![input_response_message("user", "keep question")]
    );
}

#[tokio::test]
async fn migration_coalesces_response_first_user_message_rollback_boundary() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(input_response_message("user", "remove question")),
            user_message("remove question"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate response-first rollback");

    assert!(
        !read_rollout(&path)
            .iter()
            .any(|line| matches!(line.item, RolloutItem::ResponseItem(_)))
    );
}

#[tokio::test]
async fn migration_does_not_coalesce_distinct_adjacent_user_records() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(input_response_message("user", "copied parent question")),
            user_message("child question"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate distinct adjacent user records");

    assert_eq!(
        read_rollout(&path)
            .into_iter()
            .filter_map(|line| match line.item {
                RolloutItem::ResponseItem(response) => {
                    Some(response_item_without_id(response.into_item()))
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![input_response_message("user", "copied parent question")]
    );
}

#[tokio::test]
async fn migration_keeps_late_completions_for_surviving_turns_across_rollback() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("old"),
            user_message("old question"),
            started("remove"),
            user_message("remove question"),
            completed("old"),
            exec_completion("old", "call-old"),
            item_completed("old", "reason-old"),
            exec_completion("remove", "call-remove"),
            completed("remove"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            started("replacement"),
            user_message("replacement question"),
            completed("replacement"),
        ],
    );
    let legacy_turns = build_turns_from_rollout_items(
        &read_rollout(&path)
            .into_iter()
            .map(|line| line.item)
            .collect::<Vec<_>>(),
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate late completion rollback");

    let lines = read_rollout(&path);
    assert!(!lines.iter().any(|line| matches!(
        &line.item,
        RolloutItem::EventMsg(EventMsg::ItemCompleted(event))
            if event.item.id() == "call-remove"
    )));
    assert!(lines.iter().any(|line| matches!(
        &line.item,
        RolloutItem::EventMsg(EventMsg::ItemCompleted(event))
            if event.turn_id == "old" && event.item.id() == "call-old"
    )));
    assert!(lines.iter().any(|line| matches!(
        &line.item,
        RolloutItem::EventMsg(EventMsg::ItemCompleted(event))
            if event.turn_id == "old" && event.item.id() == "reason-old"
    )));
    assert!(lines.iter().any(|line| matches!(
        &line.item,
        RolloutItem::EventMsg(EventMsg::TurnComplete(event)) if event.turn_id == "old"
    )));
    assert!(!lines.iter().any(|line| matches!(
        &line.item,
        RolloutItem::EventMsg(EventMsg::TurnComplete(event)) if event.turn_id == "remove"
    )));
    let turns = store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::Summary,
        })
        .await
        .expect("read late-completion rollback turns");
    let turn_ids = turns
        .turns
        .iter()
        .map(|turn| turn.turn_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        turn_ids,
        legacy_turns
            .iter()
            .map(|turn| turn.id.as_str())
            .collect::<Vec<_>>()
    );
    let items = store
        .list_items(ListItemsParams {
            thread_id,
            turn_id: None,
            include_archived: false,
            cursor: None,
            page_size: 20,
            sort_direction: SortDirection::Asc,
            sort_key: ItemSortKey::CreatedAtOrdinal,
            after_updated_at_ordinal: None,
        })
        .await
        .expect("read late-completion rollback items");
    for legacy_turn in &legacy_turns {
        for legacy_item in &legacy_turn.items {
            if legacy_item.id().starts_with("item-") {
                continue;
            }
            assert_eq!(
                items
                    .items
                    .iter()
                    .find(|item| item.item_id == legacy_item.id())
                    .map(|item| item.turn_id.as_str()),
                Some(legacy_turn.id.as_str())
            );
        }
    }
}

#[tokio::test]
async fn migration_rolls_back_inter_agent_metadata_with_its_delivery() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let delivery = InterAgentCommunication::new(
        AgentPath::root(),
        AgentPath::root().join("worker").expect("worker path"),
        Vec::new(),
        "remove delivery".to_string(),
        /*trigger_turn*/ true,
    );
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            RolloutItem::InterAgentCommunicationMetadata { trigger_turn: true },
            rollout_response_item(delivery.to_model_input_item()),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate rolled-back inter-agent delivery");

    assert!(!read_rollout(&path).iter().any(|line| match &line.item {
        RolloutItem::InterAgentCommunicationMetadata { .. } => true,
        RolloutItem::ResponseItem(response_item) => {
            matches!(&response_item.item, ResponseItem::AgentMessage { .. })
        }
        _ => false,
    }));
}

#[tokio::test]
async fn migration_rolls_back_pre_compaction_turns_from_sqlite_history() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let prior_assistant = input_response_message_with_id(
        "assistant",
        "prior-assistant",
        vec![ContentItem::OutputText {
            text: "old answer".to_string(),
        }],
    );
    let mut checkpoint = CompactedItem {
        message: "checkpoint".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![
            CompactedHistoryEntry::Inline {
                item: Box::new(input_response_message("user", "old question")),
                metadata: None,
            },
            CompactedHistoryEntry::Reference {
                item_id: prior_assistant
                    .id()
                    .expect("prior assistant id")
                    .as_str()
                    .to_string(),
            },
        ]),
        mcp_resource_origins: None,
        window_number: Some(1),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    };
    checkpoint.mcp_resource_origins = Some(McpResourceOriginCheckpoint {
        origins: vec![McpResourceOrigin {
            call_id: "widget-call".to_string(),
            turn_id: Some("keep-before-compaction".to_string()),
            tool: "_product_search".to_string(),
            connector_id: "shopping".to_string(),
            link_id: None,
            uri: "ui://shopping/widget".to_string(),
            ambiguous_account: false,
        }],
        turns: vec!["keep-before-compaction".to_string()],
        current_turn_id: Some("keep-before-compaction".to_string()),
    });
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(prior_assistant),
            started("keep-before-compaction"),
            user_message("old question"),
            completed("keep-before-compaction"),
            RolloutItem::Compacted(checkpoint),
            started("remove-after-compaction"),
            user_message("new question"),
            completed("remove-after-compaction"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 2,
            })),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate rollback through compaction");

    let turns = store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::Summary,
        })
        .await
        .expect("read rollback-through-compaction turns");
    assert_eq!(turns.turns.len(), 1);
    let checkpoint = read_rollout(&path)
        .into_iter()
        .find_map(|line| match line.item {
            RolloutItem::Compacted(item) => Some(item),
            _ => None,
        })
        .expect("retained compaction");
    assert_eq!(checkpoint.replacement_history, Some(Vec::new()));
    assert_eq!(checkpoint.replacement_history_entries, None);
    assert_eq!(checkpoint.mcp_resource_origins, None);
}

#[tokio::test]
async fn rollback_rewrite_resolves_requested_source_outside_selected_checkpoint() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let external_source = input_response_message_with_id(
        "assistant",
        "outside-selected-checkpoint",
        vec![ContentItem::OutputText {
            text: "externally sourced answer".to_string(),
        }],
    );
    let external_source_id = external_source
        .id()
        .expect("external source id")
        .as_str()
        .to_string();
    let rewritten_checkpoint = RolloutItem::Compacted(CompactedItem {
        message: "newer checkpoint to rewrite".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![
            CompactedHistoryEntry::Inline {
                item: Box::new(input_response_message("user", "kept history")),
                metadata: None,
            },
            CompactedHistoryEntry::Reference {
                item_id: external_source_id,
            },
            CompactedHistoryEntry::Inline {
                item: Box::new(input_response_message("user", "remove history")),
                metadata: None,
            },
        ]),
        mcp_resource_origins: None,
        window_number: Some(2),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    });
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(external_source.clone()),
            // Reverse replay selects this older checkpoint, which deliberately does not carry the
            // external source needed by the newer checkpoint's rollback rewrite.
            compacted(vec![input_response_message("user", "kept history")]),
            started("remove"),
            user_message("remove history"),
            completed("remove"),
            rewritten_checkpoint,
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            started("replacement"),
            user_message("replacement history"),
            completed("replacement"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("resolve requested source while rewriting newer checkpoint");

    let mut lines = read_rollout(&path);
    let mut resolver = CompactedHistoryResolver::default();
    for line in &mut lines {
        resolver
            .materialize_item(&mut line.item)
            .expect("materialize rewritten rollback checkpoint");
    }
    let rewritten = lines
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(item) => Some(item),
            _ => None,
        })
        .next_back()
        .expect("rewritten checkpoint");
    let history = rewritten
        .replacement_history
        .expect("rewritten checkpoint materializes completely");
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].item, external_source);
}

#[tokio::test]
async fn migration_preserves_reverse_replay_anchor_after_pre_compaction_rollback() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            rollout_response_item(input_response_message("user", "remove question")),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            compacted(vec![input_response_message("user", "old question")]),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate pre-compaction rollback");

    let replacement_history = read_rollout(&path)
        .into_iter()
        .find_map(|line| match line.item {
            RolloutItem::Compacted(item) => item.replacement_history,
            _ => None,
        })
        .expect("retained compaction");
    assert!(replacement_history.is_empty());
}

#[tokio::test]
async fn migration_keeps_empty_replay_anchor_from_rolled_back_turn() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("keep"),
            user_message("keep question"),
            completed("keep"),
            started("remove"),
            user_message("remove question"),
            compacted(vec![input_response_message("user", "old question")]),
            completed("remove"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            started("replacement"),
            user_message("replacement question"),
            completed("replacement"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate rolled-back replay anchor");

    let replacement_history = read_rollout(&path)
        .into_iter()
        .find_map(|line| match line.item {
            RolloutItem::Compacted(item) => item.replacement_history,
            _ => None,
        })
        .expect("retained compaction");
    assert!(replacement_history.is_empty());
}

#[tokio::test]
async fn migration_uses_turn_context_to_select_reverse_replay_anchor() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("keep"),
            user_message("keep question"),
            compacted(vec![input_response_message("user", "keep question")]),
            completed("keep"),
            started("remove"),
            user_message("remove question"),
            serde_json::from_value(json!({
                "type": "event_msg",
                "payload": {
                    "type": "turn_aborted",
                    "turn_id": "other",
                    "reason": "interrupted"
                }
            }))
            .expect("build mismatched turn abort"),
            serde_json::from_value(json!({
                "type": "turn_context",
                "payload": {
                    "turn_id": "remove",
                    "cwd": home.path(),
                    "approval_policy": "never",
                    "sandbox_policy": {"type": "read-only"},
                    "model": "test-model",
                    "summary": "auto"
                }
            }))
            .expect("build turn context"),
            compacted(vec![input_response_message("user", "remove question")]),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            started("replacement"),
            user_message("replacement question"),
            completed("replacement"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate turn-context replay anchor");

    let replacement_histories = read_rollout(&path)
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(item) => item.replacement_history.map(|items| {
                items
                    .into_iter()
                    .map(codex_rollout::ResponseItemEnvelope::into_item)
                    .map(response_item_without_id)
                    .collect::<Vec<_>>()
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        replacement_histories,
        vec![vec![input_response_message("user", "keep question")]]
    );
}

#[tokio::test]
async fn migration_applies_cumulative_and_overflowing_rollbacks() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            started("first"),
            user_message("first question"),
            completed("first"),
            started("second"),
            user_message("second question"),
            completed("second"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 1,
            })),
            started("third"),
            user_message("third question"),
            completed("third"),
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 99,
            })),
            user_message("replacement question"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate cumulative rollbacks");

    let turns = store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: false,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::Summary,
        })
        .await
        .expect("read cumulative rollback turns");
    assert_eq!(turns.turns.len(), 1);
    assert!(!read_rollout(&path).iter().any(|line| matches!(
        line.item,
        RolloutItem::EventMsg(EventMsg::ThreadRolledBack(_))
    )));
}

#[tokio::test]
async fn migration_drops_copied_user_fork_metadata_without_creating_a_history_base() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let parent_id = ThreadId::new();
    let copied_metadata = SessionMeta {
        session_id: parent_id.into(),
        id: parent_id,
        timestamp: TIMESTAMP.to_string(),
        cwd: home.path().to_path_buf(),
        source: SessionSource::Cli,
        ..SessionMeta::default()
    };
    let copied_response =
        rollout_response_item(input_response_message("user", "copied parent history"));
    let path = write_rollout_with_fork(
        home.path(),
        thread_id,
        SessionSource::Cli,
        Some(parent_id),
        vec![
            RolloutItem::SessionMeta(SessionMetaLine {
                meta: copied_metadata,
                git: None,
            }),
            copied_response,
            user_message("child question"),
            agent_message("child answer"),
        ],
    );
    let expected_responses = read_rollout(&path)
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::ResponseItem(item) => {
                Some(serde_json::to_value(item.item).expect("serialize copied response"))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate copied user fork");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    let lines = read_rollout(&path);
    assert!(matches!(
        &lines[0].item,
        RolloutItem::SessionMeta(metadata)
            if metadata.meta.id == thread_id
                && metadata.meta.forked_from_id == Some(parent_id)
                && metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
                && metadata.meta.history_base.is_none()
    ));
    assert_eq!(
        lines
            .iter()
            .filter(|line| matches!(line.item, RolloutItem::SessionMeta(_)))
            .count(),
        1
    );
    assert_eq!(
        lines
            .into_iter()
            .filter_map(|line| match line.item {
                RolloutItem::ResponseItem(item) => {
                    Some(
                        serde_json::to_value(response_item_without_id(item.item))
                            .expect("serialize migrated response"),
                    )
                }
                _ => None,
            })
            .collect::<Vec<_>>(),
        expected_responses
    );
}

#[tokio::test]
async fn migration_compacts_subagent_prefix_and_does_not_project_it() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::SubAgent(SubAgentSource::Other("test".to_string())),
        vec![
            RolloutItem::Compacted(CompactedItem {
                message: "superseded checkpoint".repeat(1024),
                replacement_history: Some(Vec::new()),
                replacement_history_entries: None,
                mcp_resource_origins: None,
                window_number: Some(1),
                first_window_id: None,
                previous_window_id: None,
                window_id: None,
            }),
            RolloutItem::Compacted(CompactedItem {
                message: "latest checkpoint".to_string(),
                replacement_history: Some(vec![
                    ResponseItem::Message {
                        id: None,
                        role: "user".to_string(),
                        content: vec![ContentItem::InputText {
                            text: "latest compacted context".to_string(),
                        }],
                        phase: None,
                        internal_chat_message_metadata_passthrough: None,
                    }
                    .into(),
                ]),
                replacement_history_entries: None,
                mcp_resource_origins: None,
                window_number: Some(2),
                first_window_id: None,
                previous_window_id: None,
                window_id: None,
            }),
            started("child-turn"),
            RolloutItem::TurnContext(TurnContextItem {
                turn_id: Some("child-turn".to_string()),
                cwd: serde_json::from_value(json!(home.path())).expect("absolute cwd"),
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
            }),
            user_message("child question"),
            agent_message("child answer"),
            completed("child-turn"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate legacy subagent");

    let lines = read_rollout(&path);
    let RolloutItem::SessionMeta(metadata) = &lines[0].item else {
        panic!("migrated rollout should start with session metadata");
    };
    assert_eq!(
        metadata.meta.subagent_history_start_ordinal,
        Some(lines.len() as u64)
    );
    assert!(
        !fs::read_to_string(&path)
            .expect("read migrated rollout")
            .contains("superseded checkpoint")
    );
    let context = store
        .load_latest_model_context(LoadThreadHistoryParams {
            thread_id,
            include_archived: false,
        })
        .await
        .expect("load migrated model context");
    assert!(context.items.iter().any(|item| {
        matches!(item, RolloutItem::Compacted(compacted) if compacted.message == "latest checkpoint")
    }));
    assert!(
        list_active_summary_turns(&store, thread_id)
            .await
            .turns
            .is_empty()
    );
}

#[tokio::test]
async fn migration_keeps_small_uncompacted_subagent_replay_as_prefix() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::SubAgent(SubAgentSource::Other("test".to_string())),
        vec![
            user_message("child question"),
            agent_message("child answer"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate uncompacted legacy subagent");

    let lines = read_rollout(&path);
    let RolloutItem::SessionMeta(metadata) = &lines[0].item else {
        panic!("migrated rollout should start with session metadata");
    };
    assert_eq!(
        metadata.meta.subagent_history_start_ordinal,
        Some(lines.len() as u64)
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| matches!(line.item, RolloutItem::EventMsg(EventMsg::ItemCompleted(_))))
            .count(),
        2
    );
}

#[tokio::test]
async fn migration_projects_memory_consolidation_as_ordinary_history() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::SubAgent(SubAgentSource::MemoryConsolidation),
        vec![
            user_message("memory question"),
            agent_message("memory answer"),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate memory consolidation rollout");

    let lines = read_rollout(&path);
    assert!(matches!(
        &lines[0].item,
        RolloutItem::SessionMeta(metadata)
            if metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
                && metadata.meta.subagent_history_start_ordinal.is_none()
    ));
    let turns = list_active_summary_turns(&store, thread_id).await;
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].items.len(), 2);
}

#[tokio::test]
async fn dry_run_reports_migration_order() {
    let home = TempDir::new().expect("create Codex home");
    let root_id = ThreadId::new();
    let root = write_rollout(
        home.path(),
        root_id,
        SessionSource::Cli,
        vec![user_message("root question")],
    );
    let newest_directory = home.path().join("sessions/2025/01/04");
    fs::create_dir_all(&newest_directory).expect("create newest rollout directory");
    let newest_root = newest_directory.join(format!("rollout-2025-01-04T12-00-00-{root_id}.jsonl"));
    fs::rename(&root, &newest_root).expect("move root rollout to newest date");
    let root = newest_root;
    let subagent_id = ThreadId::new();
    let subagent = write_rollout(
        home.path(),
        subagent_id,
        SessionSource::SubAgent(SubAgentSource::Other("test".to_string())),
        vec![user_message("subagent question")],
    );
    let memory_id = ThreadId::new();
    let memory = write_rollout(
        home.path(),
        memory_id,
        SessionSource::SubAgent(SubAgentSource::MemoryConsolidation),
        vec![user_message("memory question")],
    );
    let memory_directory = home.path().join("sessions/2025/01/01");
    fs::create_dir_all(&memory_directory).expect("create memory rollout directory");
    let moved_memory = memory_directory.join(memory.file_name().expect("memory filename"));
    fs::rename(memory, &moved_memory).expect("move memory rollout");
    let compressed_id = ThreadId::new();
    let oldest_directory = home.path().join("sessions/2025/01/02");
    fs::create_dir_all(&oldest_directory).expect("create oldest rollout directory");
    let source_compressed_plain = write_rollout(
        home.path(),
        compressed_id,
        SessionSource::Cli,
        vec![user_message("compressed question")],
    );
    let compressed_plain =
        oldest_directory.join(format!("rollout-2025-01-02T12-00-00-{compressed_id}.jsonl"));
    fs::rename(source_compressed_plain, &compressed_plain).expect("move compressed rollout");
    let compressed = compress_rollout(&compressed_plain);
    let archived_id = ThreadId::new();
    let archived = move_to_archived(
        home.path(),
        write_rollout(
            home.path(),
            archived_id,
            SessionSource::Cli,
            vec![user_message("archived question")],
        ),
    );
    let original = fs::read(&root).expect("read original root rollout");
    let store = LocalThreadStore::new(test_config(home.path()), /*state_db*/ None);

    let mut progress = Vec::new();
    let report = store
        .migrate_rollouts_with_progress(RolloutMigrationOptions::default(), |update| {
            progress.push(update);
        })
        .await
        .expect("inspect legacy rollouts");

    let expected = vec![
        (root.clone(), root_id, RolloutMigrationStatus::Eligible),
        (subagent, subagent_id, RolloutMigrationStatus::Eligible),
        (compressed, compressed_id, RolloutMigrationStatus::Eligible),
        (moved_memory, memory_id, RolloutMigrationStatus::Eligible),
        (archived, archived_id, RolloutMigrationStatus::Eligible),
    ];
    assert_eq!(
        report
            .outcomes
            .iter()
            .map(|outcome| (
                outcome.rollout_path.clone(),
                outcome.thread_id.expect("rollout thread ID"),
                outcome.status,
            ))
            .collect::<Vec<_>>(),
        expected,
    );
    assert_eq!(
        progress.last(),
        Some(&RolloutMigrationProgress {
            processed_paths: 5,
            total_paths: 5,
            outcome_status: Some(RolloutMigrationStatus::Eligible),
        })
    );
    assert_eq!(fs::read(&root).expect("read inspected rollout"), original);

    let selected = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![root_id],
            ..RolloutMigrationOptions::default()
        })
        .await
        .expect("inspect selected rollout");
    assert_eq!(selected.outcomes.len(), 1);
    assert_eq!(selected.outcomes[0].thread_id, Some(root_id));
    assert_eq!(
        selected.outcomes[0].status,
        RolloutMigrationStatus::Eligible
    );
}

#[tokio::test]
async fn migration_publishes_compressed_sources_as_plain_refs_and_retires_the_preserved_inode() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("compressed question"),
            agent_message("compressed answer"),
        ],
    );
    let compressed_path = compress_rollout(&path);
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate compressed rollout");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert!(path.exists());
    assert!(!compressed_path.exists());
    assert_eq!(
        codex_rollout::read_session_meta_line(&path)
            .await
            .expect("read plain metadata")
            .meta
            .history_mode,
        ThreadHistoryMode::PaginatedRefsV1
    );
    assert_eq!(report.outcomes[0].rollout_path, path);
    let preserved = report.outcomes[0]
        .preserved_source_path
        .as_ref()
        .expect("compressed source is retained until explicit retirement")
        .clone();
    assert!(preserved.exists());
    assert_eq!(
        store
            .state_db
            .as_ref()
            .expect("state db")
            .get_thread(thread_id)
            .await
            .expect("read migrated metadata")
            .expect("thread metadata")
            .rollout_path,
        path
    );
    let turns = list_active_summary_turns(&store, thread_id).await;
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].items.len(), 2);

    let retirement = store
        .retire_preserved_rollout_sources_after_confirmed_quiescence(vec![thread_id])
        .await
        .expect("retire compressed source after confirmed quiescence");
    assert_eq!(retirement.outcomes.len(), 1);
    assert_eq!(
        retirement.outcomes[0].status,
        PreservedSourceRetirementStatus::Retired
    );
    assert_eq!(retirement.outcomes[0].preserved_source_path, preserved);
    assert!(!preserved.exists());
    assert!(path.exists());
    assert!(!compressed_path.exists());
}

#[tokio::test]
async fn migration_contains_an_archived_compressed_paginated_rollout_byte_for_byte() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let active_path = write_inline_paginated_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        None,
        None,
        vec![
            user_message("archived paginated question"),
            agent_message("archived paginated answer"),
        ],
    );
    let archived_path = move_to_archived(home.path(), active_path.clone());
    let compressed_path = compress_rollout(&archived_path);
    let before = fs::read(&compressed_path).expect("read compressed paginated source");
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("contain archived compressed paginated rollout");

    assert_eq!(
        report.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
    assert_eq!(report.outcomes[0].rollout_path, compressed_path);
    assert!(!active_path.exists());
    assert!(!archived_path.exists());
    assert_eq!(
        fs::read(&compressed_path).expect("read contained compressed rollout"),
        before
    );
    assert_eq!(report.outcomes[0].preserved_source_path, None);
    assert_eq!(
        codex_rollout::read_session_meta_line(&compressed_path)
            .await
            .expect("read contained archived metadata")
            .meta
            .history_mode,
        ThreadHistoryMode::Paginated
    );
    let state = store
        .state_db
        .as_ref()
        .expect("state db")
        .get_thread(thread_id)
        .await
        .expect("read archived metadata")
        .expect("thread metadata");
    assert_eq!(state.rollout_path, compressed_path);
    assert_eq!(state.history_mode, ThreadHistoryMode::Paginated);
}

#[tokio::test]
async fn migration_migrates_archived_rollouts_without_unarchiving_them() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let active_path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("archived question"),
            agent_message("archived answer"),
        ],
    );
    let archived_path = move_to_archived(home.path(), active_path.clone());
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate archived rollout");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert!(!active_path.exists());
    assert!(archived_path.exists());
    assert!(matches!(
        &read_rollout(&archived_path)[0].item,
        RolloutItem::SessionMeta(metadata)
            if metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
    ));
    let turns = store
        .list_turns(ListTurnsParams {
            thread_id,
            include_archived: true,
            cursor: None,
            page_size: 10,
            sort_direction: SortDirection::Asc,
            items_view: StoredTurnItemsView::Summary,
        })
        .await
        .expect("read archived projected turns");
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].items.len(), 2);
}

#[tokio::test]
async fn migration_retries_a_rollout_moved_after_path_discovery() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let active_path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("question"), agent_message("answer")],
    );
    let store = indexed_store(home.path()).await;
    let archived_path = move_to_archived(home.path(), active_path.clone());

    let report = store
        .migrate_rollouts_with_progress_for_trigger(
            apply_options(),
            |_| {},
            RolloutMigrationTrigger::Startup,
            RolloutMigrationPaths::Known(vec![active_path]),
        )
        .await
        .expect("migrate moved rollout");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert!(matches!(
        &read_rollout(&archived_path)[0].item,
        RolloutItem::SessionMeta(metadata)
            if metadata.meta.history_mode == ThreadHistoryMode::PaginatedRefsV1
    ));
}

#[tokio::test]
async fn migration_preserves_legacy_displayed_thread_names() {
    let home = TempDir::new().expect("create Codex home");
    let title_thread_id = ThreadId::new();
    write_rollout(
        home.path(),
        title_thread_id,
        SessionSource::Cli,
        vec![user_message("title question")],
    );
    let index_thread_id = ThreadId::new();
    write_rollout(
        home.path(),
        index_thread_id,
        SessionSource::Cli,
        vec![user_message("index question")],
    );
    let store = indexed_store(home.path()).await;
    store
        .update_thread_metadata(UpdateThreadMetadataParams {
            thread_id: title_thread_id,
            patch: ThreadMetadataPatch {
                name: Some(Some("renamed title".to_string())),
                ..Default::default()
            },
            include_archived: false,
        })
        .await
        .expect("rename legacy thread");
    codex_rollout::append_thread_name(home.path(), title_thread_id, "stale index title")
        .await
        .expect("write stale legacy index name");
    codex_rollout::append_thread_name(home.path(), index_thread_id, "indexed title")
        .await
        .expect("write legacy index name");

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate named rollouts");

    let page = store
        .list_threads(ListThreadsParams {
            page_size: 10,
            cursor: None,
            sort_key: ThreadSortKey::CreatedAt,
            sort_direction: SortDirection::Desc,
            allowed_sources: Vec::new(),
            model_providers: None,
            cwd_filters: None,
            section: None,
            project_id: None,
            archived: false,
            search_term: None,
            relation_filter: None,
            use_state_db_only: true,
        })
        .await
        .expect("list migrated threads");
    let title_thread = page
        .items
        .iter()
        .find(|thread| thread.thread_id == title_thread_id)
        .expect("renamed title thread");
    let index_thread = page
        .items
        .iter()
        .find(|thread| thread.thread_id == index_thread_id)
        .expect("indexed title thread");

    assert_eq!(title_thread.name.as_deref(), Some("renamed title"));
    assert_eq!(index_thread.name.as_deref(), Some("indexed title"));
}

#[tokio::test]
async fn migration_repairs_a_missing_paginated_name_when_rerun() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("question")],
    );
    let store = indexed_store(home.path()).await;
    store
        .update_thread_metadata(UpdateThreadMetadataParams {
            thread_id,
            patch: ThreadMetadataPatch {
                name: Some(Some("renamed title".to_string())),
                ..Default::default()
            },
            include_archived: false,
        })
        .await
        .expect("rename legacy thread");
    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate named rollout");
    let state_db = store.state_db().await.expect("state runtime");
    state_db
        .update_thread_title(thread_id, "question")
        .await
        .expect("restore derived title");
    state_db
        .update_thread_name(thread_id, /*name*/ None)
        .await
        .expect("clear migrated name");

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("repair migrated name");

    assert_eq!(
        report.outcomes[0].status,
        RolloutMigrationStatus::AlreadyPaginated
    );
    assert_eq!(
        state_db
            .get_thread(thread_id)
            .await
            .expect("read repaired metadata")
            .expect("repaired thread")
            .name
            .as_deref(),
        Some("renamed title")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn decompression_temporaries_are_owner_only() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let compressed_path = compress_rollout(&write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("compressed question")],
    ));
    let plain_path = home.path().join("decompressed.tmp");

    decompress_rollout_to_path(&compressed_path, &plain_path)
        .await
        .expect("decompress rollout");

    assert_eq!(
        fs::metadata(&plain_path)
            .expect("read temporary metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[tokio::test]
async fn migration_skips_threads_with_an_active_writer() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("active question")],
    );
    let store = LocalThreadStore::new(test_config(home.path()), /*state_db*/ None);
    let _writer = store
        .writer_lock_coordinator
        .acquire(thread_id)
        .expect("acquire live writer lock");
    let original = fs::read(&path).expect("read active rollout");

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("inspect active writer");

    assert_eq!(
        report.outcomes[0].status,
        RolloutMigrationStatus::SkippedBusy
    );
    assert_eq!(fs::read(&path).expect("read unmodified rollout"), original);
}

#[tokio::test]
async fn migration_apply_conflicts_with_rollout_maintenance() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("maintenance question")],
    );
    let original = fs::read(&path).expect("read legacy rollout");
    let _maintenance_guard = codex_rollout::try_acquire_rollout_maintenance_lock(home.path())
        .expect("acquire rollout maintenance lock")
        .expect("claim rollout maintenance lock");
    let store = LocalThreadStore::new(test_config(home.path()), /*state_db*/ None);

    let error = store
        .migrate_rollouts(apply_options())
        .await
        .expect_err("reject concurrent rollout maintenance");

    assert!(matches!(error, ThreadStoreError::Conflict { .. }));
    assert_eq!(fs::read(&path).expect("read untouched rollout"), original);
}

#[tokio::test]
async fn copied_home_retained_journal_cannot_reach_original_home_or_mutate_copy() {
    let original_home = TempDir::new().expect("create original Codex home");
    let relocated_home = TempDir::new().expect("create relocated Codex home");
    let thread_id = ThreadId::new();
    let original_path = write_rollout(
        original_home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("relocated recovery question"),
            agent_message("relocated recovery answer"),
        ],
    );
    let original_store = indexed_store(original_home.path()).await;
    let migrated = original_store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("migrate original Codex home");
    assert_eq!(
        migrated.outcomes[0].status,
        RolloutMigrationStatus::Migrated
    );
    let original_published_path = migrated.outcomes[0].rollout_path.clone();
    let original_preserved_path = migrated.outcomes[0]
        .preserved_source_path
        .clone()
        .expect("retain original legacy source");
    let original_retained_path = retained_migration_recovery_path(original_home.path(), thread_id);
    assert!(original_published_path.exists());
    assert!(original_preserved_path.exists());
    assert!(original_retained_path.exists());
    drop(original_store);

    copy_directory_tree(original_home.path(), relocated_home.path());
    let relocated_published_path = relocated_home.path().join(
        original_published_path
            .strip_prefix(original_home.path())
            .expect("published rollout belongs to original home"),
    );
    let relocated_preserved_path = relocated_home.path().join(
        original_preserved_path
            .strip_prefix(original_home.path())
            .expect("preserved rollout belongs to original home"),
    );
    let relocated_retained_path =
        retained_migration_recovery_path(relocated_home.path(), thread_id);
    let relocated_store = indexed_store(relocated_home.path()).await;
    let original_before = file_tree_snapshot(original_home.path());
    let published_before = fs::read(&relocated_published_path).expect("read copied publication");
    let preserved_before = fs::read(&relocated_preserved_path).expect("read copied preservation");
    let retained_before = fs::read(&relocated_retained_path).expect("read copied journal");
    let metadata_before = relocated_store
        .state_db
        .as_ref()
        .expect("state db")
        .get_thread(thread_id)
        .await
        .expect("read copied thread metadata")
        .expect("copied thread metadata");
    let projection_before = thread_history::projection_state(&relocated_store, thread_id)
        .await
        .expect("read copied projection")
        .expect("copied projection");

    let refused = relocated_store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("report copied absolute-path recovery conflict");

    let outcome = &refused.outcomes[0];
    assert_eq!(outcome.status, RolloutMigrationStatus::Failed);
    assert_eq!(
        outcome.failure_reason,
        Some(RolloutMigrationFailureReason::RecoveryStateConflict)
    );
    assert!(
        outcome
            .message
            .as_deref()
            .is_some_and(|message| message.contains("outside this Codex home"))
    );
    assert_eq!(file_tree_snapshot(original_home.path()), original_before);
    assert_eq!(
        fs::read(&relocated_published_path).expect("read untouched copied publication"),
        published_before
    );
    assert_eq!(
        fs::read(&relocated_preserved_path).expect("read untouched copied preservation"),
        preserved_before
    );
    assert_eq!(
        fs::read(&relocated_retained_path).expect("read untouched copied journal"),
        retained_before
    );
    let metadata_after = relocated_store
        .state_db
        .as_ref()
        .expect("state db")
        .get_thread(thread_id)
        .await
        .expect("read unchanged copied thread metadata")
        .expect("copied thread metadata");
    assert_eq!(metadata_after.history_mode, metadata_before.history_mode);
    assert_eq!(metadata_after.rollout_path, metadata_before.rollout_path);
    let projection_after = thread_history::projection_state(&relocated_store, thread_id)
        .await
        .expect("read unchanged copied projection")
        .expect("copied projection");
    assert_eq!(
        (
            projection_after.next_byte_offset,
            projection_after.next_ordinal
        ),
        (
            projection_before.next_byte_offset,
            projection_before.next_ordinal
        )
    );
    assert_eq!(original_path, original_published_path);
}

#[tokio::test]
async fn copied_home_pending_publication_without_sqlite_rejects_old_paths_without_mutation() {
    let original_home = TempDir::new().expect("create original Codex home");
    let relocated_home = TempDir::new().expect("create relocated Codex home");
    let thread_id = ThreadId::new();
    write_rollout(
        original_home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("pending relocation question"),
            agent_message("pending relocation answer"),
        ],
    );
    let original_store = indexed_store(original_home.path()).await;
    let migrated = original_store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("migrate original Codex home");
    assert_eq!(
        migrated.outcomes[0].status,
        RolloutMigrationStatus::Migrated
    );
    let original_retained_path = retained_migration_recovery_path(original_home.path(), thread_id);
    let original_pending_path = migration_journal_path(original_home.path(), thread_id);
    drop(original_store);
    fs::rename(&original_retained_path, &original_pending_path)
        .expect("simulate pending publication recovery");

    copy_directory_tree(original_home.path(), relocated_home.path());
    let relocated_pending_path = migration_journal_path(relocated_home.path(), thread_id);
    assert!(relocated_pending_path.exists());
    assert!(!retained_migration_recovery_path(relocated_home.path(), thread_id).exists());
    let relocated_store =
        LocalThreadStore::new(test_config(relocated_home.path()), /*state_db*/ None);
    let original_before = file_tree_snapshot(original_home.path());
    let relocated_before = file_tree_snapshot(relocated_home.path());

    let refused = relocated_store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("report copied pending recovery conflict without SQLite");

    let outcome = &refused.outcomes[0];
    assert_eq!(outcome.status, RolloutMigrationStatus::Failed);
    assert_eq!(
        outcome.failure_reason,
        Some(RolloutMigrationFailureReason::RecoveryStateConflict)
    );
    assert!(
        outcome
            .message
            .as_deref()
            .is_some_and(|message| message.contains("outside this Codex home"))
    );
    assert_eq!(file_tree_snapshot(original_home.path()), original_before);
    assert_eq!(file_tree_snapshot(relocated_home.path()), relocated_before);
}

#[tokio::test]
async fn late_preserved_append_is_marked_and_later_migration_refuses_both_generations() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("split generation question"),
            agent_message("split generation answer"),
        ],
    );
    let mut old_writer = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open legacy writer before migration");
    let store = indexed_store(home.path()).await;
    let migrated = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("migrate before late legacy append");
    assert_eq!(
        migrated.outcomes[0].status,
        RolloutMigrationStatus::Migrated
    );
    let published_path = migrated.outcomes[0].rollout_path.clone();
    let preserved_path = migrated.outcomes[0]
        .preserved_source_path
        .clone()
        .expect("retain legacy source after migration");
    let retained_path = retained_migration_recovery_path(home.path(), thread_id);
    let published_before = fs::read(&published_path).expect("read published generation");
    let preserved_before = fs::read(&preserved_path).expect("read preserved generation");

    let late_record = serialized_response_record(input_response_message(
        "user",
        "late append from pre-migration writer",
    ));
    old_writer
        .write_all(&late_record)
        .expect("append through pre-migration writer");
    old_writer.sync_all().expect("sync late legacy append");
    drop(old_writer);
    let mut preserved_after_append = preserved_before.clone();
    preserved_after_append.extend_from_slice(&late_record);
    assert_eq!(
        fs::read(&preserved_path).expect("read late preserved append"),
        preserved_after_append
    );
    assert_eq!(
        fs::read(&published_path).expect("read isolated published generation"),
        published_before
    );

    let detected = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("detect late legacy append");
    let detected = &detected.outcomes[0];
    assert_eq!(detected.status, RolloutMigrationStatus::Failed);
    assert_eq!(
        detected.failure_reason,
        Some(RolloutMigrationFailureReason::RecoveryStateConflict)
    );
    assert!(detected.message.as_deref().is_some_and(|message| {
        message.contains("older writer changed the preserved legacy generation")
            && message.contains("both generations were preserved")
    }));
    let marked_journal = fs::read(&retained_path).expect("read split-generation marker");
    let marked: serde_json::Value =
        serde_json::from_slice(&marked_journal).expect("parse split-generation marker");
    assert_eq!(marked["split_generation_detected"], json!(true));
    assert!(marked.get("retirement_started").is_none());
    assert_eq!(
        fs::read(&published_path).expect("read preserved publication after detection"),
        published_before
    );
    assert_eq!(
        fs::read(&preserved_path).expect("read preserved legacy after detection"),
        preserved_after_append
    );

    let refused = store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![thread_id],
            ..apply_options()
        })
        .await
        .expect("refuse marked split generations");
    let refused = &refused.outcomes[0];
    assert_eq!(refused.status, RolloutMigrationStatus::Failed);
    assert_eq!(
        refused.failure_reason,
        Some(RolloutMigrationFailureReason::RecoveryStateConflict)
    );
    assert!(refused.message.as_deref().is_some_and(|message| {
        message.contains("legacy and paginated generations have diverged")
            && message.contains("both were preserved for explicit reconciliation")
    }));
    assert_eq!(
        fs::read(&published_path).expect("read final published generation"),
        published_before
    );
    assert_eq!(
        fs::read(&preserved_path).expect("read final preserved generation"),
        preserved_after_append
    );
    assert_eq!(
        fs::read(&retained_path).expect("read monotonic split-generation marker"),
        marked_journal
    );
}

#[tokio::test]
async fn migration_recovers_a_published_rollout_with_missing_projection() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("recover question"),
            agent_message("recover answer"),
        ],
    );
    let store = indexed_store(home.path()).await;
    store
        .migrate_rollouts(apply_options())
        .await
        .expect("publish canonical rollout");
    thread_history::delete_thread(&store, thread_id)
        .await
        .expect("simulate interrupted projection");
    let journal_path = migration_journal_path(home.path(), thread_id);
    write_migration_journal(&journal_path)
        .await
        .expect("simulate pending migration journal");

    let writer = store
        .writer_lock_coordinator
        .acquire(thread_id)
        .expect("acquire live writer lock");
    let busy = store
        .migrate_rollouts(apply_options())
        .await
        .expect("inspect busy published recovery");
    assert_eq!(busy.outcomes[0].status, RolloutMigrationStatus::SkippedBusy);
    assert!(journal_path.exists());
    drop(writer);

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("recover published rollout");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert!(!journal_path.exists());
    let projection = thread_history::projection_state(&store, thread_id)
        .await
        .expect("read repaired projection")
        .expect("projection was rebuilt");
    assert_eq!(
        projection.next_byte_offset,
        fs::metadata(&path).expect("read rollout metadata").len()
    );
}

#[tokio::test]
async fn migration_recovers_pending_rollouts_before_new_work() {
    let home = TempDir::new().expect("create Codex home");
    let pending_thread_id = ThreadId::new();
    write_rollout(
        home.path(),
        pending_thread_id,
        SessionSource::Cli,
        vec![user_message("pending question")],
    );
    let new_thread_id = ThreadId::new();
    let new_path = write_rollout(
        home.path(),
        new_thread_id,
        SessionSource::Cli,
        vec![user_message("new question")],
    );
    let newer_directory = home.path().join("sessions/2025/01/04");
    fs::create_dir_all(&newer_directory).expect("create newer rollout directory");
    fs::rename(
        &new_path,
        newer_directory.join(new_path.file_name().expect("rollout filename")),
    )
    .expect("move newer rollout");
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(RolloutMigrationOptions {
            thread_ids: vec![pending_thread_id],
            ..apply_options()
        })
        .await
        .expect("publish pending rollout");
    thread_history::delete_thread(&store, pending_thread_id)
        .await
        .expect("simulate missing projection");
    write_migration_journal(&migration_journal_path(home.path(), pending_thread_id))
        .await
        .expect("simulate pending migration journal");

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("recover pending rollout before new work");

    assert_eq!(
        report
            .outcomes
            .iter()
            .map(|outcome| (
                outcome.thread_id.expect("rollout thread ID"),
                outcome.status
            ))
            .collect::<Vec<_>>(),
        vec![
            (pending_thread_id, RolloutMigrationStatus::Migrated),
            (new_thread_id, RolloutMigrationStatus::Migrated),
        ]
    );
}

#[tokio::test]
async fn migration_recovers_a_compressed_published_rollout() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            user_message("recover compressed question"),
            agent_message("recover compressed answer"),
        ],
    );
    let store = indexed_store(home.path()).await;
    store
        .migrate_rollouts(apply_options())
        .await
        .expect("publish canonical rollout");
    thread_history::delete_thread(&store, thread_id)
        .await
        .expect("simulate interrupted projection");
    let state_db = store.state_db.as_ref().expect("state db");
    let mut legacy_metadata = state_db
        .get_thread(thread_id)
        .await
        .expect("read thread metadata")
        .expect("thread metadata");
    legacy_metadata.history_mode = ThreadHistoryMode::Legacy;
    state_db
        .delete_thread(thread_id)
        .await
        .expect("remove paginated thread metadata");
    assert!(
        state_db
            .insert_thread_if_absent(&legacy_metadata)
            .await
            .expect("restore legacy thread metadata")
    );
    let journal_path = migration_journal_path(home.path(), thread_id);
    write_migration_journal(&journal_path)
        .await
        .expect("simulate pending migration journal");
    let compressed_path = compress_rollout(&path);

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("recover compressed published rollout");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert!(!path.exists());
    assert!(compressed_path.exists());
    assert!(!journal_path.exists());
    assert_eq!(
        codex_rollout::read_session_meta_line(&compressed_path)
            .await
            .expect("read recovered canonical head")
            .meta
            .history_mode,
        ThreadHistoryMode::PaginatedRefsV1
    );
    assert_eq!(
        store
            .state_db
            .as_ref()
            .expect("state db")
            .get_thread(thread_id)
            .await
            .expect("read repaired thread metadata")
            .expect("thread metadata")
            .history_mode,
        ThreadHistoryMode::Paginated
    );
    let projection = thread_history::projection_state(&store, thread_id)
        .await
        .expect("read repaired projection")
        .expect("projection was rebuilt");
    assert_eq!(
        projection.next_byte_offset,
        zstd::stream::decode_all(fs::File::open(&compressed_path).expect("open rollout"))
            .expect("decompress rollout")
            .len() as u64
    );
}

#[tokio::test]
async fn migration_repairs_newline_split_json_string_without_losing_text() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(home.path(), thread_id, SessionSource::Cli, Vec::new());
    let expected_text = [
        "fragment one",
        "fragment two",
        "fragment three",
        "fragment four",
        "fragment five",
        "fragment six",
        "fragment seven",
        "fragment eight",
        "fragment nine",
    ]
    .join("\n");
    let expected_item = ResponseItem::CustomToolCallOutput {
        id: None,
        call_id: "split-output".to_string(),
        name: Some("custom".to_string()),
        output: FunctionCallOutputPayload::from_text(expected_text.clone()),
        internal_chat_message_metadata_passthrough: None,
    };
    let fractured =
        fracture_json_string_newlines(&serialized_response_record(expected_item.clone()));
    assert_eq!(fractured.iter().filter(|byte| **byte == b'\n').count(), 9);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open legacy rollout")
        .write_all(&fractured)
        .expect("append fractured response record");
    let source_bytes = fs::read(&path).expect("read fractured source");
    let store = indexed_store(home.path()).await;

    let dry_run = store
        .migrate_rollouts(RolloutMigrationOptions::default())
        .await
        .expect("dry-run fractured response record");
    assert_eq!(dry_run.outcomes[0].status, RolloutMigrationStatus::Eligible);
    assert_eq!(
        fs::read(&path).expect("read source after dry-run"),
        source_bytes
    );

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate fractured response record");
    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert_eq!(
        read_rollout(&path)
            .into_iter()
            .filter_map(|line| match line.item {
                RolloutItem::ResponseItem(item) => Some(response_item_without_id(item.item)),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![expected_item]
    );
}

#[tokio::test]
async fn newline_repaired_retired_record_fails_closed_instead_of_being_skipped() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(home.path(), thread_id, SessionSource::Cli, Vec::new());
    let retired_record = format!(
        r#"{{"timestamp":"{TIMESTAMP}","type":"event_msg","payload":{{"type":"thread_name_updated","name":"before\nafter"}}}}
"#
    );
    let fractured = fracture_json_string_newlines(retired_record.as_bytes());
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open legacy rollout")
        .write_all(&fractured)
        .expect("append fractured retired record");
    let source = source_snapshot(&path);
    let store = indexed_store(home.path()).await;

    let dry_run = store
        .migrate_rollouts(RolloutMigrationOptions::default())
        .await
        .expect("dry-run fractured retired record");
    let apply = store
        .migrate_rollouts(apply_options())
        .await
        .expect("apply fractured retired record");

    let dry_run = &dry_run.outcomes[0];
    let apply = &apply.outcomes[0];
    assert_failed_with_reason(
        dry_run,
        RolloutMigrationFailureReason::MalformedRolloutRecord,
    );
    assert_eq!(
        (
            dry_run.status,
            dry_run.failure_reason,
            dry_run.bytes_processed,
            dry_run.message.as_deref(),
        ),
        (
            apply.status,
            apply.failure_reason,
            apply.bytes_processed,
            apply.message.as_deref(),
        )
    );
    assert!(dry_run.message.as_deref().is_some_and(|message| {
        message.contains("newline-repaired record is not a supported rollout line")
    }));
    assert_failed_migration_preserved_source(&store, home.path(), thread_id, &path, &source).await;
}

#[tokio::test]
async fn newline_repaired_logical_record_over_limit_blocks_dry_run_and_apply() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(home.path(), thread_id, SessionSource::Cli, Vec::new());
    let response_item = |text: String| ResponseItem::CustomToolCallOutput {
        id: None,
        call_id: "oversized-split-output".to_string(),
        name: Some("custom".to_string()),
        output: FunctionCallOutputPayload::from_text(text),
        internal_chat_message_metadata_passthrough: None,
    };
    let base_text = "prefix\n\nsuffix";
    let base_record = serialized_response_record(response_item(base_text.to_string()));
    let observed_bytes = super::MAX_ROLLOUT_LINE_BYTES + 1;
    let filler_bytes = observed_bytes
        .checked_sub(base_record.len())
        .expect("response record fits below migration safety limit");
    let text = format!("prefix\n{}\nsuffix", "x".repeat(filler_bytes));
    let logical_record = serialized_response_record(response_item(text));
    assert_eq!(logical_record.len(), observed_bytes);
    let fractured = fracture_json_string_newlines(&logical_record);
    let byte_offset = fs::metadata(&path)
        .expect("read metadata record size")
        .len();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open legacy rollout")
        .write_all(&fractured)
        .expect("append oversized fractured response record");
    let source = source_snapshot(&path);
    let store = indexed_store(home.path()).await;

    let dry_run = store
        .migrate_rollouts(RolloutMigrationOptions::default())
        .await
        .expect("dry-run oversized fractured response record");
    let apply = store
        .migrate_rollouts(apply_options())
        .await
        .expect("apply oversized fractured response record");

    let dry_run = &dry_run.outcomes[0];
    let apply = &apply.outcomes[0];
    assert_failed_with_reason(
        dry_run,
        RolloutMigrationFailureReason::OversizedRolloutRecord,
    );
    assert_eq!(
        (
            dry_run.status,
            dry_run.failure_reason,
            dry_run.bytes_processed,
            dry_run.message.as_deref(),
        ),
        (
            apply.status,
            apply.failure_reason,
            apply.bytes_processed,
            apply.message.as_deref(),
        )
    );
    assert!(
        dry_run
            .message
            .as_deref()
            .is_some_and(|message| message.contains(&format!(
                "rollout record at ordinal 1, byte offset {byte_offset} is {observed_bytes} bytes, exceeding the {}-byte limit",
                super::MAX_ROLLOUT_LINE_BYTES
            )))
    );
    assert_failed_migration_preserved_source(&store, home.path(), thread_id, &path, &source).await;
}

#[tokio::test]
async fn migration_accepts_a_complete_record_at_the_safety_limit() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("kept question")],
    );
    let original = fs::read(&path).expect("read original rollout");
    let mut source_bytes = sized_retired_record(super::MAX_ROLLOUT_LINE_BYTES);
    source_bytes.extend_from_slice(&original);
    fs::write(&path, source_bytes).expect("prepend safety-limit rollout record");
    let source = source_snapshot(&path);
    let store = indexed_store(home.path()).await;

    let dry_run = store
        .migrate_rollouts(RolloutMigrationOptions::default())
        .await
        .expect("validate safety-limit rollout record");
    assert_eq!(dry_run.outcomes[0].status, RolloutMigrationStatus::Eligible);
    assert_eq!(fs::read(&path).expect("read dry-run source"), source.bytes);

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate safety-limit rollout record");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert!(!migration_journal_path(home.path(), thread_id).exists());
}

#[tokio::test]
async fn oversized_record_blocks_dry_run_and_apply_without_mutating_the_source() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("kept question")],
    );
    let observed_bytes = super::MAX_ROLLOUT_LINE_BYTES + 1;
    let original = fs::read(&path).expect("read original rollout");
    let mut source_bytes = sized_retired_record(observed_bytes);
    source_bytes.extend_from_slice(&original);
    fs::write(&path, source_bytes).expect("prepend oversized rollout record");
    let byte_offset = 0;
    let source = source_snapshot(&path);
    let store = indexed_store(home.path()).await;

    let dry_run = store
        .migrate_rollouts(RolloutMigrationOptions::default())
        .await
        .expect("dry-run oversized rollout record");
    let apply = store
        .migrate_rollouts(apply_options())
        .await
        .expect("apply oversized rollout record");

    let dry_run = &dry_run.outcomes[0];
    let apply = &apply.outcomes[0];
    assert_failed_with_reason(
        dry_run,
        RolloutMigrationFailureReason::OversizedRolloutRecord,
    );
    assert_eq!(
        (
            dry_run.status,
            dry_run.failure_reason,
            dry_run.bytes_processed,
            dry_run.message.as_deref(),
        ),
        (
            apply.status,
            apply.failure_reason,
            apply.bytes_processed,
            apply.message.as_deref(),
        )
    );
    let expected_message = format!(
        "thread-store internal error: rollout migration failed: rollout record at ordinal 0, byte offset {byte_offset} is {observed_bytes} bytes, exceeding the {}-byte limit",
        super::MAX_ROLLOUT_LINE_BYTES
    );
    assert_eq!(dry_run.message.as_deref(), Some(expected_message.as_str()));
    assert_failed_migration_preserved_source(&store, home.path(), thread_id, &path, &source).await;
}

#[tokio::test]
async fn migration_skips_empty_rollout_files() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let directory = home.path().join("sessions/2025/01/03");
    fs::create_dir_all(&directory).expect("create rollout directory");
    let path = directory.join(format!("rollout-2025-01-03T12-00-00-{thread_id}.jsonl"));
    fs::File::create(&path).expect("create empty rollout");
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("inspect empty rollout");

    assert_eq!(
        report.outcomes[0].status,
        RolloutMigrationStatus::SkippedEmpty
    );
    assert_eq!(fs::metadata(&path).expect("read empty rollout").len(), 0);
    assert!(
        store
            .state_db
            .as_ref()
            .expect("state db")
            .get_thread(thread_id)
            .await
            .expect("read thread metadata")
            .is_none()
    );
}

#[tokio::test]
async fn migration_reports_missing_sqlite_metadata() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("question")],
    );
    let store = indexed_store(home.path()).await;
    store
        .state_db
        .as_ref()
        .expect("state db")
        .delete_thread(thread_id)
        .await
        .expect("remove thread metadata");

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("inspect rollout with missing metadata");

    assert_failed_with_reason(
        &report.outcomes[0],
        RolloutMigrationFailureReason::MissingSqliteMetadata,
    );
}

#[tokio::test]
async fn migration_reports_invalid_session_metadata() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let directory = home.path().join("sessions/2025/01/03");
    fs::create_dir_all(&directory).expect("create rollout directory");
    let path = directory.join(format!("rollout-2025-01-03T12-00-00-{thread_id}.jsonl"));
    let line = RolloutLine {
        timestamp: TIMESTAMP.to_string(),
        ordinal: None,
        item: user_message("message before missing session metadata"),
    };
    let serialized = serde_json::to_string(&line).expect("serialize metadata-free rollout");
    fs::write(path, format!("{serialized}\n")).expect("write metadata-free rollout");
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("inspect rollout with invalid metadata");

    assert_failed_with_reason(
        &report.outcomes[0],
        RolloutMigrationFailureReason::InvalidSessionMetadata,
    );
}

#[tokio::test]
async fn complete_malformed_record_blocks_dry_run_and_apply_without_resynchronizing() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("kept question")],
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open legacy rollout");
    let malformed = b"{not valid rollout json\n";
    let byte_offset = fs::metadata(&path).expect("read rollout metadata").len();
    file.write_all(malformed).expect("append malformed record");
    let later_valid_line = RolloutLine {
        timestamp: TIMESTAMP.to_string(),
        ordinal: None,
        item: agent_message("kept answer"),
    };
    writeln!(
        file,
        "{}",
        serde_json::to_string(&later_valid_line).expect("serialize later valid record")
    )
    .expect("append later valid record");
    drop(file);
    let source = source_snapshot(&path);
    let store = indexed_store(home.path()).await;

    let dry_run = store
        .migrate_rollouts(RolloutMigrationOptions::default())
        .await
        .expect("dry-run malformed legacy rollout");
    let apply = store
        .migrate_rollouts(apply_options())
        .await
        .expect("apply malformed legacy rollout");

    let dry_run = &dry_run.outcomes[0];
    let apply = &apply.outcomes[0];
    assert_failed_with_reason(
        dry_run,
        RolloutMigrationFailureReason::MalformedRolloutRecord,
    );
    assert_eq!(
        (
            dry_run.status,
            dry_run.failure_reason,
            dry_run.bytes_processed,
            dry_run.message.as_deref(),
        ),
        (
            apply.status,
            apply.failure_reason,
            apply.bytes_processed,
            apply.message.as_deref(),
        )
    );
    assert!(
        dry_run
            .message
            .as_deref()
            .is_some_and(|message| message.contains(&format!(
                "rollout record at ordinal 2, byte offset {byte_offset} is malformed ({} bytes)",
                malformed.len()
            )))
    );
    assert_failed_migration_preserved_source(&store, home.path(), thread_id, &path, &source).await;
}

#[tokio::test]
async fn unresolved_compaction_reference_fails_closed_and_preserves_source() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let missing_id = "msg_missing-compaction-source";
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![RolloutItem::Compacted(CompactedItem {
            message: "broken checkpoint".to_string(),
            replacement_history: None,
            replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
                item_id: missing_id.to_string(),
            }]),
            mcp_resource_origins: None,
            window_number: Some(1),
            first_window_id: None,
            previous_window_id: None,
            window_id: None,
        })],
    );
    let source = source_snapshot(&path);
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("report unresolved compaction reference");

    let outcome = &report.outcomes[0];
    assert_failed_with_reason(
        outcome,
        RolloutMigrationFailureReason::LegacyRolloutConversionFailed,
    );
    assert!(
        outcome
            .message
            .as_deref()
            .is_some_and(|message| message.contains(missing_id))
    );
    assert_failed_migration_preserved_source(&store, home.path(), thread_id, &path, &source).await;
}

#[tokio::test]
async fn migration_preserves_unresolved_superseded_checkpoint_without_selecting_it() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let missing_id = "msg_missing-superseded-source";
    let carried_source = input_response_message_with_id(
        "user",
        "superseded-inline-source",
        vec![ContentItem::InputText {
            text: "selected history".to_string(),
        }],
    );
    let superseded = RolloutItem::Compacted(CompactedItem {
        message: "superseded checkpoint".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![
            CompactedHistoryEntry::Reference {
                item_id: missing_id.to_string(),
            },
            CompactedHistoryEntry::Inline {
                item: Box::new(carried_source.clone()),
                metadata: None,
            },
        ]),
        mcp_resource_origins: None,
        window_number: Some(1),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    });
    let selected = compacted(vec![carried_source.clone()]);
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            superseded,
            selected,
            // Force the rollback-aware two-pass path without removing a turn. The reverse replay
            // selector must validate the newer checkpoint, not every older retained checkpoint.
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 0,
            })),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate rollout with superseded dangling checkpoint");

    let checkpoints = read_rollout(&path)
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(item) => Some(item),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(
        checkpoints[0].replacement_history_entries,
        Some(vec![
            CompactedHistoryEntry::Reference {
                item_id: missing_id.to_string(),
            },
            CompactedHistoryEntry::Inline {
                item: Box::new(carried_source.clone()),
                metadata: None,
            },
        ])
    );
    assert_eq!(checkpoints[1].replacement_history_entries, None);
    assert_eq!(
        checkpoints[1]
            .replacement_history
            .as_ref()
            .expect("selected checkpoint remains complete")
            .iter()
            .map(|item| item.item.clone())
            .collect::<Vec<_>>(),
        vec![carried_source]
    );
}

#[tokio::test]
async fn ordinary_migration_ignores_dangling_reference_before_newer_valid_checkpoint() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let missing_id = "msg_missing-ordinary-superseded-source";
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            RolloutItem::Compacted(CompactedItem {
                message: "superseded checkpoint".to_string(),
                replacement_history: None,
                replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
                    item_id: missing_id.to_string(),
                }]),
                mcp_resource_origins: None,
                window_number: Some(1),
                first_window_id: None,
                previous_window_id: None,
                window_id: None,
            }),
            compacted(vec![input_response_message("user", "newer valid history")]),
        ],
    );
    let store = indexed_store(home.path()).await;

    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate ordinary rollout with superseded dangling checkpoint");

    let checkpoints = read_rollout(&path)
        .into_iter()
        .filter_map(|line| match line.item {
            RolloutItem::Compacted(item) => Some(item),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(
        checkpoints[0].replacement_history_entries,
        Some(vec![CompactedHistoryEntry::Reference {
            item_id: missing_id.to_string(),
        }])
    );
}

#[tokio::test]
async fn migration_matches_rollout_loader_when_removing_nested_ghost_snapshot() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(home.path(), thread_id, SessionSource::Cli, Vec::new());
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append legacy checkpoint");
    writeln!(
        file,
        "{}",
        json!({
            "timestamp": TIMESTAMP,
            "type": "compacted",
            "payload": {
                "message": "legacy checkpoint",
                "replacement_history": [
                    {
                        "type": "message",
                        "role": "developer",
                        "content": [{"type": "input_text", "text": "kept developer"}]
                    },
                    {"type": "ghost_snapshot", "ghost_commit": {"id": "deadbeef"}},
                    {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "kept user"}]
                    }
                ],
                "replacement_history_metadata": [
                    {"client_authored": true},
                    {"client_authored": false},
                    {"client_authored": false}
                ]
            }
        })
    )
    .expect("write legacy checkpoint");
    drop(file);

    let (loaded, loaded_thread_id, parse_errors) = RolloutRecorder::load_rollout_items(&path)
        .await
        .expect("load legacy rollout through recorder");
    assert_eq!(loaded_thread_id, Some(thread_id));
    assert_eq!(parse_errors, 0);
    let before = loaded
        .into_iter()
        .find_map(|item| match item {
            RolloutItem::Compacted(compacted) => compacted.replacement_history,
            _ => None,
        })
        .expect("loader-normalized history");

    let store = indexed_store(home.path()).await;
    store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate nested ghost snapshot");

    let after = read_rollout(&path)
        .into_iter()
        .find_map(|line| match line.item {
            RolloutItem::Compacted(compacted) => compacted.replacement_history,
            _ => None,
        })
        .expect("migration-normalized history");
    assert_eq!(envelopes_without_ids(after.clone()), before);
    assert_eq!(
        after
            .iter()
            .map(|envelope| envelope
                .metadata
                .as_ref()
                .map(|metadata| metadata.client_authored))
            .collect::<Vec<_>>(),
        vec![Some(true), Some(false)]
    );
}

#[tokio::test]
async fn rollback_aware_selected_checkpoint_still_fails_on_unresolved_reference() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let missing_id = "msg_missing-selected-source";
    let selected = RolloutItem::Compacted(CompactedItem {
        message: "selected broken checkpoint".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
            item_id: missing_id.to_string(),
        }]),
        mcp_resource_origins: None,
        window_number: Some(2),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    });
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![
            compacted(vec![input_response_message("user", "older valid history")]),
            selected,
            RolloutItem::EventMsg(EventMsg::ThreadRolledBack(ThreadRolledBackEvent {
                num_turns: 0,
            })),
        ],
    );
    let source = source_snapshot(&path);
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("report rollback-aware selected dangling checkpoint");

    let outcome = &report.outcomes[0];
    assert_failed_with_reason(
        outcome,
        RolloutMigrationFailureReason::LegacyRolloutConversionFailed,
    );
    assert!(
        outcome
            .message
            .as_deref()
            .is_some_and(|message| message.contains(missing_id))
    );
    assert_failed_migration_preserved_source(&store, home.path(), thread_id, &path, &source).await;
}

#[tokio::test]
async fn migration_discards_only_a_trailing_partial_record() {
    let home = TempDir::new().expect("create Codex home");
    let thread_id = ThreadId::new();
    let path = write_rollout(
        home.path(),
        thread_id,
        SessionSource::Cli,
        vec![user_message("kept question")],
    );
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open legacy rollout");
    file.write_all(br#"{"timestamp":"unterminated""#)
        .expect("append partial tail");
    drop(file);
    let store = indexed_store(home.path()).await;

    let report = store
        .migrate_rollouts(apply_options())
        .await
        .expect("migrate rollout with partial tail");

    assert_eq!(report.outcomes[0].status, RolloutMigrationStatus::Migrated);
    assert!(!migration_journal_path(home.path(), thread_id).exists());
    let turns = list_active_summary_turns(&store, thread_id).await;
    assert_eq!(turns.turns.len(), 1);
    assert_eq!(turns.turns[0].items.len(), 1);
}
