//! Opt-in PostToolUse publication; ordinary and async history keep their lifetimes.

use super::Session;
use super::TurnContext;
use crate::context::ContextualUserFragment;
use crate::context::HookAdditionalContext;
use crate::state::ActiveTurn;
use crate::state::RunningTask;
use codex_history::ResponseItemEnvelope;
use codex_hooks::HookContextAffinity;
use codex_hooks::PostToolUseContext;
use codex_protocol::models::ResponseItem;
use std::sync::Arc;
use tokio::sync::Notify;

/// Captured with the originating step; delayed callbacks cannot adopt a later task.
#[derive(Clone)]
pub(crate) struct OriginatingTask {
    turn: Arc<TurnContext>,
    done: Arc<Notify>,
}

impl OriginatingTask {
    pub(crate) fn capture(task: &RunningTask, turn: &Arc<TurnContext>) -> Option<Self> {
        (Arc::ptr_eq(&task.turn_context, turn) && !task.cancellation_token.is_cancelled()).then(
            || Self {
                turn: Arc::clone(turn),
                done: Arc::clone(&task.done),
            },
        )
    }

    fn is_current(&self, active: Option<&ActiveTurn>) -> bool {
        active
            .and_then(|active| active.task.as_ref())
            .is_some_and(|task| {
                Arc::ptr_eq(&task.turn_context, &self.turn)
                    && Arc::ptr_eq(&task.done, &self.done)
                    && !task.cancellation_token.is_cancelled()
            })
    }
}

pub(super) enum HistoryPublication {
    Unrestricted,
    PostToolUse {
        target: Option<OriginatingTask>,
        affinities: Vec<HookContextAffinity>,
    },
}

impl HistoryPublication {
    pub(super) fn retain_current(
        &self,
        items: &mut Vec<ResponseItemEnvelope>,
        active: Option<&ActiveTurn>,
    ) {
        if let Self::PostToolUse { target, affinities } = self {
            // The private constructor below creates exactly one text envelope per affinity.
            assert_eq!(items.len(), affinities.len());
            if !target
                .as_ref()
                .is_some_and(|target| target.is_current(active))
            {
                let mut affinity = affinities.iter();
                items.retain(|_| affinity.next() == Some(&HookContextAffinity::Conversation));
                tracing::debug!(
                    "Skipped originating-task hook context after its task ended or changed"
                );
            }
        }
    }
}

impl Session {
    pub(crate) async fn record_post_tool_use_contexts(
        &self,
        turn: &Arc<TurnContext>,
        target: Option<OriginatingTask>,
        contexts: Vec<PostToolUseContext>,
    ) {
        if contexts.is_empty() {
            return;
        }
        let (items, affinities): (Vec<_>, Vec<_>) = contexts
            .into_iter()
            .map(|context| {
                let item: ResponseItem =
                    ContextualUserFragment::into(HookAdditionalContext::new(context.text));
                (item, context.affinity)
            })
            .unzip();
        let (items, image_preparations) = self
            .prepare_conversation_items_for_history(turn, turn.model_info(), &items)
            .await;
        let items = items
            .into_owned()
            .into_iter()
            .map(ResponseItemEnvelope::new)
            .collect();
        let publication = if affinities.contains(&HookContextAffinity::OriginatingTask) {
            HistoryPublication::PostToolUse { target, affinities }
        } else {
            HistoryPublication::Unrestricted
        };
        self.record_prepared_conversation_items(
            turn,
            turn.model_info(),
            items,
            image_preparations,
            publication,
        )
        .await;
    }
}

#[cfg(test)]
#[path = "hook_context_tests.rs"]
mod tests;
