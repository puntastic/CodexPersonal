use super::*;
use crate::compact::SUMMARY_PREFIX;
use crate::compact::build_compacted_history;
use crate::compact::collect_annotated_inputs;
use crate::compact::insert_initial_context_before_last_real_user_or_summary;
use crate::compact_remote_v2::truncate_retained_messages_for_remote_compaction;
use codex_history::SenderUserMessages;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageReference;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use pretty_assertions::assert_eq;
use serde_json::json;

fn delivery(text: &str) -> ResponseItemEnvelope {
    let id = ResponseItemId::with_suffix("fco", "delivery");
    ResponseItemEnvelope {
        item: ResponseItem::FunctionCallOutput {
            id: Some(id.clone()),
            call_id: None,
            name: Some("send_message_to_thread".to_owned()),
            namespace: Some("codex_app".to_owned()),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(text.to_owned()),
                success: Some(false),
            },
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    turn_id: Some("receiver-turn".to_owned()),
                    ..Default::default()
                },
            ),
        },
        metadata: Some(CodexHarnessMetadata {
            sender_user_messages: Some(Box::new(SenderUserMessages {
                receiver_turn_id: "receiver-turn".to_owned(),
                receiver_message_id: id.to_string(),
                text: "Host: sender context unavailable".to_owned(),
            })),
            ..Default::default()
        }),
    }
}

