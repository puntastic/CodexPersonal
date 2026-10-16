use std::borrow::Cow;

use codex_protocol::ThreadId;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::ThreadHistoryMode;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::into_persisted_rollout_items;
use super::is_persisted_rollout_item;
use super::persisted_rollout_item;
use super::persisted_rollout_items;
use crate::RolloutItem;

#[test]
fn all_paginated_modes_apply_owned_command_and_mcp_projections() {
    for output in [
        "small".to_string(),
        format!("head\n{}\ntail", "x".repeat(200_000)),
    ] {
        let items = [
            json!({
                "type": "CommandExecution", "id": "command", "command": ["echo"],
                "cwd": "file:///tmp", "parsed_cmd": [], "source": "agent",
                "status": "completed", "aggregated_output": output,
            }),
            json!({
                "type": "McpToolCall", "id": "mcp", "server": "test", "tool": "test",
                "arguments": {}, "status": "completed",
                "result": {"content": [{"type": "text", "text": output}]},
            }),
        ];
        for item in items {
            let item = RolloutItem::EventMsg(EventMsg::ItemCompleted(ItemCompletedEvent {
                thread_id: ThreadId::default(),
                turn_id: "turn".to_string(),
                item: serde_json::from_value::<TurnItem>(item).expect("completed item fixture"),
                started_at_ms: None,
                completed_at_ms: 0,
            }));
            let original = serde_json::to_value(&item).expect("serialize original item");
            let expected =
                persisted_rollout_items(std::slice::from_ref(&item), ThreadHistoryMode::Paginated);
            assert!(persisted_rollout_item(&item, ThreadHistoryMode::Legacy).is_none());
            for mode in [
                ThreadHistoryMode::Paginated,
                ThreadHistoryMode::PaginatedRefsV1,
                ThreadHistoryMode::PaginatedRefsV2,
            ] {
                let projected = persisted_rollout_item(&item, mode).expect("durable projection");
                assert!(is_persisted_rollout_item(&item, mode));
                if output.len() > 64 * 1024 {
                    assert!(matches!(&projected, Cow::Owned(_)));
                    assert!(
                        serde_json::to_vec(projected.as_ref())
                            .expect("projected bytes")
                            .len()
                            < serde_json::to_vec(&item).expect("original bytes").len()
                    );
                } else {
                    assert!(matches!(&projected, Cow::Borrowed(_)));
                    assert_eq!(
                        serde_json::to_value(projected.as_ref()).expect("serialize projection"),
                        original
                    );
                }
                assert_eq!(
                    serde_json::to_value([projected.into_owned()]).expect("serialize projection"),
                    serde_json::to_value(&expected).expect("serialize expected projection")
                );
                assert_eq!(
                    serde_json::to_value(into_persisted_rollout_items(vec![item.clone()], mode))
                        .expect("serialize owned batch"),
                    serde_json::to_value(&expected).expect("serialize expected projection")
                );
                assert_eq!(
                    serde_json::to_value(&item).expect("unchanged input"),
                    original
                );
            }
        }
    }
}
