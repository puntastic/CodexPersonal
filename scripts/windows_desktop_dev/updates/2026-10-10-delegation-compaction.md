# Delegated-input retention and cold recovery

Source base: `23b4de45e246551cf63e4621d743b8efd7f44954` (0.161.0).
Purpose branch: `codex/delegation-compaction-retention-20261010`.
This is a downstream repair, not a general upstream synchronization.

## Problem and bounded improvement

Native local and remote compaction retained ordinary user messages but omitted
modern standalone inter-thread deliveries represented as function outputs.
That could leave an older explicit task visible while a newer assignment or
correction survived, at best, through an opaque summary. The repair retains
host-admitted deliveries in their original type, order and provenance; it does
not promote another instance's text into human authorization.

Admission requires an exact supported delivery route, an unpaired output with
an ID, and host-owned sender metadata bound to that receiver message ID.
Ordinary tool outputs and convincing quoted delegation tags do not qualify.
Historical deliveries without that metadata are outside this guarantee.
The recognized routes are `codex_app/send_message_to_thread`,
`codex_tui/send_message_to_thread`, and `cloud_threads/send_message`.
Testing those route shapes does not deploy this fork to another cloud host.

The shared helper keeps a fitting envelope intact. Oversized deliveries use
the existing truncator, an explicit omission notice and incomplete-source
marking. Each retained item is capped at 10,000 estimated tokens, inside the
existing 64,000 remote / 20,000 local retained-input budgets. These are not
tokenizer-exact or whole-request limits. A global-budget boundary stops older
backfill; a per-item cap does not incorrectly consume the entire global budget.
Oversized non-text content cannot erase affordable neighboring task text.
Canonical initial context also recognizes a delegated delivery as an input
placement boundary. Diagnostics contain counts, not payload text.

Manual P0 review of items exceeding 1,000 tokens accepted the bounded cost:
this preserves existing task input rather than injecting a new authority or
unbounded transcript. Omission notices are payload-local truncation annotations,
following existing output-truncator practice, not new developer/user fragments.
Transient fields continue to follow the existing serialization contract.

## Additional recovery defect exposed by the regression

The oversized integration case passed repeated live compaction and whole-log
materialization, but initially failed cold loading. Its shortened model copy
and original Guardian evidence had the same ID. Selected source lookup and the
bounded reverse scanner could mistakenly use the model rewrite for the Guardian
reference, unlike full chronological materialization.

The repair preserves a Guardian reference's pre-checkpoint source boundary in
the selected and forward source-index paths. The bounded scanner carries that
boundary only when needed to protect a retained same-ID model source. It does
not accumulate unrelated reference-only checkpoints. A 128-checkpoint test
checks constant retained size for that case.

Model and Guardian sources remain separate. A genuinely newer substituted
source still fails its digest check: there is no backward search for an older
matching digest, weaker validation, or new approval policy. Missing originals
remain failures. Existing serialized forms and public APIs are unchanged.

## Verification and retained failures

Final distinct executed checks: **675 passed**, with one core snapshot case
requiring the existing retry mechanism:

- 247 complete `codex-history` / `codex-rollout` library tests:
  `20261009-184808-Just-a25a1b86.junit.xml`.
- 418 selected `codex-core` library/integration tests:
  `20261009-184933-Just-cd02c367.junit.xml`.
  `production_turn_keeps_rebalanced_catalogs_stable_after_compaction_and_resume`
  had a tool-catalog timing/snapshot difference on its first attempt and passed
  on retry. This is a retained flaky qualification, not a clean first pass or
  an established cause of the timing variation.
- 10 selected `codex-thread-store` Guardian/migration/rollback tests:
  `20261009-190640-Just-ebba4389.junit.xml`.

The six new integration variants exercise local/remote compaction, all three
delivery route shapes, actual V2 references, oversized transformation, and the
default-disabled Guardian-context mode. They check an old task, a newer
assignment, a correction and a later human pause in order; two compactions;
persisted source identity; and outbound input after reopening with a new
manager. Mock model responses do not repeat the task. These are software
integration checks, not evidence of real-model obedience or a live chat trial.

Useful retained red evidence:

- Initial 413-case run: 401 passed / 12 failed. Six were new fixture mistakes;
  the other six reproduced on the unchanged base, along with two sibling cases.
  Baseline receipt: `20261009-174525-Just-aa1fb958.junit.xml` (2/10 passed).
  Fixtures now select their intended Guardian modes explicitly; production
  defaults and migration assertions are unchanged. The two refreshed tool
  snapshots were byte-identical between baseline and candidate output.
- A later 418-case run reached 417 passes and the genuine oversized cold-load
  failure: `20261009-181310-Just-8df5a420.junit.xml`.
- Two smaller same-ID model/Guardian regressions both failed before the recovery
  fix: `20261009-182944-Just-c728bcf7.junit.xml`. They now pass alongside missing
  and substituted-source controls.
- The first combined thread-store filter matched zero cases. The separate
  ten-case execution above uses the actual test names; compilation alone was
  not counted as coverage.

Commands use the repository's `just test` route, locked dependencies, one build
job and `CARGO_INCREMENTAL=0`. The full workspace suite was not run: repository
policy requires separate user approval. No live chat was forcibly compacted.

The change was split into implementation/unit, integration, fixture-qualification
and source-recovery commits for review. Its combined size exceeds the ordinary
800-line guidance, predominantly because of regression fixtures. Splitting does
not exempt it from review; the implementation and integration checks form one
release cohort. The shared primitive belongs to the existing core compaction
owner; source resolution stays in history/rollout, without a new crate or API.

## Package and adoption boundary

Scoped `just fix -p codex-core -p codex-history -p codex-rollout --locked`
completed successfully (`20261009-190752-Just-8fbeeef4.log`), followed by
repository `just fmt` (`20261009-191525-Just-0bd5e444.log`). No tests were
repeated merely for formatting. Canonical candidate packaging is pending at
this source checkpoint. Installation, selector changes and restart are separate.
The Windows development-lane self-test also passed all three groups
(`20261009-191643-SelfTest-f28d4b36.log`); its deployment exercises use disposable
test settings, not the live selector.
No approval, model, context-mode or hook setting is changed by this repair.

It cannot retroactively reconstruct a task already omitted from a compacted
working history; affected work needs an explicit fresh handoff. Also, executable
rollback is not history rollback. The older reader has the reproduced source
boundary defect and must not be assumed to reopen every later checkpoint merely
because the wire schema is unchanged. Preserve the old package and appropriate
history recovery copies when separately qualifying adoption.
