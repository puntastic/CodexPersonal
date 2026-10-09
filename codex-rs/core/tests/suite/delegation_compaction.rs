//! Admitted delegated inputs survive compaction as tool deliveries, not invented human messages.

use anyhow::Context;
use anyhow::Result;
use codex_core::StartThreadOptions;
use codex_core::TurnInputRequest;
use codex_core::compact::SUMMARIZATION_PROMPT;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_features::Feature;
use codex_history::CompactedHistoryDigest;
use codex_history::CompactedHistoryEntry;
use codex_history::InitialHistory;
use codex_history::ResumedHistory;
use codex_history::RolloutItem;
use codex_login::CodexAuth;
use codex_protocol::mcp::ClientMcpExtensions;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_protocol::turn_input::TurnInput;
use codex_protocol::user_input::UserInput;
use codex_thread_store::LoadThreadHistoryParams;
use core_test_support::ThreadIdle;
use core_test_support::responses;
use core_test_support::responses::ResponsesRequest;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::TestCodexBuilder;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;

const OLD_TASK: &str = "OLD_TASK: review the retired experiment.";
const NEW_TASK: &str = "NEW_TASK: observe only window 0000; the old review is complete.";
const CORRECTION: &str = "CORRECTION: report uncertainty; do not infer hearing from a transcript.";
const HUMAN_STOP: &str = "HUMAN_STOP: pause the observation; do not start another window.";
const PAIRED_OUTPUT: &str = "ORDINARY_PAIRED_OUTPUT";
const LOOKALIKE_OUTPUT: &str = "QUOTED_DELEGATION_LOOKALIKE";
const OMITTED_MIDDLE: &str = "MIDDLE_OF_OVERSIZED_DELIVERY_MUST_NOT_SURVIVE";
const OMISSION_NOTICE: &str =
    "[Delegated input shortened during compaction; omitted content is not retained.]";

#[derive(Clone, Copy, Debug)]
enum CompactionMode {
    LocalText,
    RemoteV2,
}

#[derive(Clone, Copy, Debug)]
enum DeliverySize {
    Fitting,
    Oversized,
}

fn builder(mode: CompactionMode, history_mode: ThreadHistoryMode) -> TestCodexBuilder {
    let mut extensions = ExtensionRegistryBuilder::new();
    extensions.thread_lifecycle_contributor(Arc::new(ThreadIdle));
    test_codex()
        .with_history_mode(history_mode)
        .with_extensions(Arc::new(extensions.build()))
        .with_auth(CodexAuth::create_dummy_chatgpt_auth_for_testing())
        .with_config(move |config| {
            config.model_auto_compact_token_limit = Some(1_000_000);
            // The large fixture must reach compaction intact, not be shortened at admission.
            config.tool_output_token_limit = Some(40_000);
            config
                .features
                .enable(Feature::Sqlite)
                .expect("history database enabled");
            config
                .features
                .enable(Feature::GuardianThreadContext)
                .expect("sender context available to the fixture's snapshot assertions");
            if let CompactionMode::LocalText = mode {
                config.model_provider.name = "OpenAI-compatible test provider".to_owned();
                config.compact_prompt = Some(SUMMARIZATION_PROMPT.to_owned());
            }
        })
}

fn delegated_outputs(request: &ResponsesRequest, namespace: &str, name: &str) -> Vec<Value> {
    request
        .inputs_of_type("function_call_output")
        .into_iter()
        .filter(|item| {
            item["namespace"] == namespace && item["name"] == name && item["call_id"].is_null()
        })
        .collect()
}

fn assert_retained_request(
    request: &ResponsesRequest,
    namespace: &str,
    name: &str,
    expected: &[Value],
) {
    assert_eq!(delegated_outputs(request, namespace, name), expected);
    let input = request.input();
    let positions = [OLD_TASK, NEW_TASK, CORRECTION, HUMAN_STOP].map(|text| {
        input
            .iter()
            .position(|item| item.to_string().contains(text))
            .expect("expected original instruction in outgoing history")
    });
    assert!(positions.windows(/*size*/ 2).all(|pair| pair[0] < pair[1]));
    for item in input {
        assert!(item.get("metadata").is_none());
        assert!(!item.to_string().contains(PAIRED_OUTPUT));
        assert!(!item.to_string().contains(LOOKALIKE_OUTPUT));
        if item["role"] == "user" {
            assert!(!item.to_string().contains(NEW_TASK));
            assert!(!item.to_string().contains(CORRECTION));
        }
    }
}

