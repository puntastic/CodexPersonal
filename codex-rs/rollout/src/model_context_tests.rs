use codex_history::CompactedHistoryEntry;
use codex_protocol::ResponseItemId;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionMeta;
use codex_protocol::protocol::TurnContextItem;
use codex_protocol::protocol::TurnStartedEvent;
use codex_utils_absolute_path::AbsolutePathBuf;

use super::*;

#[test]
fn entry_reference_resolves_from_older_top_level_response_item() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![reference("msg-source")])),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Continue);
    assert_eq!(
        scan.push(top_level_response("msg-source")),
        ModelContextScanProgress::Complete
    );
}

#[test]
fn entry_reference_resolves_from_older_legacy_checkpoint() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![reference("msg-source")])),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Continue);
    assert_eq!(
        scan.push(legacy_checkpoint(vec![response("msg-source")])),
        ModelContextScanProgress::Complete
    );
}

#[test]
fn older_entry_checkpoint_does_not_recruit_unrelated_dependencies() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![reference("msg-target")])),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Continue);
    assert_eq!(
        scan.push(entry_checkpoint(vec![
            inline("msg-target"),
            reference("msg-dependency"),
        ])),
        ModelContextScanProgress::Complete
    );
}

#[test]
fn intermediate_reference_checkpoints_keep_only_the_selected_base_dependency_live() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![reference("msg-target")])),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Continue);
    for index in 0..128 {
        assert_eq!(
            scan.push(entry_checkpoint(vec![
                reference("msg-target"),
                reference(&format!("msg-unrelated-{index}")),
            ])),
            ModelContextScanProgress::Continue
        );
    }
    assert_eq!(
        scan.push(top_level_response("msg-target")),
        ModelContextScanProgress::Complete
    );
}

#[test]
fn bounded_finish_retains_only_exact_reconstruction_inputs() {
    let mut scan = ModelContextScan::default();
    let selected = entry_checkpoint(vec![
        reference("msg-top-level"),
        reference("msg-checkpoint"),
        inline("msg-inline"),
    ]);
    let suffix = top_level_response("msg-suffix");

    assert_eq!(
        scan.push(suffix.clone()),
        ModelContextScanProgress::Continue
    );
    assert_eq!(
        scan.push(selected.clone()),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Continue);

    let retained_after_context = retained_item_count(&scan);
    for index in 0..128 {
        assert_eq!(
            scan.push(top_level_large_response(
                &format!("msg-unrelated-{index}"),
                32 * 1024,
            )),
            ModelContextScanProgress::Continue
        );
        assert_eq!(
            retained_item_count(&scan),
            retained_after_context,
            "unrelated pre-base item {index} was retained"
        );
    }

    let mut checkpoint_sources = vec![response_with_text("msg-checkpoint", "stale source")];
    checkpoint_sources.extend(
        (0..64)
            .map(|index| large_response(&format!("msg-checkpoint-unrelated-{index}"), 32 * 1024)),
    );
    checkpoint_sources.push(response("msg-checkpoint"));
    assert_eq!(
        scan.push(legacy_checkpoint(checkpoint_sources)),
        ModelContextScanProgress::Continue
    );
    assert_eq!(
        scan.push(top_level_response("msg-top-level")),
        ModelContextScanProgress::Complete
    );

    let canonical_meta = session_meta();
    let expected = vec![
        RolloutItem::SessionMeta(canonical_meta.clone()),
        top_level_response("msg-top-level"),
        legacy_checkpoint(vec![response("msg-checkpoint")]),
        turn_started(),
        agent_message(),
        turn_context(),
        selected,
        suffix,
    ];
    let actual = scan.finish(canonical_meta);
    let selected_index = actual
        .iter()
        .position(|item| {
            matches!(
                item,
                RolloutItem::Compacted(compacted) if compacted.message == "checkpoint"
            )
        })
        .expect("selected checkpoint");
    let resolved = codex_history::resolve_checkpoint_at(&actual, selected_index)
        .expect("resolve selected checkpoint")
        .expect("selected checkpoint history");
    let expected_history: Vec<ResponseItemEnvelope> = vec![
        response("msg-top-level").into(),
        response("msg-checkpoint").into(),
        response("msg-inline").into(),
    ];
    assert_eq!(resolved, expected_history);
    assert_eq!(
        serde_json::to_value(actual).expect("serialize bounded scan"),
        serde_json::to_value(expected).expect("serialize expected reconstruction inputs")
    );
}

