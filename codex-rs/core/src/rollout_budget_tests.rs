use super::*;
use pretty_assertions::assert_eq;

#[test]
fn rollback_rearms_only_its_thread_without_refunding_shared_usage() {
    let budget = RolloutBudget::default();
    budget.configure(RolloutBudgetConfig {
        limit_tokens: 100,
        reminder_at_remaining_tokens: vec![50],
        sampling_token_weight: 1.0,
        prefill_token_weight: 1.0,
    });
    let rolled_back_thread = ThreadId::new();
    let other_thread = ThreadId::new();
    let window = "unchanged-window";
    assert!(
        !budget
            .record_usage(&TokenUsage {
                output_tokens: 40,
                ..Default::default()
            })
            .unwrap()
    );
    for thread in [rolled_back_thread, other_thread] {
        let reminder = budget
            .pending_reminder(thread, window)
            .expect("initial reminder");
        budget.mark_reminder_delivered(thread, window, reminder);
        assert!(budget.pending_reminder(thread, window).is_none());
    }

    budget.rearm_reminder(rolled_back_thread);

    let reminder = budget
        .pending_reminder(rolled_back_thread, window)
        .expect("rollback should rearm the current reminder in the same window");
    assert_eq!(
        (reminder.remaining_tokens, reminder.reminder_index),
        (60, 0)
    );
    assert!(budget.pending_reminder(other_thread, window).is_none());
    budget.mark_reminder_delivered(rolled_back_thread, window, reminder);
    assert!(
        budget
            .pending_reminder(rolled_back_thread, window)
            .is_none()
    );

    // Rearming must not turn the already spent 40 tokens into new available budget.
    assert!(
        budget
            .record_usage(&TokenUsage {
                output_tokens: 60,
                ..Default::default()
            })
            .unwrap()
    );
    for thread in [rolled_back_thread, other_thread] {
        let reminder = budget
            .pending_reminder(thread, window)
            .expect("crossed threshold");
        assert_eq!((reminder.remaining_tokens, reminder.reminder_index), (0, 1));
    }
}
