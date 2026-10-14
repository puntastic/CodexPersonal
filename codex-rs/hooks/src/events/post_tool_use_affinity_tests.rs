use super::tests::handler_with_async;
use super::tests::run_result;
use super::*;
use crate::output_spill::AdditionalContextLimit;
use crate::output_spill::HookOutputSpiller;
use pretty_assertions::assert_eq;

#[test]
fn affinity_is_explicit_and_async_cannot_erase_its_contract() {
    for (affinity, asynchronous, expected) in [
        (None, false, Some(HookContextAffinity::Conversation)),
        (None, true, Some(HookContextAffinity::Conversation)),
        (
            Some("originating_task"),
            false,
            Some(HookContextAffinity::OriginatingTask),
        ),
        (Some("originating_task"), true, None),
        (Some("unknown"), false, None),
    ] {
        let mut specific =
            serde_json::json!({"hookEventName":"PostToolUse", "additionalContext":"candidate"});
        if let Some(affinity) = affinity {
            specific["additionalContextAffinity"] = affinity.into();
        }
        let output = serde_json::json!({"hookSpecificOutput":specific}).to_string();
        let parsed = parse_completed(
            &handler_with_async(asynchronous),
            run_result(Some(0), &output, ""),
            Some("old-turn".into()),
        );
        let actual = parsed
            .data
            .additional_contexts_for_model
            .first()
            .map(|_| parsed.data.context_affinity);
        assert_eq!(actual, expected);
        assert_eq!(
            parsed.completed.run.status,
            if expected.is_some() {
                HookRunStatus::Completed
            } else {
                HookRunStatus::Failed
            }
        );
        assert_eq!(
            parsed
                .completed
                .run
                .entries
                .iter()
                .any(|entry| entry.kind == HookOutputEntryKind::Context),
            expected.is_some()
        );
    }
}

#[tokio::test]
async fn mixed_affinities_survive_ordered_spilling() {
    let spiller = HookOutputSpiller::new(ThreadId::new());
    let large = "source-bound detail ".repeat(1_000);
    let contexts = vec![
        ("before".into(), HookContextAffinity::Conversation),
        (large.clone(), HookContextAffinity::OriginatingTask),
        ("after".into(), HookContextAffinity::Conversation),
    ];
    let output = spiller
        .spill_post_tool_use_contexts(
            contexts
                .into_iter()
                .map(|(text, affinity)| {
                    (
                        AdditionalContext {
                            text,
                            limit: AdditionalContextLimit::from_config(Some(20)),
                        },
                        affinity,
                    )
                })
                .collect(),
        )
        .await;
    assert_eq!(
        output
            .iter()
            .map(|value| value.affinity)
            .collect::<Vec<_>>(),
        vec![
            HookContextAffinity::Conversation,
            HookContextAffinity::OriginatingTask,
            HookContextAffinity::Conversation,
        ]
    );
    assert_eq!(
        (&output[0].text, &output[2].text),
        (&"before".to_string(), &"after".to_string())
    );
    let path = output[1]
        .text
        .lines()
        .find_map(|line| line.strip_prefix("Full hook output saved to: "))
        .expect("spilled path");
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), large);
    tokio::fs::remove_file(path).await.unwrap();
}
