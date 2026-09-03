use std::sync::Arc;
use std::sync::PoisonError;

use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionWarning;
use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolExecutorFuture;
use codex_extension_api::ToolExposure;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::TurnAbortInput;
use codex_extension_api::TurnErrorInput;
use codex_extension_api::TurnLifecycleContributor;
use codex_extension_api::TurnStopInput;
use codex_extension_api::parse_tool_input_schema;
use serde::Deserialize;
use serde_json::json;

use super::Cue;
use super::CueActivationConfig;
use super::CueActivationMode;
use super::Extension;
use super::Runtime;

const REPORT_CUE_OUTCOME_TOOL_NAME: &str = "report_cue_outcome";

pub(super) const ASSESSMENT_INSTRUCTION: &str = "At natural release, call `report_cue_outcome` once before the final answer. Report only this cue's effect, source use, burden, and resulting outcome; self-report is not a correctness verdict.";

pub(super) fn pointer(cue: &Cue, handle: &str) -> String {
    const MAX_POINTER: usize = 1_120;
    const INTRO: &str = "Experimental advisory pointer: context only, not instruction, truth, authority, authorization, or proof. No source was hydrated or synchronized. You may ignore it.";
    const ACTION: &str = "If useful, inspect/hydrate that exact source; take at most one reversible probe/reframe, then release.";
    let tail = format!(
        "\nSource: {}\nHandle: {handle}\n{ACTION}\n{ASSESSMENT_INSTRUCTION}",
        cue.source,
    );
    let mut output = INTRO.to_string();
    optional(
        &mut output,
        "Cue: ",
        &cue.title,
        MAX_POINTER.saturating_sub(tail.len()),
    );
    output.push_str(&tail);
    optional(
        &mut output,
        "Discriminator: ",
        &cue.discriminator,
        MAX_POINTER,
    );
    optional(
        &mut output,
        "Release when: ",
        &cue.release_when,
        MAX_POINTER,
    );
    output
}

fn optional(output: &mut String, prefix: &str, value: &str, cap: usize) {
    let room = cap.saturating_sub(output.len());
    if room <= prefix.len() + 1 {
        return;
    }
    output.push('\n');
    output.push_str(prefix);
    let mut end = value.len().min(room - prefix.len() - 1);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&value[..end]);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingAssessment {
    pub(super) cue_id: String,
    pub(super) catalog_sha256: String,
}