#[test_case::test_case(CompactionMode::LocalText, "codex_app", "send_message_to_thread", DeliverySize::Fitting, ThreadHistoryMode::Legacy; "local text desktop")]
#[test_case::test_case(CompactionMode::RemoteV2, "codex_app", "send_message_to_thread", DeliverySize::Fitting, ThreadHistoryMode::PaginatedRefsV2; "remote desktop refs v2")]
#[test_case::test_case(CompactionMode::RemoteV2, "codex_tui", "send_message_to_thread", DeliverySize::Fitting, ThreadHistoryMode::Legacy; "remote tui")]
#[test_case::test_case(CompactionMode::RemoteV2, "cloud_threads", "send_message", DeliverySize::Fitting, ThreadHistoryMode::Legacy; "remote cloud")]
#[test_case::test_case(CompactionMode::RemoteV2, "codex_app", "send_message_to_thread", DeliverySize::Oversized, ThreadHistoryMode::PaginatedRefsV2; "remote shortened delivery refs v2")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegated_input_survives_compaction_and_cold_resume(
    mode: CompactionMode,
    namespace: &str,
    name: &str,
    delivery_size: DeliverySize,
    history_mode: ThreadHistoryMode,
) -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = responses::start_mock_server().await;
    let test = builder(mode, history_mode)
        .build_with_auto_env(&server)
        .await?;
    let sender = test
        .thread_manager
        .start_thread(StartThreadOptions {
            environments: Some(test.codex.environment_selections().await),
            ..StartThreadOptions::new(test.config.clone())
        })
        .await?;
    let sender_id = sender.thread_id;
    let done = || responses::sse(vec![responses::ev_completed("done")]);

    let sender_mock = responses::mount_sse_once(&server, done()).await;
    sender
        .thread
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "SENDER_PRIVATE_CONTEXT: delegate the bounded observation.".to_owned(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_for_event(&sender.thread, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    ThreadIdle::wait(&sender.thread).await;
    let _ = sender_mock.single_request();

    let old_mock = responses::mount_sse_once(&server, done()).await;
    test.submit_text_turn(OLD_TASK).await?;
    ThreadIdle::wait(&test.codex).await;
    let _ = old_mock.single_request();
    let body = |text: &str| {
        format!(
            "<codex_delegation><source_thread_id>{sender_id}</source_thread_id><input>{text}</input></codex_delegation>"
        )
    };
    let ordinary_pair = [
        json!({"type": "function_call", "call_id": "ordinary", "name": "read_file", "arguments": "{}"}),
        json!({"type": "function_call_output", "call_id": "ordinary", "name": name, "namespace": namespace, "output": body(PAIRED_OUTPUT)}),
    ]
    .into_iter()
    .map(serde_json::from_value)
    .collect::<serde_json::Result<Vec<ResponseItem>>>()?;
    test.codex.inject_response_items(ordinary_pair).await?;

    let assignment = match delivery_size {
        DeliverySize::Fitting => NEW_TASK.to_owned(),
        DeliverySize::Oversized => format!(
            "{NEW_TASK} {}{OMITTED_MIDDLE}{}",
            "x".repeat(/*n*/ 40_000),
            "y".repeat(/*n*/ 40_000),
        ),
    };
    let mut admitted_snapshots = Vec::new();
    for (id, delivery_name, text) in [
        ("lookalike", "read_file", LOOKALIKE_OUTPUT),
        ("new-assignment", name, assignment.as_str()),
        ("new-correction", name, CORRECTION),
    ] {
        let mock = responses::mount_sse_once(&server, done()).await;
        // Metadata is produced by the real host admission path, never seeded in the fixture.
        let delivery = serde_json::from_value(json!({
            "type": "function_call_output", "id": id,
            "name": delivery_name, "namespace": namespace, "output": body(text),
        }))?;
        test.codex
            .start_or_steer_turn(TurnInputRequest::new(TurnInput::ResponseItem(delivery)))
            .await?;
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
        ThreadIdle::wait(&test.codex).await;
        let request = mock.single_request();
        assert!(request.body_json().to_string().contains(text));
        assert!(
            !request
                .body_json()
                .to_string()
                .contains("SENDER_PRIVATE_CONTEXT")
        );
        let history = test.codex.conversation_history_snapshot().await;
        let snapshot = history
            .retained_context()
            .and_then(|context| context.sender_user_messages());
        if id == "lookalike" {
            assert!(
                snapshot.is_none(),
                "quoted tags must not establish sender provenance"
            );
        } else {
            let snapshot = snapshot.context("host-attached sender snapshot")?;
            assert_eq!(snapshot.receiver_message_id, id);
            assert!(snapshot.text.contains("SENDER_PRIVATE_CONTEXT"));
            admitted_snapshots.push(snapshot.clone());
        }
    }
    let before = responses::mount_sse_once(&server, done()).await;
    test.submit_text_turn(HUMAN_STOP).await?;
    ThreadIdle::wait(&test.codex).await;
    let before = before.single_request();
    assert!(before.body_json().to_string().contains(PAIRED_OUTPUT));
    assert!(before.body_json().to_string().contains(LOOKALIKE_OUTPUT));
    let mut expected = delegated_outputs(&before, namespace, name);
    assert_eq!(expected.len(), 2);
    assert_eq!(expected[0]["output"], body(&assignment));
    assert_eq!(expected[1]["output"], body(CORRECTION));

    // Neither response echoes the task. A second boundary tests durable retention rather
    // than a one-time rescue from still-live admission state.
    for round in 0..2 {
        let compact_response = match mode {
            CompactionMode::LocalText => responses::sse(vec![
                responses::ev_assistant_message("summary", "OPAQUE_LOCAL_SUMMARY"),
                responses::ev_completed("compact"),
            ]),
            CompactionMode::RemoteV2 => responses::sse(vec![
                json!({"type": "response.output_item.done", "item": {
                    "type": "compaction", "encrypted_content": "OPAQUE_REMOTE_SUMMARY"
                }}),
                responses::ev_completed("compact"),
            ]),
        };
        let compact_mock = responses::mount_sse_once(&server, compact_response).await;
        test.codex.submit(Op::Compact).await?;
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
        ThreadIdle::wait(&test.codex).await;
        let compact_request = compact_mock.single_request();
        assert_eq!(
            compact_request.inputs_of_type("compaction_trigger").len(),
            match mode {
                CompactionMode::LocalText => 0,
                CompactionMode::RemoteV2 => 1,
            }
        );
        let followup = responses::mount_sse_once(&server, done()).await;
        test.submit_text_turn(&format!(
            "Continue only within the pause, checkpoint {round}."
        ))
        .await?;
        ThreadIdle::wait(&test.codex).await;
        let followup = followup.single_request();
        if round == 0 && matches!(delivery_size, DeliverySize::Oversized) {
            let retained = delegated_outputs(&followup, namespace, name);
            assert_eq!(retained.len(), 2);
            let shortened = retained[0]["output"]
                .as_str()
                .context("retained text body")?;
            assert!(shortened.contains(NEW_TASK));
            assert!(shortened.contains(OMISSION_NOTICE));
            assert!(!shortened.contains(OMITTED_MIDDLE));
            assert!(shortened.len() < body(&assignment).len());
            // All fixture content is ASCII text. Bound the visible payload and function
            // identity to 40k bytes (10k estimated tokens), not merely a response count.
            assert!(shortened.len() + name.len() + namespace.len() <= 40_000);
            let mut first = expected[0].clone();
            first["output"] = json!(shortened);
            assert_eq!(
                retained[0], first,
                "shortening must preserve delivery type and identity"
            );
            expected[0] = first;
        }
        assert_retained_request(&followup, namespace, name, &expected);
    }

    let rollout_path = test.codex.rollout_path().context("rollout path")?;
    test.codex.shutdown_and_wait().await?;
    sender.thread.shutdown_and_wait().await?;
    let items = std::fs::read_to_string(&rollout_path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(codex_rollout::parse_rollout_line)
        .map(|line| line.map(|line| line.item))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if history_mode == ThreadHistoryMode::PaginatedRefsV2 {
        assert!(items.iter().any(|item| matches!(
            item, RolloutItem::SessionMeta(meta) if meta.meta.history_mode == history_mode
        )));
        let entries = items
            .iter()
            .filter_map(|item| match item {
                RolloutItem::Compacted(compacted) => compacted.replacement_history_entries.as_ref(),
                _ => None,
            })
            .next_back()
            .context("reference-backed checkpoint")?;
        // A shortened assignment may legitimately be inline or reference its transformed
        // source. The unchanged correction must still exercise a digest-bound reference.
        let unchanged_ids: &[&str] = match delivery_size {
            DeliverySize::Fitting => &["new-assignment", "new-correction"],
            DeliverySize::Oversized => &["new-correction"],
        };
        for &id in unchanged_ids {
            let source = items
                .iter()
                .find_map(|item| match item {
                    RolloutItem::ResponseItem(envelope)
                        if envelope
                            .item
                            .id()
                            .is_some_and(|source_id| source_id.as_str() == id) =>
                    {
                        Some(envelope)
                    }
                    _ => None,
                })
                .context("original persisted delivery")?;
            let digest = CompactedHistoryDigest::from_envelope(source)?;
            assert!(
                entries.iter().any(|entry| matches!(
                    entry, CompactedHistoryEntry::ReferenceV2 { item_id, source_digest }
                        if item_id == id && source_digest == &digest
                )),
                "delivery {id} must be an actual digest-bound reference"
            );
        }
    }
    let materialized = codex_history::materialize_compacted_histories(&items);
    assert!(materialized.unresolved_item_ids.is_empty());
    let checkpoint = materialized
        .rollout_items
        .iter()
        .filter_map(|item| match item {
            RolloutItem::Compacted(compacted) => compacted.replacement_history.as_ref(),
            _ => None,
        })
        .next_back()
        .context("compacted replacement history")?;
    for delivery in &expected {
        let retained = checkpoint
            .iter()
            .find(|envelope| envelope.item.id().map(|id| id.as_str()) == delivery["id"].as_str())
            .context("materialized delegated delivery")?;
        let ResponseItem::FunctionCallOutput { output, .. } = &retained.item else {
            panic!("materialized delivery changed type");
        };
        assert_eq!(serde_json::to_value(output)?, delivery["output"]);
    }
    let retained_snapshots = checkpoint
        .iter()
        .filter_map(|item| {
            item.metadata
                .as_ref()?
                .sender_user_messages
                .as_deref()
                .cloned()
        })
        .collect::<Vec<_>>();
    assert_eq!(retained_snapshots, admitted_snapshots);

    // A new manager/store reopens the checkpoint through the public model-context route,
    // including paginated references. The original sender is no longer loaded here.
    let cold = builder(mode, history_mode)
        .with_home(Arc::clone(&test.home))
        .build_with_auto_env(&server)
        .await?;
    cold.codex.shutdown_and_wait().await?;
    let context = cold
        .thread_store
        .load_latest_model_context(LoadThreadHistoryParams {
            thread_id: test.session_configured.thread_id,
            include_archived: true,
        })
        .await?;
    let resumed = cold
        .thread_manager
        .resume_thread_with_history(
            cold.config.clone(),
            InitialHistory::Resumed(ResumedHistory {
                conversation_id: context.thread_id,
                history: Arc::new(context.items),
                history_revision: context.revision,
                rollout_path: Some(rollout_path),
            }),
            cold.thread_manager.auth_manager(),
            /*parent_trace*/ None,
            ClientMcpExtensions::default(),
        )
        .await?
        .thread;
    let cold_history = resumed.conversation_history_snapshot().await;
    assert_eq!(
        cold_history
            .retained_context()
            .and_then(|context| context.sender_user_messages()),
        admitted_snapshots.last()
    );
    let resume_mock = responses::mount_sse_once(&server, done()).await;
    resumed
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Report the current assignment and pause status.".to_owned(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_for_event(&resumed, |event| matches!(event, EventMsg::TurnComplete(_))).await;
    ThreadIdle::wait(&resumed).await;
    assert_retained_request(&resume_mock.single_request(), namespace, name, &expected);
    resumed.shutdown_and_wait().await?;
    Ok(())
}
