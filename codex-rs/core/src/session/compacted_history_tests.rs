use std::collections::HashMap;
use std::collections::HashSet;

use codex_history::CodexHarnessMetadata;
use codex_history::CompactedHistoryEntry;
use codex_history::CompactedItem;
use codex_history::ResponseItemEnvelope;
use codex_history::RolloutItem;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ThreadHistoryMode;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::compacted_history::encode_replacement_history;
use super::compacted_history::normalize_copied_fork_rollout;
use super::compacted_history::retained_checkpoint_reference_item_ids;

fn message(id: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Message {
        id: Some(ResponseItemId::with_suffix("msg", id)),
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

#[test]
fn exact_persisted_items_are_referenced_and_new_items_stay_inline() {
    let persisted = message("persisted", "image-bearing user message");
    let mut inline = message("new", "new compaction output");
    inline.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let persisted_items = HashMap::from([(
        persisted
            .item
            .id()
            .expect("persisted item id")
            .as_str()
            .to_string(),
        persisted.clone(),
    )]);

    let encoded = encode_replacement_history(
        &[persisted.clone(), inline.clone()],
        &persisted_items,
        ThreadHistoryMode::PaginatedRefsV1,
    );

    assert_eq!(encoded.legacy, None);
    assert_eq!(
        encoded.entries,
        Some(vec![
            CompactedHistoryEntry::Reference {
                item_id: persisted.item.id().expect("item id").as_str().to_string(),
            },
            CompactedHistoryEntry::from(inline),
        ])
    );
}

#[test]
fn v2_mode_emits_complete_envelope_digest_without_changing_v1_shape() {
    let mut persisted = message("persisted-v2", "digest-bound source");
    persisted.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let item_id = persisted.item.id().expect("item id").as_str().to_string();
    let persisted_items = HashMap::from([(item_id.clone(), persisted.clone())]);

    let encoded_v2 = encode_replacement_history(
        std::slice::from_ref(&persisted),
        &persisted_items,
        ThreadHistoryMode::PaginatedRefsV2,
    );
    let Some(
        [
            CompactedHistoryEntry::ReferenceV2 {
                item_id: encoded_id,
                source_digest,
            },
        ],
    ) = encoded_v2.entries.as_deref()
    else {
        panic!("V2 mode must emit exactly one integrity reference");
    };
    assert_eq!(encoded_id, &item_id);
    assert_eq!(
        source_digest,
        &codex_history::CompactedHistoryDigest::from_envelope(&persisted)
            .expect("persisted envelope should encode")
    );

    let encoded_v1 = encode_replacement_history(
        std::slice::from_ref(&persisted),
        &persisted_items,
        ThreadHistoryMode::PaginatedRefsV1,
    );
    assert_eq!(
        encoded_v1.entries,
        Some(vec![CompactedHistoryEntry::Reference { item_id }])
    );
}

#[test]
fn changed_or_not_yet_persisted_items_are_not_referenced() {
    let persisted = message("same-id", "before");
    let changed = message("same-id", "after");
    let not_persisted = message("new", "new");
    let persisted_items = HashMap::from([(
        persisted
            .item
            .id()
            .expect("persisted item id")
            .as_str()
            .to_string(),
        persisted,
    )]);

    let encoded = encode_replacement_history(
        &[changed.clone(), not_persisted.clone()],
        &persisted_items,
        ThreadHistoryMode::PaginatedRefsV1,
    );

    assert_eq!(encoded.legacy, Some(vec![changed, not_persisted]));
    assert_eq!(encoded.entries, None);
}

#[test]
fn duplicate_stable_ids_remain_inline_to_preserve_occurrence_values() {
    let first = message("reused", "first value");
    let second = message("reused", "second value");
    let item_id = second
        .item
        .id()
        .expect("stable item id")
        .as_str()
        .to_string();
    let persisted_items = HashMap::from([(item_id, second.clone())]);

    let encoded = encode_replacement_history(
        &[first.clone(), second.clone()],
        &persisted_items,
        ThreadHistoryMode::PaginatedRefsV1,
    );

    assert_eq!(encoded.legacy, Some(vec![first, second]));
    assert_eq!(encoded.entries, None);
}

#[test]
fn ungated_history_modes_keep_compacted_history_inline() {
    let persisted = message("persisted", "image-bearing user message");
    let persisted_items = HashMap::from([(
        persisted.item.id().expect("item id").to_string(),
        persisted.clone(),
    )]);

    for history_mode in [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated] {
        let encoded = encode_replacement_history(
            std::slice::from_ref(&persisted),
            &persisted_items,
            history_mode,
        );
        assert_eq!(encoded.legacy, Some(vec![persisted.clone()]));
        assert_eq!(encoded.entries, None);
    }
}

#[test]
fn copied_fork_source_demand_excludes_superseded_checkpoint_references() {
    let compacted = |entries| {
        RolloutItem::Compacted(CompactedItem {
            message: String::new(),
            replacement_history: None,
            replacement_history_entries: Some(entries),
            mcp_resource_origins: None,
            window_number: None,
            first_window_id: None,
            previous_window_id: None,
            window_id: None,
        })
    };
    let items = vec![
        compacted(vec![CompactedHistoryEntry::Reference {
            item_id: "superseded".to_string(),
        }]),
        compacted(vec![
            CompactedHistoryEntry::Reference {
                item_id: "selected".to_string(),
            },
            CompactedHistoryEntry::from(message("inline", "not a source demand")),
        ]),
    ];

    assert_eq!(
        retained_checkpoint_reference_item_ids(&items, 0, Some(1)),
        HashSet::from(["selected".to_string()])
    );
}

#[test]
fn copied_fork_inlines_writer_filtered_source_at_durable_checkpoint() {
    let filtered_id = ResponseItemId::with_suffix("at", "copied-filtered");
    let filtered = ResponseItemEnvelope::new(ResponseItem::AdditionalTools {
        id: Some(filtered_id.clone()),
        role: "developer".to_string(),
        tools: vec![json!({"type": "function", "name": "temporary"})],
    });
    for history_mode in [ThreadHistoryMode::Legacy, ThreadHistoryMode::Paginated] {
        let normalized = normalize_copied_fork_rollout(
            vec![
                RolloutItem::ResponseItem(filtered.clone()),
                RolloutItem::Compacted(CompactedItem {
                    message: "checkpoint".to_string(),
                    replacement_history: None,
                    replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
                        item_id: filtered_id.to_string(),
                    }]),
                    mcp_resource_origins: None,
                    window_number: Some(1),
                    first_window_id: None,
                    previous_window_id: None,
                    window_id: None,
                }),
            ],
            history_mode,
        )
        .expect("filtered source should be made self-contained at the checkpoint");

        let RolloutItem::Compacted(checkpoint) = &normalized[1] else {
            panic!("expected checkpoint");
        };
        assert_eq!(checkpoint.replacement_history, Some(vec![filtered.clone()]));
        assert_eq!(checkpoint.replacement_history_entries, None);
    }
}

