use anyhow::Result;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn response_item_envelope_accessors_preserve_item() {
    let expected_item = ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: "hello".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    let mut envelope = ResponseItemEnvelope::new(expected_item.clone());

    assert_eq!(&*envelope, &expected_item);
    let borrowed: &ResponseItem = envelope.borrow();
    assert_eq!(borrowed, &expected_item);
    let replacement_item = ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: "goodbye".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    *envelope = replacement_item.clone();

    assert_eq!(envelope.into_item(), replacement_item);
}

#[test]
/// Keeps legacy response-item rollout lines readable and byte-shape compatible.
fn response_item_rollout_line_preserves_shape() -> Result<()> {
    let legacy_line = json!({
        "timestamp": "2025-01-03T12:00:00.000Z",
        "ordinal": 7,
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "hello",
            }],
        },
    });

    let line = serde_json::from_value::<RolloutLine>(legacy_line.clone())?;
    let RolloutItem::ResponseItem(envelope) = &line.item else {
        panic!("expected response item");
    };
    assert!(matches!(&envelope.item, ResponseItem::Message { .. }));
    assert_eq!(envelope.metadata, None);
    assert_eq!(serde_json::to_value(line)?, legacy_line);
    Ok(())
}

#[test]
/// Keeps harness metadata beside, rather than inside, response-item payloads.
fn response_item_envelope_stores_metadata_beside_rollout_payload() -> Result<()> {
    let response_item = response_message("developer");
    let line = RolloutLine {
        timestamp: "2025-01-03T12:00:00.000Z".to_string(),
        ordinal: Some(7),
        item: RolloutItem::ResponseItem(ResponseItemEnvelope {
            item: response_item.clone(),
            metadata: Some(CodexHarnessMetadata {
                client_authored: true,
            }),
        }),
    };
    let serialized = serde_json::to_value(&line)?;

    assert_eq!(
        serialized,
        json!({
            "timestamp": "2025-01-03T12:00:00.000Z",
            "ordinal": 7,
            "type": "response_item",
            "payload": response_item,
            "metadata": { "client_authored": true },
        })
    );
    assert_eq!(serialized["payload"].get("metadata"), None);

    let restored = serde_json::from_value::<RolloutLine>(serialized)?;
    let RolloutItem::ResponseItem(envelope) = restored.item else {
        panic!("expected response item");
    };
    assert_eq!(
        envelope.metadata,
        Some(CodexHarnessMetadata {
            client_authored: true,
        })
    );
    Ok(())
}

#[test]
/// Keeps future metadata fields from making older binaries reject persisted items.
fn response_item_envelope_ignores_unknown_harness_metadata_fields() -> Result<()> {
    let line = serde_json::from_value::<RolloutLine>(json!({
        "timestamp": "2025-01-03T12:00:00.000Z",
        "ordinal": 7,
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "hello",
            }],
        },
        "metadata": {
            "future_field": "value",
        },
    }))?;

    let RolloutItem::ResponseItem(envelope) = line.item else {
        panic!("expected response item");
    };
    assert_eq!(envelope.metadata, Some(CodexHarnessMetadata::default()));

    let compacted = serde_json::from_value::<CompactedItem>(json!({
        "message": "summary",
        "replacement_history": [response_message("user")],
        "replacement_history_metadata": [{ "future_field": "value" }],
    }))?;
    assert_eq!(
        compacted.replacement_history.expect("replacement history")[0].metadata,
        Some(CodexHarnessMetadata::default())
    );
    Ok(())
}

#[test]
/// Keeps legacy compacted replacement histories readable and shape-compatible.
fn response_item_replacement_history_preserves_shape() -> Result<()> {
    let legacy_item = json!({
        "message": "summary",
        "replacement_history": [{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "hello",
            }],
        }],
    });

    let item = serde_json::from_value::<CompactedItem>(legacy_item.clone())?;
    let replacement_history = item
        .replacement_history
        .as_ref()
        .expect("replacement history");
    assert!(matches!(
        &replacement_history[0].item,
        ResponseItem::Message { .. }
    ));
    assert_eq!(replacement_history[0].metadata, None);
    assert_eq!(serde_json::to_value(item)?, legacy_item);
    Ok(())
}

#[test]
/// Stores complete aligned checkpoint metadata without modifying response items.
fn compacted_replacement_history_stores_metadata_in_an_aligned_sidecar() -> Result<()> {
    let developer_message = response_message("developer");
    let compaction_item = ResponseItem::Compaction {
        id: None,
        encrypted_content: "opaque".to_string(),
        internal_chat_message_metadata_passthrough: None,
    };
    let item = CompactedItem {
        message: "summary".to_string(),
        replacement_history: Some(vec![
            ResponseItemEnvelope {
                item: developer_message.clone(),
                metadata: Some(CodexHarnessMetadata {
                    client_authored: true,
                }),
            },
            ResponseItemEnvelope::new(compaction_item.clone()),
        ]),
        replacement_history_entries: None,
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    };

    let serialized = serde_json::to_value(item)?;
    assert_eq!(
        serialized,
        json!({
            "message": "summary",
            "replacement_history": [developer_message, compaction_item],
            "replacement_history_metadata": [
                { "client_authored": true },
                { "client_authored": false },
            ],
        })
    );

    let restored = serde_json::from_value::<CompactedItem>(serialized)?;
    assert_eq!(
        restored.replacement_history,
        Some(vec![
            ResponseItemEnvelope {
                item: developer_message,
                metadata: Some(CodexHarnessMetadata {
                    client_authored: true,
                }),
            },
            ResponseItemEnvelope {
                item: compaction_item,
                metadata: Some(CodexHarnessMetadata::default()),
            },
        ])
    );
    Ok(())
}

