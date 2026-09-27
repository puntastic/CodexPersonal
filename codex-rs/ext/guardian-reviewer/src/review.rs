//! Owns synchronous review orchestration, reporting and outcome accounting.
//! The host binds the original action, captures evidence and enforces authorization.

use crate::GuardianReviewError;
use crate::GuardianReviewOutcome;
use crate::GuardianReviewSessionLimits;
use crate::ReviewDenials;
use crate::ReviewReport;
use crate::ReviewRequest;
use codex_analytics::GuardianReviewAnalyticsResult;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::SynchronousApprovalReviewer;
use codex_protocol::approvals::GuardianReviewReason;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::GuardianAssessmentEvent;
use codex_protocol::protocol::GuardianAssessmentOutcome;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::WarningEvent;
use std::future::Future;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Operations bound to one immutable action and its issuing context.
/// Hosts validate authority and publish supplied events;
/// they do not select Guardian outcomes, retry policy or reporting effects.
pub trait ReviewHost: Send + Sync {
    type Prepared: Send + Sync;
    /// Returns the owning turn and optional target item after validating the action.
    fn validate_action(&self) -> Result<(&str, Option<&str>), ReviewDecision>;
    fn prepare(
        &self,
        approval_id: &str,
        reason: GuardianReviewReason,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> impl Future<Output = Result<(Self::Prepared, ReviewReport), ReviewDecision>> + Send;
    /// Rejects stale approvals before returning the attempt's outcome.
    fn attempt(
        &self,
        prepared: &Self::Prepared,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> impl Future<Output = (GuardianReviewOutcome, GuardianReviewAnalyticsResult)> + Send;
    fn emit(&self, event: EventMsg) -> impl Future<Output = ()> + Send;
    fn record_evidence(
        &self,
        prepared: &Self::Prepared,
        event: &GuardianAssessmentEvent,
    ) -> impl Future<Output = ()> + Send;
}

impl<H: ReviewHost> SynchronousApprovalReviewer for ReviewRequest<'_, H> {
    fn review(&self, reason: GuardianReviewReason) -> ExtensionFuture<'_, Option<ReviewDecision>> {
        Box::pin(async move {
            let deadline = Instant::now() + crate::REVIEW_TIMEOUT;
            let (context, report) = match self
                .host
                .prepare(self.approval_id, reason, deadline, &self.cancellation)
                .await
            {
                Ok(prepared) => prepared,
                Err(decision) => return Some(decision),
            };
            self.host
                .emit(EventMsg::GuardianAssessment(report.started_event()))
                .await;
            let (outcome, analytics) = if self.cancellation.is_cancelled() {
                (
                    GuardianReviewOutcome::Error(GuardianReviewError::Cancelled),
                    GuardianReviewAnalyticsResult::without_session(),
                )
            } else {
                Box::pin(crate::run_with_retry(
                    GuardianReviewSessionLimits {
                        max_attempts: crate::MAX_REVIEW_ATTEMPTS,
                        deadline,
                    },
                    Some(&self.cancellation),
                    |deadline| self.host.attempt(&context, deadline, &self.cancellation),
                ))
                .await
            };
            let completed_at_ms = codex_analytics::now_unix_millis();
            let mut completed = report.complete(
                outcome,
                self.model,
                self.require_guardian,
                analytics,
                completed_at_ms.try_into().unwrap_or_default(),
            );
            report.track(
                self.telemetry,
                self.analytics,
                completed.analytics,
                completed_at_ms,
            );
            if let Some(message) = completed.warning {
                self.host
                    .emit(EventMsg::GuardianWarning(WarningEvent { message }))
                    .await;
            }
            if completed.assessment_outcome.is_some() {
                self.host.record_evidence(&context, &completed.event).await;
            }
            let action_class = crate::reporting::action_class(&completed.event.action);
            let risk = completed.event.risk_level;
            let authorization = completed.event.user_authorization;
            self.host
                .emit(EventMsg::GuardianAssessment(completed.event))
                .await;
            let denials = ReviewDenials::for_thread(self.thread_store);
            if completed.assessment_outcome == Some(GuardianAssessmentOutcome::Deny) {
                if let Some(message) = denials.record_denial(self.turn_id, self.model).await {
                    let risk = risk.map_or_else(String::new, |risk| {
                        serde_json::to_value(risk)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    });
                    let authorization = authorization.map_or_else(String::new, |authorization| {
                        serde_json::to_value(authorization)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    });
                    self.host
                        .emit(EventMsg::Warning(WarningEvent {
                            message: format!(
                                "{message} Latest denied action: {action_class}. Review category: {risk} risk, {authorization} authorization; detailed rationale is available in the preceding automatic-review result."
                            ),
                        }))
                        .await;
                }
                if denials.is_closed(self.turn_id).await
                    && let Some(ReviewDecision::Denied { rejection }) = &mut completed.decision
                {
                    rejection.push('\n');
                    rejection.push_str(crate::reporting::APPROVAL_LANE_CLOSED_INSTRUCTIONS);
                }
            } else {
                denials.record_non_denial(self.turn_id).await;
            }
            completed.decision
        })
    }
}
