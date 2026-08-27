use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_rollout::CompactedHistoryEntry;
use codex_rollout::RolloutItem;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::parse_legacy_rollout_line;

fn line(payload_type: &str, payload: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "timestamp": "2025-01-03T12:00:00Z",
        "type": payload_type,
        "payload": payload,
    }))
    .expect("serialize fixture")
}

#[test]
fn parses_legacy_numeric_event_payloads_through_value() {
    let bytes = line(
        "event_msg",
        json!({
            "type": "token_count",
            "info": null,
            "rate_limits": {
                "primary": {
                    "used_percent": 2,
                    "window_minutes": 300,
                    "resets_at": 1_770_414_841,
                },
                "secondary": {
                    "used_percent": 12,
                    "window_minutes": 10_080,
                    "resets_at": 1_770_698_702,
                },
                "credits": {
                    "has_credits": false,
                    "unlimited": false,
                    "balance": null,
                },
                "plan_type": null,
            },
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse legacy token count")
        .expect("keep legacy token count");
    assert!(matches!(
        parsed.item,
        RolloutItem::EventMsg(EventMsg::TokenCount(_))
    ));
}

#[test]
fn normalizes_legacy_rate_limit_reset_timestamps() {
    let bytes = line(
        "event_msg",
        json!({
            "type": "token_count",
            "info": null,
            "rate_limits": {
                "primary": {
                    "used_percent": 2,
                    "window_minutes": 300,
                    "resets_at": "2025-10-19T08:51:37.876641+00:00",
                },
                "secondary": null,
                "credits": null,
                "plan_type": null,
            },
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse legacy reset timestamp")
        .expect("keep legacy token count");
    assert!(matches!(
        parsed.item,
        RolloutItem::EventMsg(EventMsg::TokenCount(_))
    ));
}

#[test]
fn normalizes_legacy_turn_context_collaboration_mode() {
    let cwd = std::env::temp_dir().to_string_lossy().into_owned();
    let bytes = line(
        "turn_context",
        json!({
            "cwd": cwd,
            "approval_policy": "never",
            "sandbox_policy": {"type": "danger-full-access"},
            "model": "gpt-test",
            "personality": null,
            "collaboration_mode": {
                "mode": "plan",
                "model": "gpt-test",
                "reasoning_effort": null,
                "developer_instructions": null,
            },
            "effort": null,
            "summary": "auto",
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse legacy turn context")
        .expect("keep legacy turn context");
    let RolloutItem::TurnContext(context) = parsed.item else {
        panic!("expected turn context");
    };
    assert_eq!(
        context
            .collaboration_mode
            .expect("collaboration mode")
            .model(),
        "gpt-test"
    );
}

#[test]
fn normalizes_legacy_turn_context_sandbox_policy() {
    let cwd = std::env::temp_dir().to_string_lossy().into_owned();
    let bytes = line(
        "turn_context",
        json!({
            "cwd": cwd,
            "approval_policy": "never",
            "sandbox_policy": {"mode": "danger-full-access"},
            "model": "gpt-test",
            "personality": null,
            "effort": null,
            "summary": "auto",
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse legacy sandbox policy")
        .expect("keep legacy turn context");
    assert!(matches!(parsed.item, RolloutItem::TurnContext(_)));
}

#[test]
fn normalizes_legacy_review_entry_prompt() {
    let bytes = line(
        "event_msg",
        json!({
            "type": "entered_review_mode",
            "prompt": "review these changes",
            "user_facing_hint": "Review requested.",
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse legacy review entry")
        .expect("keep legacy review entry");
    assert!(matches!(
        parsed.item,
        RolloutItem::EventMsg(EventMsg::EnteredReviewMode(_))
    ));
}

#[test]
fn normalizes_legacy_plain_command_cwd() {
    let cwd = std::env::temp_dir().to_string_lossy().into_owned();
    let bytes = line(
        "event_msg",
        json!({
            "type": "exec_command_end",
            "call_id": "call-1",
            "turn_id": "turn-1",
            "command": ["echo", "ok"],
            "cwd": cwd,
            "parsed_cmd": [],
            "source": "agent",
            "stdout": "",
            "stderr": "",
            "aggregated_output": "",
            "exit_code": 0,
            "duration": {"secs": 0, "nanos": 0},
            "formatted_output": "",
            "status": "completed",
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse legacy command")
        .expect("keep legacy command");
    assert!(matches!(
        parsed.item,
        RolloutItem::EventMsg(EventMsg::ExecCommandEnd(_))
    ));
}

#[test]
fn skips_only_known_retired_events() {
    for event_type in [
        "guardian_assessment",
        "thread_name_updated",
        "undo_completed",
    ] {
        let bytes = line("event_msg", json!({"type": event_type}));
        assert!(
            parse_legacy_rollout_line(&bytes)
                .expect("inspect retired event")
                .is_none()
        );
    }

    let unknown = line("event_msg", json!({"type": "unknown_legacy_event"}));
    assert!(parse_legacy_rollout_line(&unknown).is_err());
}

#[test]
fn skips_legacy_ghost_snapshots() {
    let ghost_snapshot = line(
        "response_item",
        json!({
            "type": "ghost_snapshot",
            "ghost_commit": {"id": "legacy"},
        }),
    );

    assert!(
        parse_legacy_rollout_line(&ghost_snapshot)
            .expect("inspect ghost snapshot")
            .is_none()
    );
}

#[test]
fn strips_nested_ghost_snapshot_and_keeps_legacy_metadata_aligned() {
    let bytes = line(
        "compacted",
        json!({
            "message": "summary",
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
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse compacted history")
        .expect("keep compacted history");
    let RolloutItem::Compacted(compacted) = parsed.item else {
        panic!("expected compacted rollout item");
    };
    let history = compacted
        .replacement_history
        .expect("legacy replacement history");
    assert_eq!(history.len(), 2);
    assert_eq!(
        history
            .iter()
            .map(|envelope| envelope
                .metadata
                .as_ref()
                .map(|metadata| metadata.client_authored))
            .collect::<Vec<_>>(),
        vec![Some(true), Some(false)]
    );
    assert!(
        history
            .iter()
            .all(|envelope| !matches!(&envelope.item, ResponseItem::Other))
    );
    assert!(matches!(
        &history[0].item,
        ResponseItem::Message { content, .. }
            if content == &vec![ContentItem::InputText {
                text: "kept developer".to_string(),
            }]
    ));
}

#[test]
fn strips_nested_ghost_snapshot_from_entry_backed_history() {
    let bytes = line(
        "compacted",
        json!({
            "message": "summary",
            "replacement_history_entries": [
                {"type": "reference", "item_id": "older-source"},
                {
                    "type": "inline",
                    "item": {"type": "ghost_snapshot", "ghost_commit": {"id": "deadbeef"}},
                    "metadata": {"client_authored": true}
                },
                {
                    "type": "inline",
                    "item": {
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "kept"}]
                    }
                }
            ]
        }),
    );

    let parsed = parse_legacy_rollout_line(&bytes)
        .expect("parse entry-backed history")
        .expect("keep entry-backed history");
    let RolloutItem::Compacted(compacted) = parsed.item else {
        panic!("expected compacted rollout item");
    };
    let entries = compacted
        .replacement_history_entries
        .expect("entry-backed replacement history");
    assert_eq!(entries.len(), 2);
    assert!(matches!(
        &entries[0],
        CompactedHistoryEntry::Reference { item_id } if item_id == "older-source"
    ));
    assert!(matches!(
        &entries[1],
        CompactedHistoryEntry::Inline { item, .. }
            if matches!(item.as_ref(), ResponseItem::Message { .. })
    ));
}

#[test]
fn nested_ghost_snapshot_does_not_mask_misaligned_legacy_metadata() {
    let bytes = line(
        "compacted",
        json!({
            "message": "summary",
            "replacement_history": [
                {"type": "message", "role": "assistant", "content": []},
                {"type": "ghost_snapshot", "ghost_commit": {"id": "deadbeef"}}
            ],
            "replacement_history_metadata": [{"client_authored": true}]
        }),
    );

    assert!(parse_legacy_rollout_line(&bytes).is_err());
}