#[test]
/// Round-trips reference-backed histories without losing inline harness metadata.
fn compacted_replacement_history_entries_round_trip_mixed_entries() -> Result<()> {
    let response_item = response_message("developer");
    let item = CompactedItem {
        message: "summary".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![
            CompactedHistoryEntry::Inline {
                item: Box::new(response_item.clone()),
                metadata: Some(CodexHarnessMetadata {
                    client_authored: true,
                }),
            },
            CompactedHistoryEntry::Reference {
                item_id: "item-123".to_string(),
            },
        ]),
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    };

    let serialized = serde_json::to_value(&item)?;
    assert_eq!(
        serialized,
        json!({
            "message": "summary",
            "replacement_history_entries": [
                {
                    "type": "inline",
                    "item": response_item,
                    "metadata": { "client_authored": true },
                },
                {
                    "type": "reference",
                    "item_id": "item-123",
                },
            ],
        })
    );
    assert_eq!(serde_json::from_value::<CompactedItem>(serialized)?, item);
    Ok(())
}

#[test]
fn compacted_history_entry_layout_stays_bounded() {
    assert!(std::mem::size_of::<CompactedHistoryEntry>() <= 64);
}

#[test]
/// Rejects ambiguous compacted items containing both replacement-history formats.
fn compacted_replacement_history_rejects_legacy_and_entries_together() {
    let error = serde_json::from_value::<CompactedItem>(json!({
        "message": "summary",
        "replacement_history": [response_message("user")],
        "replacement_history_entries": [{
            "type": "reference",
            "item_id": "item-123",
        }],
    }))
    .expect_err("both replacement-history formats must be rejected");

    assert_eq!(
        error.to_string(),
        "replacement_history and replacement_history_entries are mutually exclusive"
    );
}

#[test]
/// Rejects checkpoint sidecars that cannot be paired unambiguously with history.
fn compacted_replacement_history_rejects_misaligned_metadata() {
    let malformed_items = [
        json!({
            "message": "summary",
            "replacement_history": [response_message("user")],
            "replacement_history_metadata": [],
        }),
        json!({
            "message": "summary",
            "replacement_history": [response_message("user")],
            "replacement_history_metadata": [{}, {}],
        }),
        json!({
            "message": "summary",
            "replacement_history_metadata": [{}],
        }),
    ];

    for malformed in malformed_items {
        let error = serde_json::from_value::<CompactedItem>(malformed)
            .expect_err("misaligned checkpoint metadata must be rejected");
        assert!(
            error.to_string().contains("replacement_history_metadata"),
            "error: {error}"
        );
    }
}

#[test]
/// Keeps annotated checkpoints readable by binaries expecting raw response items.
fn compacted_metadata_remains_compatible_with_legacy_response_item_readers() -> Result<()> {
    #[derive(Deserialize)]
    #[serde(tag = "type", content = "payload", rename_all = "snake_case")]
    enum LegacyRolloutItem {
        ResponseItem(Box<ResponseItem>),
        Compacted(LegacyCompactedItem),
    }

    #[derive(Deserialize)]
    struct LegacyCompactedItem {
        replacement_history: Vec<ResponseItem>,
    }

    let response_item = response_message("developer");
    let envelope = ResponseItemEnvelope {
        item: response_item.clone(),
        metadata: Some(CodexHarnessMetadata {
            client_authored: true,
        }),
    };
    let response_line = serde_json::to_value(RolloutItem::ResponseItem(envelope.clone()))?;
    let LegacyRolloutItem::ResponseItem(legacy_response) =
        serde_json::from_value::<LegacyRolloutItem>(response_line)?
    else {
        panic!("expected legacy response item");
    };
    assert_eq!(*legacy_response, response_item);

    let compacted_line = serde_json::to_value(RolloutItem::Compacted(CompactedItem {
        message: "summary".to_string(),
        replacement_history: Some(vec![envelope]),
        replacement_history_entries: None,
        mcp_resource_origins: Some(McpResourceOriginCheckpoint::default()),
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    }))?;

    let LegacyRolloutItem::Compacted(legacy) =
        serde_json::from_value::<LegacyRolloutItem>(compacted_line)?
    else {
        panic!("expected legacy compacted item");
    };
    assert_eq!(legacy.replacement_history, vec![response_item]);
    Ok(())
}

#[test]
/// Preserves the established tagged payload representation for every rollout variant.
fn rollout_item_variants_preserve_existing_payload_shapes() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let fixtures = [
        json!({
            "type": "session_meta",
            "payload": SessionMetaLine {
                meta: SessionMeta::default(),
                git: None,
            },
        }),
        json!({
            "type": "response_item",
            "payload": response_message("user"),
        }),
        json!({
            "type": "inter_agent_communication",
            "payload": {
                "author": "/root",
                "recipient": "/root/child",
                "other_recipients": [],
                "content": "hello",
                "trigger_turn": false,
            },
        }),
        json!({
            "type": "inter_agent_communication_metadata",
            "payload": { "trigger_turn": true },
        }),
        json!({
            "type": "compacted",
            "payload": { "message": "summary" },
        }),
        json!({
            "type": "turn_context",
            "payload": {
                "cwd": cwd,
                "approval_policy": "never",
                "sandbox_policy": { "type": "danger-full-access" },
                "model": "gpt-5",
                "summary": "auto",
            },
        }),
        json!({
            "type": "world_state",
            "payload": { "full": true, "state": { "cwd": "/tmp" } },
        }),
        json!({
            "type": "security_risk_score",
            "payload": {
                "scores": {
                    "action_risk": 0.92,
                    "data_exfiltration": 0.31,
                },
            },
        }),
        json!({
            "type": "event_msg",
            "payload": { "type": "warning", "message": "heads up" },
        }),
        json!({
            "type": "realtime_item",
            "payload": {
                "id": "segment-1",
                "realtime_session_id": "session-1",
                "type": "transcript_segment",
                "role": "assistant",
                "text": "hello",
            },
        }),
    ];

    for expected in fixtures {
        let item = serde_json::from_value::<RolloutItem>(expected.clone())?;
        assert_eq!(serde_json::to_value(item)?, expected);
    }
    Ok(())
}

