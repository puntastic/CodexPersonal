//! Retain host-admitted delegated inputs without turning them into user instructions.

use crate::context_manager::estimate_item_token_count;
use codex_history::CodexHarnessMetadata;
use codex_history::ResponseItemEnvelope;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ResponseItem;
use codex_utils_audio::estimate_audio_token_count;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::truncate_function_output_payload;

pub(crate) const MAX_RETAINED_DELEGATED_INPUT_TOKENS: usize = 10_000;
const OMITTED: &str =
    "[Delegated input shortened during compaction; omitted content is not retained.]";
// Shorter than OMITTED, so the existing marker reservation also covers this fallback.
const NON_TEXT_OMITTED: &str = "[Non-text content omitted during compaction.]";

pub(crate) fn is_admitted_delegated_input(
    item: &ResponseItem,
    metadata: Option<&CodexHarnessMetadata>,
) -> bool {
    let ResponseItem::FunctionCallOutput {
        id: Some(id),
        call_id: None,
        name: Some(name),
        namespace: Some(namespace),
        ..
    } = item
    else {
        return false;
    };
    matches!(
        (namespace.as_str(), name.as_str()),
        ("codex_app" | "codex_tui", "send_message_to_thread") | ("cloud_threads", "send_message")
    ) && metadata
        .and_then(|metadata| metadata.sender_user_messages.as_ref())
        .is_some_and(|snapshot| snapshot.receiver_message_id == id.as_str())
}

pub(crate) struct RetainedDelegatedInput {
    pub(crate) envelope: ResponseItemEnvelope,
    pub(crate) token_count: usize,
    // A per-item cap does not itself consume the remaining global budget.
    pub(crate) global_budget_exhausted: bool,
}

pub(crate) fn retain_delegated_input(
    item: &ResponseItem,
    metadata: Option<&CodexHarnessMetadata>,
    available_tokens: usize,
) -> Option<RetainedDelegatedInput> {
    if !is_admitted_delegated_input(item, metadata) {
        return None;
    }
    let original_tokens = usize::try_from(estimate_item_token_count(item)).unwrap_or(usize::MAX);
    let limit = available_tokens.min(MAX_RETAINED_DELEGATED_INPUT_TOKENS);
    let global_budget_exhausted =
        original_tokens.min(MAX_RETAINED_DELEGATED_INPUT_TOKENS) > available_tokens;
    let mut envelope = ResponseItemEnvelope {
        item: item.clone(),
        metadata: metadata.cloned(),
    };
    let shortened = original_tokens > limit;
    if shortened {
        let ResponseItem::FunctionCallOutput { output, .. } = &mut envelope.item else {
            unreachable!("admitted input must be a function output");
        };
        let mut original = output.clone();
        // Reserve the marker and function identity before asking the existing payload
        // truncator to allocate text/audio. Its image/encrypted preservation is not
        // a whole-item bound, so remeasure the finished item below.
        output.body = match &original.body {
            FunctionCallOutputBody::Text(_) => FunctionCallOutputBody::Text(OMITTED.to_owned()),
            FunctionCallOutputBody::ContentItems(_) => FunctionCallOutputBody::ContentItems(vec![
                FunctionCallOutputContentItem::InputText {
                    text: OMITTED.to_owned(),
                },
            ]),
        };
        let marker_tokens =
            usize::try_from(estimate_item_token_count(&envelope.item)).unwrap_or(usize::MAX);
        if marker_tokens > limit {
            tracing::debug!(
                original_tokens,
                available_tokens,
                "compaction omitted delegated input at budget boundary"
            );
            return None;
        }
        let mut marker_only = envelope.item.clone();
        let mut omission_marker = OMITTED;
        let mut payload_budget = limit.saturating_sub(marker_tokens).saturating_sub(/*rhs*/ 1);
        let mut previous_tokens = usize::MAX;
        loop {
            let ResponseItem::FunctionCallOutput { output, .. } = &mut envelope.item else {
                unreachable!("admitted input must be a function output");
            };
            *output = original.clone();
            truncate_function_output_payload(
                output,
                TruncationPolicy::Tokens(payload_budget),
                estimate_audio_token_count,
            );
            match &mut output.body {
                FunctionCallOutputBody::Text(text) => {
                    text.push('\n');
                    text.push_str(omission_marker);
                }
                FunctionCallOutputBody::ContentItems(items) => {
                    items.push(FunctionCallOutputContentItem::InputText {
                        text: omission_marker.to_owned(),
                    });
                }
            }
            let tokens =
                usize::try_from(estimate_item_token_count(&envelope.item)).unwrap_or(usize::MAX);
            if tokens <= limit {
                break;
            }
            if payload_budget == 0 || tokens >= previous_tokens {
                if let FunctionCallOutputBody::ContentItems(items) = &mut original.body
                    && items.iter().any(|item| {
                        !matches!(item, FunctionCallOutputContentItem::InputText { .. })
                    })
                {
                    // Unbudgeted media must not erase task text that still fits.
                    // Retry from the original text parts, in order, with a visible
                    // omission; this is not a new media selection policy.
                    items.retain(|item| {
                        matches!(item, FunctionCallOutputContentItem::InputText { .. })
                    });
                    omission_marker = NON_TEXT_OMITTED;
                    let ResponseItem::FunctionCallOutput { output, .. } = &mut marker_only else {
                        unreachable!("admitted input must be a function output");
                    };
                    output.body = FunctionCallOutputBody::ContentItems(vec![
                        FunctionCallOutputContentItem::InputText {
                            text: omission_marker.to_owned(),
                        },
                    ]);
                    payload_budget = limit.saturating_sub(marker_tokens).saturating_sub(/*rhs*/ 1);
                    previous_tokens = usize::MAX;
                    continue;
                }
                // No bounded payload remains, but retain its explicit omission.
                envelope.item = marker_only;
                break;
            }
            previous_tokens = tokens;
            payload_budget = payload_budget
                .saturating_sub(tokens - limit)
                .saturating_sub(/*rhs*/ 1);
        }
        if let Some(metadata) = &mut envelope.metadata {
            metadata.mark_retained_sources_incomplete();
        }
    }
    let token_count =
        usize::try_from(estimate_item_token_count(&envelope.item)).unwrap_or(usize::MAX);
    tracing::debug!(
        original_tokens,
        token_count,
        shortened,
        global_budget_exhausted,
        "compaction retained delegated input"
    );
    Some(RetainedDelegatedInput {
        envelope,
        token_count,
        global_budget_exhausted,
    })
}

#[cfg(test)]
#[path = "compact_delegated_input_tests.rs"]
mod tests;
