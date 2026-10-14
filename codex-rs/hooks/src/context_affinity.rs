use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Applicability of synchronous PostToolUse additional context, not tool effects.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookContextAffinity {
    /// Preserve the historical behavior, including late conversation context.
    #[default]
    Conversation,
    /// Publish only while the task captured with the originating step is still current.
    OriginatingTask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostToolUseContext {
    pub text: String,
    pub affinity: HookContextAffinity,
}

impl crate::output_spill::HookOutputSpiller {
    pub(crate) async fn spill_post_tool_use_contexts(
        &self,
        contexts: Vec<(crate::output_spill::AdditionalContext, HookContextAffinity)>,
    ) -> Vec<PostToolUseContext> {
        let mut spilled = Vec::with_capacity(contexts.len());
        for (context, affinity) in contexts {
            let text = self
                .maybe_spill_text_with_limit(context.text, context.limit)
                .await;
            spilled.push(PostToolUseContext { text, affinity });
        }
        spilled
    }
}
