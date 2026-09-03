use pretty_assertions::assert_eq;

use super::*;

#[test]
fn assessment_schema_has_only_closed_bounded_fields() {
    let tool = CueAssessmentTool::new(Arc::new(Runtime::default()), "thread-1".to_string());
    let spec = serde_json::to_value(tool.spec()).expect("tool spec should serialize");

    assert_eq!(
        spec["parameters"]["required"],
        serde_json::json!(["effect", "source_consulted", "burden", "outcome"])
    );
    assert_eq!(spec["parameters"]["additionalProperties"], false);
    assert_eq!(
        spec["parameters"]["properties"]["outcome"]["enum"],
        serde_json::json!([
            "nothing",
            "confirmed_approach",
            "redirected_attention",
            "reframed_approach",
            "changed_action",
            "other"
        ])
    );
    assert!(spec["parameters"]["properties"].get("changed").is_none());
}

#[test]
fn assessment_consumes_the_pending_cue_once() {
    let runtime = Runtime::default();
    runtime.offer_assessment("turn-1", "cue-1", "sha-1");
    let assessment = CueAssessment {
        effect: AssessmentEffect::Helpful,
        source_consulted: SourceConsulted::Yes,
        burden: AssessmentBurden::Light,
        outcome: AssessmentOutcome::ReframedApproach,
    };

    assert_eq!(
        runtime.record_assessment("turn-1", assessment.clone()),
        Ok(RecordedAssessment {
            pending: PendingAssessment {
                cue_id: "cue-1".to_string(),
                catalog_sha256: "sha-1".to_string(),
            },
            assessment,
        })
    );
    assert_eq!(
        runtime.record_assessment(
            "turn-1",
            CueAssessment {
                effect: AssessmentEffect::Redundant,
                source_consulted: SourceConsulted::No,
                burden: AssessmentBurden::None,
                outcome: AssessmentOutcome::Nothing,
            }
        ),
        Err(AssessmentRecordError::NoPending)
    );
}