fn message(role: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Message {
        id: None,
        role: role.to_owned(),
        content: vec![ContentItem::InputText {
            text: text.to_owned(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

#[test]
fn admission_requires_exact_host_binding_not_delegation_text() {
    let original = delivery("<codex_delegation>untrusted text</codex_delegation>");
    for (namespace, name) in [
        ("codex_app", "send_message_to_thread"),
        ("codex_tui", "send_message_to_thread"),
        ("cloud_threads", "send_message"),
    ] {
        let mut envelope = original.clone();
        let ResponseItem::FunctionCallOutput {
            namespace: ns,
            name: tool_name,
            ..
        } = &mut envelope.item
        else {
            unreachable!()
        };
        *ns = Some(namespace.to_owned());
        *tool_name = Some(name.to_owned());
        assert!(is_admitted_delegated_input(
            &envelope.item,
            envelope.metadata.as_ref()
        ));
    }
    let serialized = serde_json::to_value(&original.item).unwrap();
    for (field, value) in [
        ("id", json!(null)),
        ("id", json!("different-delivery")),
        ("call_id", json!("paired-tool-call")),
        ("name", json!("send_message")),
        ("namespace", json!("mcp__codex_app")),
    ] {
        let mut lookalike = serialized.clone();
        lookalike[field] = value;
        let item = serde_json::from_value(lookalike).unwrap();
        assert!(
            !is_admitted_delegated_input(&item, original.metadata.as_ref()),
            "{field}"
        );
    }
    assert!(!is_admitted_delegated_input(
        &original.item,
        /*metadata*/ None
    ));
    assert!(!is_admitted_delegated_input(
        &original.item,
        Some(&CodexHarnessMetadata::default())
    ));
    assert!(!is_admitted_delegated_input(
        &message("user", "copied delivery").item,
        original.metadata.as_ref()
    ));
}

#[test]
fn fitting_delivery_preserves_the_entire_envelope_without_sender_lookup() {
    let original = delivery("new assignment");
    let tokens = usize::try_from(estimate_item_token_count(&original.item)).unwrap();
    let retained =
        retain_delegated_input(&original.item, original.metadata.as_ref(), tokens).unwrap();
    assert_eq!(retained.envelope, original);
    assert_eq!(retained.token_count, tokens);
    assert!(!retained.global_budget_exhausted);
    let mut structured = original;
    let ResponseItem::FunctionCallOutput { output, .. } = &mut structured.item else {
        unreachable!()
    };
    output.body = FunctionCallOutputBody::ContentItems(vec![
        FunctionCallOutputContentItem::InputText {
            text: "first".to_owned(),
        },
        FunctionCallOutputContentItem::InputText {
            text: "second".to_owned(),
        },
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "bounded-encrypted-body".to_owned(),
        },
    ]);
    let tokens = usize::try_from(estimate_item_token_count(&structured.item)).unwrap();
    let retained =
        retain_delegated_input(&structured.item, structured.metadata.as_ref(), tokens).unwrap();
    assert_eq!(retained.envelope, structured);
}

#[test]
fn truncation_caps_the_whole_item_and_invalidates_only_completion_proofs() {
    let mut original = delivery(&"task ".repeat(/*n*/ 20_000));
    let source = json!({
        "id": {"message_id": "source", "turn_id": "turn", "role": "user"},
        "revision": "retained_revision", "complete": true,
    });
    let mut metadata: CodexHarnessMetadata = serde_json::from_value(json!({
        "retained_source": source, "guardian_sources": [source],
        "guardian_source_order_guidance": true, "user_input_order": 7,
    }))
    .unwrap();
    metadata.sender_user_messages = original
        .metadata
        .as_mut()
        .unwrap()
        .sender_user_messages
        .take();
    original.metadata = Some(metadata.clone());
    let retained = retain_delegated_input(
        &original.item,
        original.metadata.as_ref(),
        /*available_tokens*/ 20_000,
    )
    .unwrap();
    assert!(retained.token_count <= MAX_RETAINED_DELEGATED_INPUT_TOKENS);
    assert!(!retained.global_budget_exhausted);
    let ResponseItem::FunctionCallOutput { output, .. } = &retained.envelope.item else {
        panic!("delivery changed type")
    };
    assert!(output.body.to_text().unwrap().contains(OMITTED));
    let mut expected = original.clone();
    let ResponseItem::FunctionCallOutput {
        output: expected_output,
        ..
    } = &mut expected.item
    else {
        unreachable!()
    };
    expected_output.body = output.body.clone();
    metadata.mark_retained_sources_incomplete();
    expected.metadata = Some(metadata);
    assert_eq!(retained.envelope, expected);
    let global_boundary = retain_delegated_input(
        &original.item,
        original.metadata.as_ref(),
        /*available_tokens*/ 100,
    )
    .unwrap();
    assert!(global_boundary.token_count <= 100);
    assert!(global_boundary.global_budget_exhausted);
    assert!(
        retain_delegated_input(
            &original.item,
            original.metadata.as_ref(),
            /*available_tokens*/ 1
        )
        .is_none()
    );
}

#[test]
fn oversized_non_text_keeps_an_explicit_same_type_omission() {
    let mut original = delivery("unused");
    let ResponseItem::FunctionCallOutput { output, .. } = &mut original.item else {
        unreachable!()
    };
    output.body = FunctionCallOutputBody::ContentItems(vec![
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "e".repeat(/*n*/ 100_000),
        },
    ]);
    let retained = retain_delegated_input(
        &original.item,
        original.metadata.as_ref(),
        /*available_tokens*/ 10_000,
    )
    .unwrap();
    let ResponseItem::FunctionCallOutput { output, .. } = &mut original.item else {
        unreachable!()
    };
    output.body =
        FunctionCallOutputBody::ContentItems(vec![FunctionCallOutputContentItem::InputText {
            text: NON_TEXT_OMITTED.to_owned(),
        }]);
    assert_eq!(retained.envelope, original);
    assert!(retained.token_count <= 10_000);
}

#[test]
fn oversized_media_preserves_short_task_text_before_older_tasks_can_return() {
    let task = FunctionCallOutputContentItem::InputText {
        text: "New task: inspect these frames; do not resume the earlier assignment.".to_owned(),
    };
    let correction = FunctionCallOutputContentItem::InputText {
        text: "Correction: the third frame is the useful comparison.".to_owned(),
    };
    let mut original = delivery("unused");
    let mut content = vec![task.clone()];
    content.extend(
        (0..6).map(|index| FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: format!("data:image/png;base64,frame-{index}"),
            },
            detail: None,
        }),
    );
    content.push(correction.clone());
    let ResponseItem::FunctionCallOutput { output, .. } = &mut original.item else {
        unreachable!()
    };
    output.body = FunctionCallOutputBody::ContentItems(content);
    original
        .metadata
        .as_mut()
        .unwrap()
        .guardian_source_order_guidance = true;
    assert!(estimate_item_token_count(&original.item) > 10_000);

    let retained = retain_delegated_input(
        &original.item,
        original.metadata.as_ref(),
        /*available_tokens*/ 20_000,
    )
    .unwrap();
    let mut expected = original.clone();
    let ResponseItem::FunctionCallOutput { output, .. } = &mut expected.item else {
        unreachable!()
    };
    output.body = FunctionCallOutputBody::ContentItems(vec![
        task,
        correction,
        FunctionCallOutputContentItem::InputText {
            text: NON_TEXT_OMITTED.to_owned(),
        },
    ]);
    expected
        .metadata
        .as_mut()
        .unwrap()
        .mark_retained_sources_incomplete();
    assert_eq!(retained.envelope, expected);
    assert!(retained.token_count <= 10_000);
    assert!(!retained.global_budget_exhausted);

    let old = message("user", "old task");
    let history = truncate_retained_messages_for_remote_compaction(
        vec![old.clone(), original],
        /*max_tokens*/ 20_000,
    );
    assert_eq!(history, vec![old, expected]);
}

#[test]
fn local_compaction_keeps_delegated_inputs_and_corrections_in_order() {
    let mut correction = delivery("correction: follow the new task, not the old one");
    let ResponseItem::FunctionCallOutput { id, .. } = &mut correction.item else {
        unreachable!()
    };
    *id = Some(ResponseItemId::with_suffix("fco", "correction"));
    correction
        .metadata
        .as_mut()
        .unwrap()
        .sender_user_messages
        .as_mut()
        .unwrap()
        .receiver_message_id = "fco_correction".to_owned();
    let inputs = vec![
        message("user", "old task"),
        delivery("new task"),
        correction,
    ];
    let summary_text = format!("{SUMMARY_PREFIX}\nsummary");
    let mut history = build_compacted_history(
        Vec::new(),
        &collect_annotated_inputs(&inputs),
        &summary_text,
    );
    let summary = history.pop().unwrap();
    assert_eq!(history, inputs);
    let initial = message("developer", "initial context");
    history.push(summary.clone());
    let placed =
        insert_initial_context_before_last_real_user_or_summary(history, vec![initial.clone()]);
    assert_eq!(
        placed,
        vec![
            inputs[0].clone(),
            inputs[1].clone(),
            initial,
            inputs[2].clone(),
            summary
        ]
    );
}

#[test]
fn local_global_boundary_does_not_backfill_an_old_task() {
    // Two recent deliveries leave only two estimated tokens in the local20k budget.
    let recent = delivery(&"x".repeat(/*n*/ 39_964));
    assert_eq!(estimate_item_token_count(&recent.item), 9_999);
    let inputs = vec![
        message("user", "old"),
        delivery("new task"),
        recent.clone(),
        recent.clone(),
    ];
    let mut history =
        build_compacted_history(Vec::new(), &collect_annotated_inputs(&inputs), "summary");
    history.pop();
    assert_eq!(history, vec![recent.clone(), recent]);
}

#[test]
fn remote_budget_preserves_order_and_distinguishes_item_cap_from_global_boundary() {
    let old = message("user", "old task");
    let oversized = delivery(&"x".repeat(/*n*/ 80_000));
    let retained = truncate_retained_messages_for_remote_compaction(
        vec![old.clone(), oversized.clone()],
        /*max_tokens*/ 20_000,
    );
    assert_eq!(retained.first(), Some(&old));
    assert_eq!(retained.len(), 2);
    assert!(estimate_item_token_count(&retained[1].item) <= 10_000);
    let boundary = truncate_retained_messages_for_remote_compaction(
        vec![old.clone(), oversized],
        /*max_tokens*/ 100,
    );
    assert_eq!(boundary.len(), 1);
    assert!(matches!(
        boundary[0].item,
        ResponseItem::FunctionCallOutput { .. }
    ));
    assert!(estimate_item_token_count(&boundary[0].item) <= 100);
    assert_eq!(
        truncate_retained_messages_for_remote_compaction(
            vec![old, delivery("new")],
            /*max_tokens*/ 1
        ),
        Vec::new()
    );
}