#[test]
/// Keeps the generated schema aligned with each variant's actual persisted shape.
fn rollout_item_schema_matches_tagged_payload_and_sibling_metadata() -> Result<()> {
    let schema = serde_json::to_value(schemars::schema_for!(RolloutItem))?;
    let variants = schema["oneOf"].as_array().expect("rollout variants");
    assert_eq!(variants.len(), 10);

    for variant in variants {
        let required = variant["required"].as_array().expect("required fields");
        assert!(required.contains(&json!("type")), "schema: {variant}");
        assert!(required.contains(&json!("payload")), "schema: {variant}");
    }

    let response_item = variants
        .iter()
        .find(|variant| variant["properties"]["type"]["enum"] == json!(["response_item"]))
        .expect("response item schema");
    assert!(response_item["properties"].get("metadata").is_some());
    assert_eq!(
        response_item["properties"]["payload"]["$ref"],
        json!("#/definitions/ResponseItem")
    );

    let compacted = &schema["definitions"]["CompactedItem"];
    assert_eq!(
        compacted["properties"]["replacement_history"]["items"]["$ref"],
        json!("#/definitions/ResponseItem")
    );
    assert_eq!(
        compacted["properties"]["replacement_history_metadata"]["items"]["$ref"],
        json!("#/definitions/CodexHarnessMetadata")
    );
    assert_eq!(
        compacted["properties"]["replacement_history_entries"]["items"]["$ref"],
        json!("#/definitions/CompactedHistoryEntry")
    );
    let history_entry_variants = schema["definitions"]["CompactedHistoryEntry"]["oneOf"]
        .as_array()
        .expect("compacted history entry variants");
    assert_eq!(history_entry_variants.len(), 3);
    for entry_type in ["inline", "reference", "reference_v2"] {
        let variant = history_entry_variants
            .iter()
            .find(|variant| variant["properties"]["type"]["enum"] == json!([entry_type]))
            .unwrap_or_else(|| panic!("missing {entry_type} compacted history entry schema"));
        let required = variant["required"].as_array().expect("required fields");
        assert!(required.contains(&json!("type")), "schema: {variant}");
    }
    let required = compacted["required"].as_array().expect("required fields");
    assert!(!required.contains(&json!("replacement_history")));
    assert!(!required.contains(&json!("replacement_history_metadata")));
    assert!(!required.contains(&json!("replacement_history_entries")));
    Ok(())
}

