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
    /// Evidence captured for one completed assessment, never reused by another attempt.
    type Evidence: Send;
    /// Returns the target's rules, or no evidence when they cannot be resolved.
    fn permissions(&self) -> Option<codex_guardian_context::PermissionContext>;
    /// Returns the owning turn and optional target item after validating the action.
    fn validate_action(&self) -> Result<(&str, Option<&str>), ReviewDecision>;
    fn prepare(
        &self,
        approval_id: &str,
        reason: GuardianReviewReason,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> impl Future<Output = Result<(Self::Prepared, ReviewReport), ReviewDecision>> + Send;
    /// Captures fresh authorization evidence and rejects stale approvals for each attempt.
    fn attempt(
        &self,
        prepared: &Self::Prepared,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> impl Future<
        Output = (
            GuardianReviewOutcome,
            GuardianReviewAnalyticsResult,
            Option<Self::Evidence>,
        ),
    > + Send;
    /// Revalidates the original action and history lifetime before using cached approval.
    fn cached_approval_is_current(
        &self,
        prepared: &Self::Prepared,
    ) -> impl Future<Output = bool> + Send;
    fn emit(&self, event: EventMsg) -> impl Future<Output = ()> + Send;
    fn record_evidence(
        &self,
        prepared: &Self::Prepared,
        evidence: Self::Evidence,
        event: &GuardianAssessmentEvent,
    ) -> impl Future<Output = ()> + Send;
}

impl<H: ReviewHost> SynchronousApprovalReviewer for ReviewRequest<'_, H> {
    #[tracing::instrument(
        name = "guardian_review",
        level = "debug",
        skip_all,
        fields(
            review_id = self.approval_id,
            thread_id = %self.thread_id,
            tool_call_id = self.tool_call_id,
        )
    )]
    fn review<'a>(
        &'a self,
        reason: GuardianReviewReason,
        async_approval: Option<ExtensionFuture<'a, ()>>,
    ) -> ExtensionFuture<'a, Option<ReviewDecision>> {
        Box::pin(async move {
            let deadline = Instant::now() + crate::REVIEW_TIMEOUT;
            let (prepared, report) = match self
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
            let cancellation = self.cancellation.child_token();
            let mut review = Box::pin(crate::run_with_retry(
                GuardianReviewSessionLimits {
                    max_attempts: crate::MAX_REVIEW_ATTEMPTS,
                    deadline,
                },
                Some(&cancellation),
                |deadline| self.host.attempt(&prepared, deadline, &cancellation),
            ));
            let (outcome, analytics, evidence) = if let Some(async_approval) = async_approval {
                tokio::select! {
                    biased;
                    result = &mut review => result,
                    () = async_approval => {
                        // The async approval won. Finish cancellation through the existing
                        // reviewer cleanup; the retry loop is never restarted.
                        cancellation.cancel();
                        let (_, analytics, _) = review.await;
                        let outcome = if !self.host.cached_approval_is_current(&prepared).await
                            || self.cancellation.is_cancelled() {
                            GuardianReviewOutcome::Error(GuardianReviewError::Cancelled)
                        } else if Instant::now() >= deadline {
                            GuardianReviewOutcome::Error(GuardianReviewError::Timeout)
                        } else {
                            GuardianReviewOutcome::CachedApproval
                        };
                        (outcome, analytics, None)
                    }
                }
            } else {
                review.await
            };
            let completed_at_ms = codex_analytics::now_unix_millis();
            let cached_approval = matches!(outcome, GuardianReviewOutcome::CachedApproval);
            let mut completed = report.complete(
                outcome,
                self.model,
                self.require_guardian,
                analytics,
                completed_at_ms.try_into().unwrap_or_default(),
            );
            if self.log_assessments && !cached_approval {
                self.telemetry
                    .guardian_assessment(&completed.event, completed.assessment_outcome);
            }
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
            if completed.assessment_outcome.is_some()
                && let Some(evidence) = evidence
            {
                self.host
                    .record_evidence(&prepared, evidence, &completed.event)
                    .await;
            }
            let action_class = crate::reporting::action_class(&completed.event.action);
            let risk = completed.event.risk_level;
            let authorization = completed.event.user_authorization;
            self.host
                .emit(EventMsg::GuardianAssessment(completed.event))
                .await;
            if cached_approval {
                return Some(self.cached_approval().await);
            }
            let denials = ReviewDenials::for_thread(self.thread_store);
            if completed.assessment_outcome == Some(GuardianAssessmentOutcome::Deny) {
                if let Some(message) = denials.record_denial(self.turn_id, self.model).await {
                    self.telemetry.counter(
                        "codex.guardian.denial_limit_reached",
                        /*inc*/ 1,
                        &[],
                    );
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