#[test]
fn unresolved_entry_reference_never_reports_a_bounded_cutoff() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![reference("msg-missing")])),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Continue);
    assert_eq!(
        scan.push(top_level_response("msg-unrelated")),
        ModelContextScanProgress::Continue
    );
}

#[test]
fn same_checkpoint_inline_item_cannot_satisfy_its_own_backward_reference() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![
            reference("msg-same-checkpoint"),
            inline("msg-same-checkpoint"),
        ])),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Continue);
}

#[test]
fn inline_only_entry_checkpoint_is_a_complete_history_base() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![inline("msg-inline")])),
        ModelContextScanProgress::Continue
    );
    assert_completed_turn_context(&mut scan, ModelContextScanProgress::Complete);
}

#[test]
fn entry_checkpoint_still_requires_a_completed_turn_context() {
    let mut scan = ModelContextScan::default();

    assert_eq!(
        scan.push(entry_checkpoint(vec![inline("msg-inline")])),
        ModelContextScanProgress::Continue
    );
    assert_eq!(
        scan.push(agent_message()),
        ModelContextScanProgress::Continue
    );
    assert_eq!(
        scan.push(turn_started()),
        ModelContextScanProgress::Continue
    );
}

fn assert_completed_turn_context(scan: &mut ModelContextScan, expected: ModelContextScanProgress) {
    assert_eq!(
        scan.push(turn_context()),
        ModelContextScanProgress::Continue
    );
    assert_eq!(
        scan.push(agent_message()),
        ModelContextScanProgress::Continue
    );
    assert_eq!(scan.push(turn_started()), expected);
}

fn entry_checkpoint(entries: Vec<CompactedHistoryEntry>) -> RolloutItem {
    RolloutItem::Compacted(codex_history::CompactedItem {
        message: "checkpoint".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(entries),
        mcp_resource_origins: None,
        window_number: Some(1),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    })
}

fn legacy_checkpoint(items: Vec<ResponseItem>) -> RolloutItem {
    RolloutItem::Compacted(codex_history::CompactedItem {
        message: "legacy checkpoint".to_string(),
        replacement_history: Some(items.into_iter().map(Into::into).collect()),
        replacement_history_entries: None,
        mcp_resource_origins: None,
        window_number: Some(1),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    })
}

fn reference(item_id: &str) -> CompactedHistoryEntry {
    CompactedHistoryEntry::Reference {
        item_id: item_id.to_string(),
    }
}

fn inline(item_id: &str) -> CompactedHistoryEntry {
    CompactedHistoryEntry::Inline {
        item: Box::new(response(item_id)),
        metadata: None,
    }
}

fn top_level_response(item_id: &str) -> RolloutItem {
    RolloutItem::ResponseItem(response(item_id).into())
}

fn response(item_id: &str) -> ResponseItem {
    response_with_text(item_id, "source")
}

fn large_response(item_id: &str, text_len: usize) -> ResponseItem {
    response_with_text(item_id, &"x".repeat(text_len))
}

fn top_level_large_response(item_id: &str, text_len: usize) -> RolloutItem {
    RolloutItem::ResponseItem(large_response(item_id, text_len).into())
}

fn response_with_text(item_id: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::from_server(item_id.to_string())),
        role: "developer".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn session_meta() -> SessionMetaLine {
    SessionMetaLine {
        meta: SessionMeta::default(),
        git: None,
    }
}

fn retained_item_count(scan: &ModelContextScan) -> usize {
    scan.items_newest_first.len() + scan.active_segment.retained_items_newest_first.len()
}

fn turn_context() -> RolloutItem {
    let cwd = std::env::current_dir().expect("current directory");
    RolloutItem::TurnContext(TurnContextItem {
        turn_id: Some("turn-1".to_string()),
        cwd: AbsolutePathBuf::from_absolute_path(cwd).expect("absolute current directory"),
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
    })
}

fn agent_message() -> RolloutItem {
    RolloutItem::ResponseItem(
        ResponseItem::AgentMessage {
            id: None,
            author: "worker".to_string(),
            recipient: "root".to_string(),
            content: vec![AgentMessageInputContent::InputText {
                text: "done".to_string(),
            }],
            internal_chat_message_metadata_passthrough: None,
        }
        .into(),
    )
}

fn turn_started() -> RolloutItem {
    RolloutItem::EventMsg(EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: "turn-1".to_string(),
        trace_id: None,
        started_at: None,
        model_context_window: Some(128_000),
        collaboration_mode_kind: Default::default(),
    }))
}