#[derive(Clone, Debug)]
pub(super) struct AssessmentOffer {
    pub(super) turn_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum AssessmentEffect {
    Helpful,
    Redundant,
    Distracting,
    Unclear,
}

impl AssessmentEffect {
    fn as_str(self) -> &'static str {
        match self {
            Self::Helpful => "helpful",
            Self::Redundant => "redundant",
            Self::Distracting => "distracting",
            Self::Unclear => "unclear",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SourceConsulted {
    Yes,
    No,
    Uncertain,
}

impl SourceConsulted {
    fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum AssessmentBurden {
    None,
    Light,
    Material,
}

impl AssessmentBurden {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Light => "light",
            Self::Material => "material",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum AssessmentOutcome {
    Nothing,
    ConfirmedApproach,
    RedirectedAttention,
    ReframedApproach,
    ChangedAction,
    Other,
}

impl AssessmentOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::ConfirmedApproach => "confirmed_approach",
            Self::RedirectedAttention => "redirected_attention",
            Self::ReframedApproach => "reframed_approach",
            Self::ChangedAction => "changed_action",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CueAssessmentArgs {
    effect: AssessmentEffect,
    source_consulted: SourceConsulted,
    burden: AssessmentBurden,
    outcome: AssessmentOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CueAssessment {
    effect: AssessmentEffect,
    source_consulted: SourceConsulted,
    burden: AssessmentBurden,
    outcome: AssessmentOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecordedAssessment {
    pending: PendingAssessment,
    assessment: CueAssessment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AssessmentRecordError {
    NoPending,
}

impl Runtime {
    pub(super) fn offer_assessment(&self, turn_id: &str, cue_id: &str, catalog_sha256: &str) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.pending_assessments.insert(
            turn_id.to_string(),
            PendingAssessment {
                cue_id: cue_id.to_string(),
                catalog_sha256: catalog_sha256.to_string(),
            },
        );
    }

    pub(super) fn take_pending_assessment(&self, turn_id: &str) -> Option<PendingAssessment> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending_assessments
            .remove(turn_id)
    }

    fn record_assessment(
        &self,
        turn_id: &str,
        assessment: CueAssessment,
    ) -> Result<RecordedAssessment, AssessmentRecordError> {
        let pending = self
            .take_pending_assessment(turn_id)
            .ok_or(AssessmentRecordError::NoPending)?;
        Ok(RecordedAssessment {
            pending,
            assessment,
        })
    }
}

pub(super) struct CueAssessmentTool {
    runtime: Arc<Runtime>,
    thread_id: String,
}

impl CueAssessmentTool {
    pub(super) fn new(runtime: Arc<Runtime>, thread_id: String) -> Self {
        Self { runtime, thread_id }
    }

    async fn handle_call(
        &self,
        call: ToolCall<'_>,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let arguments = call.function_arguments()?;
        let arguments: CueAssessmentArgs = serde_json::from_str(arguments).map_err(|_| {
            tracing::warn!(
                event.name = "codex.cue_activation.assessment",
                thread_id = self.thread_id.as_str(),
                turn_id = call.turn_id.as_str(),
                status = "invalid",
                reason = "invalid_arguments",
                "cue activation assessment"
            );
            FunctionCallError::RespondToModel(
                "Invalid cue outcome arguments. Use only the documented enum values, then call once."
                    .to_string(),
            )
        })?;
        let assessment = CueAssessment {
            effect: arguments.effect,
            source_consulted: arguments.source_consulted,
            burden: arguments.burden,
            outcome: arguments.outcome,
        };
        let recorded = self
            .runtime
            .record_assessment(&call.turn_id, assessment)
            .map_err(|AssessmentRecordError::NoPending| {
                tracing::warn!(
                    event.name = "codex.cue_activation.assessment",
                    thread_id = self.thread_id.as_str(),
                    turn_id = call.turn_id.as_str(),
                    status = "unexpected",
                    reason = "no_pending_cue",
                    "cue activation assessment"
                );
                FunctionCallError::RespondToModel(
                    "No advisory cue outcome is pending for this turn; do not retry this tool."
                        .to_string(),
                )
            })?;
        tracing::info!(
            event.name = "codex.cue_activation.assessment",
            thread_id = self.thread_id.as_str(),
            turn_id = call.turn_id.as_str(),
            status = "recorded",
            cue_id = recorded.pending.cue_id.as_str(),
            catalog_sha256 = recorded.pending.catalog_sha256.as_str(),
            effect = recorded.assessment.effect.as_str(),
            source_consulted = recorded.assessment.source_consulted.as_str(),
            burden = recorded.assessment.burden.as_str(),
            outcome = recorded.assessment.outcome.as_str(),
            "cue activation assessment"
        );
        Ok(Box::new(JsonToolOutput::new(json!({
            "status": "recorded"
        }))))
    }
}

impl<C: Send + Sync + 'static> ToolContributor for Extension<C> {
    fn tools(
        &self,
        _: &ExtensionData,
        thread: &ExtensionData,
    ) -> Vec<Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>> {
        let (Some(config), Some(runtime)) =
            (thread.get::<CueActivationConfig>(), thread.get::<Runtime>())
        else {
            return Vec::new();
        };
        if config.mode != CueActivationMode::Advisory {
            return Vec::new();
        }
        vec![Arc::new(CueAssessmentTool::new(
            runtime,
            thread.level_id().to_string(),
        ))]
    }
}

impl<C: Send + Sync + 'static> TurnLifecycleContributor for Extension<C> {
    fn on_turn_stop<'a>(&'a self, input: TurnStopInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let Some(offer) = input.turn_store.remove::<AssessmentOffer>() else {
                return;
            };
            let Some(runtime) = input.thread_store.get::<Runtime>() else {
                return;
            };
            let Some(pending) = runtime.take_pending_assessment(&offer.turn_id) else {
                return;
            };
            tracing::warn!(
                event.name = "codex.cue_activation.assessment",
                thread_id = input.thread_store.level_id(),
                turn_id = offer.turn_id.as_str(),
                status = "missing",
                cue_id = pending.cue_id.as_str(),
                catalog_sha256 = pending.catalog_sha256.as_str(),
                "cue activation assessment"
            );
            self.event_sink.emit_warning(ExtensionWarning {
                thread_id: input.thread_store.level_id().to_string(),
                turn_id: Some(offer.turn_id.clone()),
                message: format!(
                    "Cue `{}` was offered, but its self-assessment was not recorded; its effect remains unknown.",
                    pending.cue_id
                ),
            });
        })
    }

