use super::*;
use codex_models_manager::model_info::model_info_from_slug;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn protected_exec_rejection_uses_guardian_source_and_captured_turn_state() {
    let store = codex_extension_api::ExtensionData::new("protected-exec-test");
    let model = model_info_from_slug("acting-model");
    let denials = codex_guardian_reviewer::ReviewDenials::for_thread(&store);
    for _ in 0..3 {
        let _ = denials.record_denial("closed-turn", &model).await;
    }
    assert!(denials.is_closed("closed-turn").await);
    let rejection = format!(
        "Configured feedback.\n{}",
        codex_guardian_reviewer::APPROVAL_LANE_CLOSED_INSTRUCTIONS,
    );
    for (source, captured_turn_id, protected) in [
        (ApprovalResolutionSource::Guardian, "closed-turn", true),
        (ApprovalResolutionSource::Guardian, "different-turn", false),
        (ApprovalResolutionSource::Hook, "closed-turn", false),
        (ApprovalResolutionSource::User, "closed-turn", false),
    ] {
        let result = ApprovalResolution {
            decision: ReviewDecision::denied(rejection.clone()),
            source,
        }
        .into_exec_tool_result(&model, &store, captured_turn_id)
        .await;
        if protected {
            assert!(matches!(
                result,
                Err(ToolError::RejectedWithApprovalLaneClosed(message)) if message == rejection
            ));
        } else {
            assert!(matches!(
                result,
                Err(ToolError::Rejected(message)) if message == rejection
            ));
        }
    }

    // An older closed turn cannot close a new turn, and clearing that old turn
    // cannot borrow a concurrently closed turn's state for a late result.
    codex_guardian_reviewer::ReviewDenials::clear_turn(&store, "closed-turn").await;
    for _ in 0..3 {
        let _ = denials.record_denial("different-turn", &model).await;
    }
    let result = ApprovalResolution {
        decision: ReviewDecision::denied(rejection.clone()),
        source: ApprovalResolutionSource::Guardian,
    }
    .into_exec_tool_result(&model, &store, "closed-turn")
    .await;
    assert!(matches!(result, Err(ToolError::Rejected(message)) if message == rejection));
}

#[tokio::test]
async fn approved_and_non_exec_decisions_keep_their_existing_treatment() {
    let store = codex_extension_api::ExtensionData::new("ordinary-approval-test");
    let model = model_info_from_slug("acting-model");
    assert_eq!(
        ApprovalResolution {
            decision: ReviewDecision::Approved,
            source: ApprovalResolutionSource::Guardian,
        }
        .into_exec_tool_result(&model, &store, "full-access-turn")
        .await
        .expect("approved exec remains approved"),
        ReviewDecision::Approved,
    );
    assert!(
        store
            .get::<codex_guardian_reviewer::ReviewDenials>()
            .is_none()
    );

    let resolution = ApprovalResolution {
        decision: ReviewDecision::denied("unchanged non-exec rejection"),
        source: ApprovalResolutionSource::Guardian,
    };
    assert!(matches!(
        resolution.into_tool_result(&model),
        Err(ToolError::Rejected(message)) if message == "unchanged non-exec rejection"
    ));
}
