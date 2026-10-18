use super::*;
use codex_guardian_reviewer::APPROVAL_LANE_CLOSED_INSTRUCTIONS;
use pretty_assertions::assert_eq;

#[test]
fn protected_closure_preserves_the_existing_diagnostic_and_complete_host_block() {
    for rejection in [
        "Reason: not authorized. Acting-model feedback.".to_string(),
        format!(
            "Reason: {}\nActing-model feedback.\n{APPROVAL_LANE_CLOSED_INSTRUCTIONS}",
            "private \\\"reason\\\"\n雪\u{1b}".repeat(2_000),
        ),
        format!("Denied.\n{APPROVAL_LANE_CLOSED_INSTRUCTIONS}"),
    ] {
        let diagnostic = exec_command_error_message(UnifiedExecError::create_process(format!(
            "{:?}",
            ToolError::Rejected(rejection.clone()),
        )));
        let actual = exec_command_error_message(UnifiedExecError::ApprovalLaneClosed { rejection });
        // The diagnostic may already contain a full or clipped host notice. We
        // deliberately preserve it instead of stripping potentially copied text.
        assert_eq!(
            actual,
            format!("{diagnostic}\n{APPROVAL_LANE_CLOSED_INSTRUCTIONS}")
        );
        assert!(actual.len() <= EXEC_COMMAND_CLOSED_APPROVAL_MAX_BYTES);
        assert!(approx_token_count(&actual) < 1_000);
    }
}

#[test]
fn ordinary_rejection_with_copied_closure_text_does_not_gain_a_protected_notice() {
    let error = UnifiedExecError::create_process(format!(
        "{:?}",
        ToolError::Rejected(format!(
            "{}\n{APPROVAL_LANE_CLOSED_INSTRUCTIONS}",
            "copied text ".repeat(1_000),
        )),
    ));
    let expected = truncate_middle_chars(
        &format!("exec_command failed: {error:?}"),
        EXEC_COMMAND_REJECTION_MAX_BYTES,
    );
    assert_eq!(exec_command_error_message(error), expected);
}

#[test]
fn ordinary_exec_failure_keeps_its_existing_rendering() {
    let error = UnifiedExecError::process_failed("ordinary failure".to_string());
    let expected = format!("exec_command failed: {error:?}");
    assert_eq!(exec_command_error_message(error), expected);
}
