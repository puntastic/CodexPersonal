use super::*;
use codex_protocol::openai_models::MODEL_SPECIALTY_CYBER;
use pretty_assertions::assert_eq;

#[derive(Clone, Copy)]
enum IssuingSpecialty {
    Standard,
    Cyber,
}

#[test_case::test_case(IssuingSpecialty::Standard; "standard_step_after_cyber_switch")]
#[test_case::test_case(IssuingSpecialty::Cyber; "cyber_step_after_standard_switch")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn denial_reporting_and_breaker_follow_the_issuing_model(
    issuing_specialty: IssuingSpecialty,
) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let request_log = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("issuing-model-review"),
            ev_assistant_message(
                "issuing-model-denial",
                r#"{"risk_level":"high","user_authorization":"low","outcome":"deny","rationale":"Denied test action."}"#,
            ),
            ev_completed("issuing-model-review"),
        ]),
    )
    .await;
    let (session, mut turn, events) = guardian_test_session_turn_and_rx(&server).await;
    let issuing_is_cyber = matches!(issuing_specialty, IssuingSpecialty::Cyber);
    update_turn_settings_for_test(Arc::get_mut(&mut turn).expect("unique turn"), |settings| {
        let model = Arc::make_mut(&mut settings.model_info);
        model.model_specialty = (!issuing_is_cyber).then(|| MODEL_SPECIALTY_CYBER.to_owned());
        model.model_messages = Some(
            serde_json::from_value(serde_json::json!({
                "auto_review": { "rejection_instructions": "Active thread rejection." }
            }))
            .unwrap(),
        );
    });
    let mut context = GuardianReviewContext::from(&turn);
    let model = Arc::make_mut(&mut context.model_info);
    model.slug = "issuing-model".to_owned();
    model.model_specialty = issuing_is_cyber.then(|| MODEL_SPECIALTY_CYBER.to_owned());
    model.model_messages = Some(
        serde_json::from_value(serde_json::json!({
            "auto_review": { "rejection_instructions": "Captured step rejection." }
        }))
        .unwrap(),
    );
    seed_guardian_parent_history(&session, &turn).await;
    let decision = review_approval_request(
        &session,
        context,
        "issuing-model-approval".to_owned(),
        guardian_exec_command_request("issuing-model-command"),
        ApprovalRequestReasons::default(),
    )
    .await;
    let ReviewDecision::Denied { rejection } = decision else {
        panic!("expected the explicit denial");
    };
    assert!(rejection.contains("Captured step rejection."));
    assert!(!rejection.contains("Active thread rejection."));
    assert_eq!(
        codex_guardian_reviewer::ReviewDenials::for_thread(
            &session.services.thread_extension_data,
        )
        .is_closed(&turn.sub_id)
        .await,
        issuing_is_cyber,
    );
    let mut model_contexts = Vec::new();
    while let Ok(event) = events.try_recv() {
        match event.msg {
            EventMsg::GuardianAssessment(event) => {
                model_contexts.push(
                    event
                        .model_context
                        .expect("captured model attribution")
                        .model_slug,
                );
            }
            EventMsg::TurnAborted(event) => panic!("denial interrupted the turn: {event:?}"),
            _ => {}
        }
    }
    assert_eq!(model_contexts, vec!["issuing-model", "issuing-model"]);
    assert_eq!(request_log.requests().len(), 1);
    Ok(())
}