fn response_message(role: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: role.to_string(),
        content: vec![ContentItem::InputText {
            text: "hello".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn identified_message(id: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Message {
        id: Some(codex_protocol::ResponseItemId::with_suffix("msg", id)),
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

fn legacy_checkpoint(message: &str, history: Vec<ResponseItemEnvelope>) -> RolloutItem {
    RolloutItem::Compacted(CompactedItem {
        message: message.to_string(),
        replacement_history: Some(history),
        replacement_history_entries: None,
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    })
}

fn entry_checkpoint(message: &str, entries: Vec<CompactedHistoryEntry>) -> RolloutItem {
    RolloutItem::Compacted(CompactedItem {
        message: message.to_string(),
        replacement_history: None,
        replacement_history_entries: Some(entries),
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    })
}

fn integrity_reference(envelope: &ResponseItemEnvelope) -> CompactedHistoryEntry {
    let item_id = envelope
        .item
        .id()
        .expect("integrity reference item id")
        .as_str()
        .to_string();
    CompactedHistoryEntry::reference_v2(item_id, envelope)
        .expect("test envelope should have a canonical digest")
}

#[test]
fn v2_reference_round_trips_with_versioned_digest() -> Result<()> {
    let mut source = identified_message("v2-wire", "complete envelope");
    source.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let reference = integrity_reference(&source);
    let value = serde_json::to_value(&reference)?;

    assert_eq!(value["type"], json!("reference_v2"));
    assert_eq!(
        value["item_id"],
        json!(source.item.id().expect("source id").as_str())
    );
    assert!(
        value["source_digest"]
            .as_str()
            .expect("digest string")
            .starts_with("sha256-response-item-envelope-v1:")
    );
    assert_eq!(
        serde_json::from_value::<CompactedHistoryEntry>(value)?,
        reference
    );
    Ok(())
}

#[test]
fn v2_reference_rejects_malformed_digest_before_resolution() {
    let error = serde_json::from_value::<CompactedHistoryEntry>(json!({
        "type": "reference_v2",
        "item_id": "msg-malformed",
        "source_digest": "sha256-response-item-envelope-v1:ABC",
    }))
    .expect_err("malformed digest must fail closed");

    assert!(
        error
            .to_string()
            .contains("exactly 64 lowercase hexadecimal characters"),
        "unexpected error: {error}"
    );
}

#[test]
fn v2_reference_resolves_exact_complete_envelope() {
    let mut source = identified_message("v2-exact", "exact content");
    source.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let mut resolver = CompactedHistoryResolver::default();
    resolver.index_explicit_sources(&RolloutItem::ResponseItem(source.clone()));
    let RolloutItem::Compacted(checkpoint) =
        entry_checkpoint("v2 exact", vec![integrity_reference(&source)])
    else {
        panic!("expected compacted checkpoint");
    };

    assert_eq!(
        resolver
            .resolve_compacted_item_detailed(&checkpoint)
            .expect("exact V2 source should resolve"),
        Some(vec![source])
    );
}

#[test]
fn v2_reference_survives_cold_reload_of_host_only_passthrough_fields() -> Result<()> {
    let mut source = identified_message("v2-cold-reload", "durable envelope");
    source.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let ResponseItem::Message {
        internal_chat_message_metadata_passthrough,
        ..
    } = &mut source.item
    else {
        panic!("expected message source");
    };
    *internal_chat_message_metadata_passthrough = Some(
        codex_protocol::models::InternalChatMessageMetadataPassthrough {
            cell_id: Some("host-cell".to_string()),
            executed_tool_calls: Some(vec![
                codex_protocol::models::ExecutedToolCall::truncated(
                    "oversized-tool".to_string(),
                    4096,
                    1024,
                ),
            ]),
            tool_calls_complete: Some(true),
            ..Default::default()
        },
    );
    let reference = integrity_reference(&source);
    let serialized_source = serde_json::to_value(RolloutItem::ResponseItem(source))?;
    let RolloutItem::ResponseItem(reloaded_source) =
        serde_json::from_value::<RolloutItem>(serialized_source)?
    else {
        panic!("expected reloaded response item");
    };
    let ResponseItem::Message {
        internal_chat_message_metadata_passthrough: Some(reloaded_passthrough),
        ..
    } = &reloaded_source.item
    else {
        panic!("expected reloaded message passthrough");
    };
    assert_eq!(reloaded_passthrough.cell_id, None);
    assert_eq!(reloaded_passthrough.executed_tool_calls, None);
    assert_eq!(reloaded_passthrough.tool_calls_complete, None);
    assert_eq!(
        reloaded_source.metadata,
        Some(CodexHarnessMetadata {
            client_authored: true
        })
    );

    let mut resolver = CompactedHistoryResolver::default();
    resolver.index_explicit_sources(&RolloutItem::ResponseItem(reloaded_source.clone()));
    let serialized_checkpoint =
        serde_json::to_value(entry_checkpoint("v2 cold reload", vec![reference]))?;
    let RolloutItem::Compacted(checkpoint) =
        serde_json::from_value::<RolloutItem>(serialized_checkpoint)?
    else {
        panic!("expected reloaded compacted checkpoint");
    };
    assert_eq!(
        resolver
            .resolve_compacted_item_detailed(&checkpoint)
            .expect("durably identical cold source should resolve"),
        Some(vec![reloaded_source])
    );
    Ok(())
}

#[test]
fn v2_reference_rejects_same_id_content_substitution_as_digest_mismatch() {
    let expected = identified_message("v2-content", "original content");
    let reference = integrity_reference(&expected);
    let substituted = identified_message("v2-content", "substituted content");
    let actual_digest =
        CompactedHistoryDigest::from_envelope(&substituted).expect("substitute should encode");
    let mut resolver = CompactedHistoryResolver::default();
    resolver.index_explicit_sources(&RolloutItem::ResponseItem(substituted));
    let RolloutItem::Compacted(checkpoint) = entry_checkpoint("v2 changed", vec![reference]) else {
        panic!("expected compacted checkpoint");
    };

    let error = resolver
        .resolve_compacted_item_detailed(&checkpoint)
        .expect_err("same ID with changed content must fail closed");
    assert!(error.missing_item_ids().is_empty());
    let [mismatch] = error.digest_mismatches() else {
        panic!("expected exactly one digest mismatch: {error}");
    };
    assert_eq!(
        mismatch.item_id(),
        expected.item.id().expect("expected id").as_str()
    );
    assert_eq!(mismatch.actual(), &actual_digest);
    assert_ne!(mismatch.expected(), mismatch.actual());
}

#[test]
fn v2_reference_rejects_same_item_metadata_substitution_as_digest_mismatch() {
    let expected = identified_message("v2-metadata", "same content");
    let reference = integrity_reference(&expected);
    let mut substituted = expected.clone();
    substituted.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let mut resolver = CompactedHistoryResolver::default();
    resolver.index_explicit_sources(&RolloutItem::ResponseItem(substituted));
    let RolloutItem::Compacted(checkpoint) = entry_checkpoint("v2 metadata", vec![reference])
    else {
        panic!("expected compacted checkpoint");
    };

    let error = resolver
        .resolve_compacted_item_detailed(&checkpoint)
        .expect_err("metadata-only substitution must fail closed");
    assert!(error.missing_item_ids().is_empty());
    assert_eq!(error.digest_mismatches().len(), 1);
}

#[test]
fn v2_reference_reports_missing_source_separately_from_digest_mismatch() {
    let source = identified_message("v2-missing", "not persisted");
    let RolloutItem::Compacted(checkpoint) =
        entry_checkpoint("v2 missing", vec![integrity_reference(&source)])
    else {
        panic!("expected compacted checkpoint");
    };

    let error = CompactedHistoryResolver::default()
        .resolve_compacted_item_detailed(&checkpoint)
        .expect_err("missing V2 source must fail closed");
    assert_eq!(
        error.missing_item_ids(),
        &[source.item.id().expect("source id").as_str().to_string()]
    );
    assert!(error.digest_mismatches().is_empty());
}

#[test]
fn v2_checkpoint_resolution_ignores_large_irrelevant_sources() {
    const IRRELEVANT_SOURCE_COUNT: usize = 64;
    const IRRELEVANT_PAYLOAD_BYTES: usize = 16 * 1024;

    let target = identified_message("v2-bounded-target", "selected source");
    let mut rollout = vec![RolloutItem::ResponseItem(target.clone())];
    rollout.extend((0..IRRELEVANT_SOURCE_COUNT).map(|index| {
        RolloutItem::ResponseItem(identified_message(
            &format!("v2-irrelevant-{index}"),
            &format!("{index}:{}", "x".repeat(IRRELEVANT_PAYLOAD_BYTES)),
        ))
    }));
    rollout.push(entry_checkpoint(
        "selected V2 checkpoint",
        vec![integrity_reference(&target)],
    ));

    assert_eq!(
        resolve_checkpoint_at_detailed(&rollout, rollout.len() - 1)
            .expect("target should resolve through the irrelevant prefix"),
        Some(vec![target])
    );
}

#[test]
fn index_many_reference_backed_checkpoints_without_materializing_them() {
    const CHECKPOINT_COUNT: usize = 32;

    let first = identified_message("bounded-0", "top-level source");
    let mut previous = first.clone();
    let mut resolver = CompactedHistoryResolver::default();
    resolver
        .index_item(&RolloutItem::ResponseItem(first))
        .expect("top-level source should index");

    for checkpoint_number in 1..=CHECKPOINT_COUNT {
        let previous_id = previous
            .item
            .id()
            .expect("previous item id")
            .as_str()
            .to_string();
        let next = identified_message(
            &format!("bounded-{checkpoint_number}"),
            &format!("checkpoint source {checkpoint_number}"),
        );
        let checkpoint = entry_checkpoint(
            &format!("checkpoint {checkpoint_number}"),
            vec![
                CompactedHistoryEntry::Reference {
                    item_id: previous_id,
                },
                CompactedHistoryEntry::from(next.clone()),
            ],
        );

        resolver
            .index_item(&checkpoint)
            .expect("reference-backed checkpoint should index");

        let RolloutItem::Compacted(compacted) = &checkpoint else {
            panic!("expected compacted item");
        };
        assert_eq!(compacted.replacement_history, None);
        assert!(compacted.replacement_history_entries.is_some());
        assert!(resolver.knows_exact_envelope(&next));
        previous = next;
    }
}

#[test]
fn resolve_checkpoint_at_returns_exact_target_history_from_older_sources() {
    let mut top_level = identified_message("target-top", "top-level source");
    top_level.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let checkpoint_source = identified_message("target-checkpoint", "checkpoint source");
    let top_level_id = top_level
        .item
        .id()
        .expect("top-level id")
        .as_str()
        .to_string();
    let checkpoint_source_id = checkpoint_source
        .item
        .id()
        .expect("checkpoint source id")
        .as_str()
        .to_string();
    let rollout = vec![
        RolloutItem::ResponseItem(top_level.clone()),
        entry_checkpoint(
            "source checkpoint",
            vec![
                CompactedHistoryEntry::Reference {
                    item_id: top_level_id.clone(),
                },
                CompactedHistoryEntry::from(checkpoint_source.clone()),
            ],
        ),
        entry_checkpoint(
            "target checkpoint",
            vec![
                CompactedHistoryEntry::Reference {
                    item_id: checkpoint_source_id,
                },
                CompactedHistoryEntry::Reference {
                    item_id: top_level_id,
                },
            ],
        ),
    ];

    let resolved = resolve_checkpoint_at(&rollout, 2)
        .expect("target checkpoint should resolve")
        .expect("target checkpoint should have history");

    assert_eq!(resolved, vec![checkpoint_source, top_level]);
    let RolloutItem::Compacted(source) = &rollout[1] else {
        panic!("expected compacted source");
    };
    assert_eq!(source.replacement_history, None);
    assert!(source.replacement_history_entries.is_some());
}

#[test]
fn resolve_checkpoint_at_ignores_unrelated_dangling_refs_in_older_checkpoints() {
    let source = identified_message("source-after-dangling", "complete inline source");
    let source_id = source
        .item
        .id()
        .expect("source item id")
        .as_str()
        .to_string();
    let rollout = vec![
        entry_checkpoint(
            "older checkpoint",
            vec![
                CompactedHistoryEntry::Reference {
                    item_id: "unrelated-missing".to_string(),
                },
                CompactedHistoryEntry::from(source.clone()),
            ],
        ),
        entry_checkpoint(
            "selected checkpoint",
            vec![CompactedHistoryEntry::Reference { item_id: source_id }],
        ),
    ];

    let resolved = resolve_checkpoint_at(&rollout, 1)
        .expect("unrelated older dangling ref should not block target")
        .expect("selected checkpoint should have history");

    assert_eq!(resolved, vec![source]);
}

#[test]
fn resolve_checkpoint_at_does_not_retain_many_irrelevant_large_sources() {
    const IRRELEVANT_SOURCE_COUNT: usize = 256;
    const IRRELEVANT_PAYLOAD_BYTES: usize = 16 * 1024;

    let target = identified_message("bounded-target", "selected source");
    let target_id = target
        .item
        .id()
        .expect("target item id")
        .as_str()
        .to_string();
    let mut rollout = vec![RolloutItem::ResponseItem(target.clone())];
    rollout.extend((0..IRRELEVANT_SOURCE_COUNT).map(|index| {
        RolloutItem::ResponseItem(identified_message(
            &format!("irrelevant-{index}"),
            &format!("{index}:{}", "x".repeat(IRRELEVANT_PAYLOAD_BYTES)),
        ))
    }));
    rollout.push(entry_checkpoint(
        "selected checkpoint",
        vec![CompactedHistoryEntry::Reference { item_id: target_id }],
    ));

    let resolved = resolve_checkpoint_at(&rollout, rollout.len() - 1)
        .expect("target should resolve through the large irrelevant prefix")
        .expect("selected checkpoint should have history");

    assert_eq!(resolved, vec![target]);
}

#[test]
fn requested_source_index_ignores_unrelated_explicit_payloads() {
    let target = identified_message("requested-index-target", "needed");
    let unrelated = identified_message("requested-index-unrelated", &"unrelated".repeat(16 * 1024));
    let requested_item_ids = std::collections::HashSet::from([target
        .item
        .id()
        .expect("target item id")
        .as_str()
        .to_string()]);
    let mut resolver = CompactedHistoryResolver::default();

    resolver.index_explicit_sources_for_ids(
        &RolloutItem::ResponseItem(unrelated.clone()),
        &requested_item_ids,
    );
    resolver.index_explicit_sources_for_ids(
        &RolloutItem::ResponseItem(target.clone()),
        &requested_item_ids,
    );

    assert!(resolver.knows_exact_envelope(&target));
    assert!(!resolver.knows_exact_envelope(&unrelated));
}

#[test]
fn filter_exact_explicit_sources_is_candidate_bounded_and_uses_newest_value() {
    let stale = identified_message("candidate", "stale value");
    let mut newest = identified_message("candidate", "newest value");
    newest.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let inline = identified_message("inline-candidate", "inline value");
    let no_id = ResponseItemEnvelope::new(response_message("user"));
    let rollout = vec![
        RolloutItem::ResponseItem(stale.clone()),
        RolloutItem::ResponseItem(identified_message("irrelevant", "not a candidate")),
        RolloutItem::ResponseItem(newest.clone()),
        entry_checkpoint(
            "explicit inline carrier",
            vec![
                CompactedHistoryEntry::Reference {
                    item_id: "unrelated-dangling".to_string(),
                },
                CompactedHistoryEntry::from(inline.clone()),
            ],
        ),
    ];
    let candidates = vec![
        inline.clone(),
        stale,
        newest.clone(),
        no_id,
        identified_message("absent", "not stored"),
    ];

    let exact = CompactedHistoryResolver::filter_exact_explicit_sources(&rollout, &candidates);

    assert_eq!(exact, vec![inline, newest]);
}

#[test]
fn same_checkpoint_inline_cannot_satisfy_its_reference() {
    let inline = identified_message("same-checkpoint", "inline before reference");
    let inline_id = inline
        .item
        .id()
        .expect("inline item id")
        .as_str()
        .to_string();
    let checkpoint = entry_checkpoint(
        "same checkpoint",
        vec![
            CompactedHistoryEntry::from(inline.clone()),
            CompactedHistoryEntry::Reference {
                item_id: inline_id.clone(),
            },
        ],
    );
    let mut resolver = CompactedHistoryResolver::default();

    let missing = resolver
        .index_item(&checkpoint)
        .expect_err("same-checkpoint reference must fail");

    assert_eq!(missing, vec![inline_id]);
    assert!(!resolver.knows_exact_envelope(&inline));
}

#[test]
fn missing_reference_does_not_partially_index_inline_values() {
    let inline = identified_message("partial-inline", "must not be indexed");
    let checkpoint = entry_checkpoint(
        "missing reference",
        vec![
            CompactedHistoryEntry::from(inline.clone()),
            CompactedHistoryEntry::Reference {
                item_id: "missing-b".to_string(),
            },
            CompactedHistoryEntry::Reference {
                item_id: "missing-a".to_string(),
            },
        ],
    );
    let mut resolver = CompactedHistoryResolver::default();

    let missing = resolver
        .index_item(&checkpoint)
        .expect_err("missing references must fail closed");

    assert_eq!(missing, vec!["missing-a", "missing-b"]);
    assert!(!resolver.knows_exact_envelope(&inline));
}

#[test]
fn materialize_rebases_sources_to_current_checkpoint_and_suffix() {
    let abandoned = identified_message("abandoned", "outside current context");
    let current = identified_message("current", "inside current context");
    let suffix = identified_message("suffix", "after current checkpoint");
    let abandoned_id = abandoned
        .item
        .id()
        .expect("abandoned item id")
        .as_str()
        .to_string();
    let mut resolver = CompactedHistoryResolver::default();
    let mut abandoned_item = RolloutItem::ResponseItem(abandoned.clone());
    let mut checkpoint = legacy_checkpoint("current checkpoint", vec![current.clone()]);
    let mut suffix_item = RolloutItem::ResponseItem(suffix.clone());

    resolver
        .materialize_item(&mut abandoned_item)
        .expect("top-level source should index");
    resolver
        .materialize_item(&mut checkpoint)
        .expect("checkpoint should materialize");

    assert!(!resolver.knows_exact_envelope(&abandoned));
    assert!(resolver.knows_exact_envelope(&current));

    resolver
        .materialize_item(&mut suffix_item)
        .expect("suffix should index");
    assert!(resolver.knows_exact_envelope(&current));
    assert!(resolver.knows_exact_envelope(&suffix));

    let mut invalid_future = entry_checkpoint(
        "invalid future checkpoint",
        vec![CompactedHistoryEntry::Reference {
            item_id: abandoned_id.clone(),
        }],
    );
    let error = resolver
        .materialize_item(&mut invalid_future)
        .expect_err("a source dropped by the current checkpoint must not leak forward");
    assert_eq!(error, vec![abandoned_id]);
    assert!(resolver.knows_exact_envelope(&current));
    assert!(resolver.knows_exact_envelope(&suffix));
}

#[test]
fn reencode_first_checkpoint_does_not_reference_items_from_itself() {
    let source = identified_message("self", "first checkpoint source");
    let expected_history = vec![source.clone(), source];
    let mut checkpoint = legacy_checkpoint("first", expected_history.clone());
    let mut resolver = CompactedHistoryResolver::default();

    resolver
        .reencode_item_with_backward_references(&mut checkpoint)
        .expect("legacy checkpoint should re-encode");

    let RolloutItem::Compacted(compacted) = checkpoint else {
        panic!("expected compacted item");
    };
    assert_eq!(compacted.replacement_history, Some(expected_history));
    assert_eq!(compacted.replacement_history_entries, None);
}

#[test]
fn reencode_later_checkpoint_references_previous_inline_source() {
    let previous_inline = identified_message("previous", "previous inline source");
    let previous_id = previous_inline
        .item
        .id()
        .expect("previous inline id")
        .as_str()
        .to_string();
    let new_inline = identified_message("new", "new inline source");
    let mut first = legacy_checkpoint("first", vec![previous_inline.clone()]);
    let mut second = legacy_checkpoint("second", vec![previous_inline, new_inline.clone()]);
    let mut resolver = CompactedHistoryResolver::default();

    resolver
        .reencode_item_with_backward_references(&mut first)
        .expect("first checkpoint should re-encode");
    resolver
        .reencode_item_with_backward_references(&mut second)
        .expect("second checkpoint should re-encode");

    let RolloutItem::Compacted(compacted) = second else {
        panic!("expected compacted item");
    };
    assert_eq!(compacted.replacement_history, None);
    assert_eq!(
        compacted.replacement_history_entries,
        Some(vec![
            CompactedHistoryEntry::Reference {
                item_id: previous_id,
            },
            CompactedHistoryEntry::from(new_inline),
        ])
    );
}

#[test]
fn reencode_keeps_duplicate_ids_inline_across_repeated_checkpoints() {
    let first_value = identified_message("reused-id", "first value");
    let second_value = identified_message("reused-id", "second value");
    let expected_history = vec![first_value, second_value];
    let mut first = legacy_checkpoint("first", expected_history.clone());
    let mut second = legacy_checkpoint("second", expected_history.clone());
    let mut resolver = CompactedHistoryResolver::default();

    resolver
        .reencode_item_with_backward_references(&mut first)
        .expect("first duplicate-ID checkpoint should remain exact");
    resolver
        .reencode_item_with_backward_references(&mut second)
        .expect("repeated duplicate-ID checkpoint should remain exact");

    for checkpoint in [first, second] {
        let RolloutItem::Compacted(compacted) = checkpoint else {
            panic!("expected compacted item");
        };
        assert_eq!(
            compacted.replacement_history,
            Some(expected_history.clone())
        );
        assert_eq!(compacted.replacement_history_entries, None);
    }
}

#[test]
fn reencode_rebases_sources_to_complete_current_checkpoint() {
    let abandoned = identified_message("reencode-abandoned", "outside checkpoint");
    let current = identified_message("reencode-current", "current checkpoint");
    let suffix = identified_message("reencode-suffix", "later suffix");
    let mut resolver = CompactedHistoryResolver::default();
    let mut abandoned_item = RolloutItem::ResponseItem(abandoned.clone());
    let mut checkpoint = legacy_checkpoint("current", vec![current.clone()]);
    let mut suffix_item = RolloutItem::ResponseItem(suffix.clone());

    resolver
        .reencode_item_with_backward_references(&mut abandoned_item)
        .expect("top-level source should index");
    resolver
        .reencode_item_with_backward_references(&mut checkpoint)
        .expect("checkpoint should re-encode");

    assert!(!resolver.knows_exact_envelope(&abandoned));
    assert!(resolver.knows_exact_envelope(&current));

    resolver
        .reencode_item_with_backward_references(&mut suffix_item)
        .expect("suffix should index");
    assert!(resolver.knows_exact_envelope(&current));
    assert!(resolver.knows_exact_envelope(&suffix));
}

#[test]
fn reencode_requires_metadata_to_match_for_a_reference() {
    let original = identified_message("metadata", "same response item");
    let item_id = original
        .item
        .id()
        .expect("metadata item id")
        .as_str()
        .to_string();
    let mut top_level = RolloutItem::ResponseItem(original.clone());
    let mut changed_metadata = original;
    changed_metadata.metadata = Some(CodexHarnessMetadata {
        client_authored: true,
    });
    let expected_changed_metadata = changed_metadata.clone();
    let mut first_checkpoint =
        legacy_checkpoint("metadata changed", vec![changed_metadata.clone()]);
    let mut second_checkpoint = legacy_checkpoint("metadata now exact", vec![changed_metadata]);
    let mut resolver = CompactedHistoryResolver::default();

    resolver
        .reencode_item_with_backward_references(&mut top_level)
        .expect("top-level response item should index");
    resolver
        .reencode_item_with_backward_references(&mut first_checkpoint)
        .expect("metadata-mismatched checkpoint should re-encode");

    let RolloutItem::Compacted(first) = &first_checkpoint else {
        panic!("expected compacted item");
    };
    assert_eq!(
        first.replacement_history,
        Some(vec![expected_changed_metadata])
    );
    assert_eq!(first.replacement_history_entries, None);

    resolver
        .reencode_item_with_backward_references(&mut second_checkpoint)
        .expect("metadata-exact checkpoint should re-encode");

    let RolloutItem::Compacted(second) = second_checkpoint else {
        panic!("expected compacted item");
    };
    assert_eq!(second.replacement_history, None);
    assert_eq!(
        second.replacement_history_entries,
        Some(vec![CompactedHistoryEntry::Reference { item_id }])
    );
}

#[test]
fn reencode_preserves_empty_legacy_history() {
    let mut checkpoint = legacy_checkpoint("empty", Vec::new());
    let mut resolver = CompactedHistoryResolver::default();

    resolver
        .reencode_item_with_backward_references(&mut checkpoint)
        .expect("empty checkpoint should re-encode");

    let RolloutItem::Compacted(compacted) = checkpoint else {
        panic!("expected compacted item");
    };
    assert_eq!(compacted.replacement_history, Some(Vec::new()));
    assert_eq!(compacted.replacement_history_entries, None);
}

#[test]
fn reencode_unresolved_checkpoint_fails_without_mutating_or_indexing_it() {
    let failed_inline = identified_message("failed-inline", "must not become a source");
    let expected = CompactedItem {
        message: "unresolved".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![
            CompactedHistoryEntry::Reference {
                item_id: "missing-z".to_string(),
            },
            CompactedHistoryEntry::from(failed_inline.clone()),
            CompactedHistoryEntry::Reference {
                item_id: "missing-a".to_string(),
            },
            CompactedHistoryEntry::Reference {
                item_id: "missing-z".to_string(),
            },
        ]),
        mcp_resource_origins: None,
        window_number: None,
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    };
    let mut unresolved = RolloutItem::Compacted(expected.clone());
    let mut resolver = CompactedHistoryResolver::default();

    let missing = resolver
        .reencode_item_with_backward_references(&mut unresolved)
        .expect_err("unresolved checkpoint must fail closed");

    assert_eq!(missing, vec!["missing-a", "missing-z"]);
    let RolloutItem::Compacted(after_failure) = unresolved else {
        panic!("expected compacted item");
    };
    assert_eq!(after_failure, expected);

    let mut following = legacy_checkpoint("following", vec![failed_inline.clone()]);
    resolver
        .reencode_item_with_backward_references(&mut following)
        .expect("following legacy checkpoint should re-encode");
    let RolloutItem::Compacted(following) = following else {
        panic!("expected compacted item");
    };
    assert_eq!(following.replacement_history, Some(vec![failed_inline]));
    assert_eq!(following.replacement_history_entries, None);
}

#[test]
fn materialize_compacted_histories_resolves_top_level_and_checkpoint_sources() {
    let top_level = identified_message("top", "top-level source");
    let checkpoint_source = identified_message("checkpoint", "checkpoint source");
    let top_level_id = top_level
        .item
        .id()
        .expect("top-level id")
        .as_str()
        .to_string();
    let checkpoint_id = checkpoint_source
        .item
        .id()
        .expect("checkpoint id")
        .as_str()
        .to_string();
    let rollout = vec![
        RolloutItem::ResponseItem(top_level.clone()),
        RolloutItem::Compacted(CompactedItem {
            message: "first".to_string(),
            replacement_history: None,
            replacement_history_entries: Some(vec![
                CompactedHistoryEntry::Reference {
                    item_id: top_level_id.clone(),
                },
                CompactedHistoryEntry::from(checkpoint_source.clone()),
            ]),
            mcp_resource_origins: None,
            window_number: Some(1),
            first_window_id: None,
            previous_window_id: None,
            window_id: None,
        }),
        RolloutItem::Compacted(CompactedItem {
            message: "second".to_string(),
            replacement_history: None,
            replacement_history_entries: Some(vec![
                CompactedHistoryEntry::Reference {
                    item_id: checkpoint_id,
                },
                CompactedHistoryEntry::Reference {
                    item_id: top_level_id,
                },
            ]),
            mcp_resource_origins: None,
            window_number: Some(2),
            first_window_id: None,
            previous_window_id: None,
            window_id: None,
        }),
    ];

    let materialized = materialize_compacted_histories(&rollout);

    assert!(materialized.unresolved_item_ids.is_empty());
    let RolloutItem::Compacted(first) = &materialized.rollout_items[1] else {
        panic!("expected first checkpoint");
    };
    assert_eq!(
        first.replacement_history,
        Some(vec![top_level.clone(), checkpoint_source.clone()])
    );
    assert_eq!(first.replacement_history_entries, None);
    let RolloutItem::Compacted(second) = &materialized.rollout_items[2] else {
        panic!("expected second checkpoint");
    };
    assert_eq!(
        second.replacement_history,
        Some(vec![checkpoint_source, top_level])
    );
    assert_eq!(second.replacement_history_entries, None);
}

#[test]
fn materialize_compacted_histories_reports_and_preserves_unresolved_checkpoint() {
    let rollout = vec![RolloutItem::Compacted(CompactedItem {
        message: "unresolved".to_string(),
        replacement_history: None,
        replacement_history_entries: Some(vec![CompactedHistoryEntry::Reference {
            item_id: "missing-item".to_string(),
        }]),
        mcp_resource_origins: None,
        window_number: Some(1),
        first_window_id: None,
        previous_window_id: None,
        window_id: None,
    })];

    let materialized = materialize_compacted_histories(&rollout);

    assert_eq!(materialized.unresolved_item_ids, vec!["missing-item"]);
    let RolloutItem::Compacted(compacted) = &materialized.rollout_items[0] else {
        panic!("expected checkpoint");
    };
    assert_eq!(compacted.replacement_history, None);
    assert!(compacted.replacement_history_entries.is_some());
}

#[test]
/// Preserves the stored compacted-item window metadata shape.
fn compacted_item_serializes_window_number_and_id() -> Result<()> {
    let item = CompactedItem {
        message: "summary".to_string(),
        replacement_history: None,
        replacement_history_entries: None,
        mcp_resource_origins: None,
        window_number: Some(3),
        first_window_id: Some("019b3f6e-0000-7000-8000-000000000001".to_string()),
        previous_window_id: Some("019b3f6e-0000-7000-8000-000000000002".to_string()),
        window_id: Some("019b3f6e-7a10-7cc3-8b6e-1d09e2f7a001".to_string()),
    };

    assert_eq!(
        serde_json::to_value(item)?,
        json!({
            "message": "summary",
            "window_number": 3,
            "first_window_id": "019b3f6e-0000-7000-8000-000000000001",
            "previous_window_id": "019b3f6e-0000-7000-8000-000000000002",
            "window_id": "019b3f6e-7a10-7cc3-8b6e-1d09e2f7a001",
        })
    );
    Ok(())
}

#[test]
/// Keeps legacy numeric window IDs readable in stored compacted items.
fn compacted_item_migrates_legacy_numeric_window_id() -> Result<()> {
    let item = serde_json::from_value::<CompactedItem>(json!({
        "message": "summary",
        "window_id": 3,
    }))?;

    assert_eq!(
        item,
        CompactedItem {
            message: "summary".to_string(),
            replacement_history: None,
            replacement_history_entries: None,
            mcp_resource_origins: None,
            window_number: Some(3),
            first_window_id: None,
            previous_window_id: None,
            window_id: None,
        }
    );
    Ok(())
}

#[test]
fn copied_history_uses_persisted_history_mode() -> Result<()> {
    let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001")?;
    let session_meta = RolloutItem::SessionMeta(SessionMetaLine {
        meta: SessionMeta {
            session_id: thread_id.into(),
            id: thread_id,
            history_mode: ThreadHistoryMode::Legacy,
            ..SessionMeta::default()
        },
        git: None,
    });
    let history = InitialHistory::Resumed(ResumedHistory {
        conversation_id: thread_id,
        history: Arc::new(vec![session_meta.clone()]),
        rollout_path: None,
    });

    assert_eq!(
        history.get_history_mode(ThreadHistoryMode::Paginated),
        ThreadHistoryMode::Legacy
    );
    assert_eq!(
        InitialHistory::Forked(vec![session_meta]).get_history_mode(ThreadHistoryMode::Paginated),
        ThreadHistoryMode::Legacy
    );
    assert_eq!(
        InitialHistory::New.get_history_mode(ThreadHistoryMode::Paginated),
        ThreadHistoryMode::Paginated
    );
    assert_eq!(
        InitialHistory::Resumed(ResumedHistory {
            conversation_id: thread_id,
            history: Arc::new(Vec::new()),
            rollout_path: None,
        })
        .get_history_mode(ThreadHistoryMode::Paginated),
        ThreadHistoryMode::Paginated
    );
    Ok(())
}

#[test]
fn multi_agent_version_uses_newest_present_session_meta_value() -> Result<()> {
    let thread_id = ThreadId::from_string("67e55044-10b1-426f-9247-bb680e5fe0c8")?;
    let older_meta = SessionMetaLine {
        meta: SessionMeta {
            session_id: thread_id.into(),
            id: thread_id,
            multi_agent_version: Some(MultiAgentVersion::V2),
            ..Default::default()
        },
        git: None,
    };
    let newer_meta_without_version = SessionMetaLine {
        meta: SessionMeta {
            session_id: thread_id.into(),
            id: thread_id,
            multi_agent_version: None,
            ..Default::default()
        },
        git: None,
    };

    assert_eq!(
        multi_agent_version_from_items(
            &[
                RolloutItem::SessionMeta(older_meta),
                RolloutItem::SessionMeta(newer_meta_without_version),
            ],
            Some(thread_id),
        ),
        Some(MultiAgentVersion::V2)
    );
    Ok(())
}