    fn on_turn_abort<'a>(&'a self, input: TurnAbortInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            record_unavailable_assessment(input.thread_store, input.turn_store, "turn_aborted");
        })
    }

    fn on_turn_error<'a>(&'a self, input: TurnErrorInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            record_unavailable_assessment(input.thread_store, input.turn_store, "turn_error");
        })
    }
}

fn record_unavailable_assessment(
    thread: &ExtensionData,
    turn: &ExtensionData,
    status: &'static str,
) {
    let Some(offer) = turn.remove::<AssessmentOffer>() else {
        return;
    };
    let Some(runtime) = thread.get::<Runtime>() else {
        return;
    };
    let Some(pending) = runtime.take_pending_assessment(&offer.turn_id) else {
        return;
    };
    tracing::info!(
        event.name = "codex.cue_activation.assessment",
        thread_id = thread.level_id(),
        turn_id = offer.turn_id.as_str(),
        status,
        cue_id = pending.cue_id.as_str(),
        catalog_sha256 = pending.catalog_sha256.as_str(),
        "cue activation assessment"
    );
}

impl<'call> ToolExecutor<ToolCall<'call>> for CueAssessmentTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(REPORT_CUE_OUTCOME_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: REPORT_CUE_OUTCOME_TOOL_NAME.to_string(),
            description: "Record your first-person assessment of the advisory cue offered in this turn. Call exactly once after using or dismissing the cue and before the final answer. This reports perceived working effect, not objective correctness."
                .to_string(),
            strict: true,
            parameters: parse_tool_input_schema(&json!({
                "type": "object",
                "properties": {
                    "effect": {
                        "type": "string",
                        "enum": ["helpful", "redundant", "distracting", "unclear"],
                        "description": "Your immediate assessment of the cue's effect on this turn."
                    },
                    "source_consulted": {
                        "type": "string",
                        "enum": ["yes", "no", "uncertain"],
                        "description": "Whether you consulted the exact source named by the cue."
                    },
                    "burden": {
                        "type": "string",
                        "enum": ["none", "light", "material"],
                        "description": "How much extra work or distraction the cue caused."
                    },
                    "outcome": {
                        "type": "string",
                        "enum": ["nothing", "confirmed_approach", "redirected_attention", "reframed_approach", "changed_action", "other"],
                        "description": "The closest bounded description of what the cue changed."
                    }
                },
                "required": ["effect", "source_consulted", "burden", "outcome"],
                "additionalProperties": false
            }))
            .unwrap_or_else(|error| panic!("cue assessment input schema should parse: {error}")),
            output_schema: None,
            defer_loading: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::DirectModelOnly
    }

    fn handle<'a>(&'a self, call: ToolCall<'call>) -> ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(self.handle_call(call))
    }
}

#[cfg(test)]
#[path = "assessment_tests.rs"]
mod tests;