#[test]
fn copied_fork_reencodes_exact_sources_only_in_gated_mode() {
    let source = message("source", "persisted source");
    let source_id = source.item.id().expect("source id").to_string();
    let normalized = normalize_copied_fork_rollout(
        vec![
            RolloutItem::ResponseItem(source),
            RolloutItem::Compacted(CompactedItem {
                message: "checkpoint".to_string(),
                replacement_history: None,
                replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
                    item_id: source_id.clone(),
                }]),
                mcp_resource_origins: None,
                window_number: Some(1),
                first_window_id: None,
                previous_window_id: None,
                window_id: None,
            }),
        ],
        ThreadHistoryMode::PaginatedRefsV1,
    )
    .expect("gated copied fork should retain an exact backward reference");

    let RolloutItem::Compacted(checkpoint) = &normalized[1] else {
        panic!("expected checkpoint");
    };
    assert_eq!(checkpoint.replacement_history, None);
    assert_eq!(
        checkpoint.replacement_history_entries,
        Some(vec![CompactedHistoryEntry::Reference {
            item_id: source_id
        }])
    );
}

#[test]
fn copied_fork_ignores_unrelated_dangling_reference_in_superseded_checkpoint() {
    let source = message("new-source", "new source");
    let source_id = source.item.id().expect("source id").to_string();
    let compacted = |message: &str, entries| {
        RolloutItem::Compacted(CompactedItem {
            message: message.to_string(),
            replacement_history: None,
            replacement_history_entries: Some(entries),
            mcp_resource_origins: None,
            window_number: Some(1),
            first_window_id: None,
            previous_window_id: None,
            window_id: None,
        })
    };
    let normalized = normalize_copied_fork_rollout(
        vec![
            compacted(
                "superseded",
                vec![CompactedHistoryEntry::Reference {
                    item_id: "unrelated-missing".to_string(),
                }],
            ),
            RolloutItem::ResponseItem(source),
            compacted(
                "selected",
                vec![CompactedHistoryEntry::Reference { item_id: source_id }],
            ),
        ],
        ThreadHistoryMode::Legacy,
    )
    .expect("only the selected checkpoint should require strict resolution");

    assert_eq!(
        normalized
            .iter()
            .filter(|item| matches!(item, RolloutItem::Compacted(_)))
            .count(),
        1
    );
}
