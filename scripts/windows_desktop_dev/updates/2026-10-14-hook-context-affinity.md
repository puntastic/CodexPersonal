# Opt-in originating-task PostToolUse context — source candidate

Based on `c532df776a55a3655ae1fe07ebfc78ad809797a7`, in the isolated
`codex/hook-context-affinity-20261014` branch of `puntastic/CodexPersonal`.
No installed/deployed binary, saved configuration or feature default changes.

The mock-only native event-join cancellation case `cancel-at-publication-003`
retained the old task's packet in the next request (counts 0, 0, 1). This is an
origin-bound capability failure, not proof that legacy historical context or
all asynchronous hooks should be suppressed. Its original outputs remain intact
in the separate Athenaeum experiment worktree; this candidate does not regrade them.

Synchronous PostToolUse output may now request
`hookSpecificOutput.additionalContextAffinity: "originating_task"` beside
`additionalContext`. Omission or `"conversation"` preserves legacy lifetime.
Unknown values fail parsing. Async handlers requesting originating-task affinity
produce a failed hook diagnostic and no Context, rather than losing the qualifier.
Other hook events, block/feedback decisions and actual tool effects stay unchanged.

Affinity travels with each fragment through ordered aggregation and spilling.
Step capture retains the matching running task's context and per-task completion
identity before step-preparation waits. The immutable StepContext carries it
through queued tools and Code Mode cells; registry dispatch never recaptures a
currently active replacement. At publication,
only opted-in fragments are filtered if that task is unavailable, cancelled or
replaced. The history append and final comparison share active-turn then history
locks; persistence and notifications occur after releasing both. Same turn IDs
and even the same context object do not substitute for the captured task identity.

The private core module is justified by ownership: only Session owns the running
task and history locks. Hooks owns the wire/affinity carrier. Generic history
remains unrestricted; the existing preparation, IDs, truncation, attribution and
persistence path is reused. This is neither a general authorization check nor a
promise that arbitrary external source/access state cannot change after admission.

At the original source handoff only `git diff --check` had run. The parent then
used the existing Windows Rust1.95/MSVC toolchain, two Cargo jobs and disabled
incremental compilation, respecting the 80 GiB free-space floor:

- `just write-hooks-schema` passed after repairing a private enum-import path.
  The generated PostToolUse output schema is included, not hand-edited.
- `just fmt` passed; only the intended source files changed.
- `just fix -p codex-hooks -p codex-core --profile dev-small` completed. It
  reported an unfulfilled production lint expectation and the deliberately
  held history lock in the race test. The expectation was moved to that test,
  with its exact reason, and formatting rerun before tests.
- `just test -p codex-hooks -p codex-core --lib --cargo-profile dev-small -E
  'package(codex-hooks) or test(session::hook_context::tests)'` passed:
  186 tests, 2705 filtered out, nextest run
  `c2e89de8-6164-45b5-888c-981e225d8b44`.

This is focused Windows evidence, not the full workspace suite or the native
end-to-end probe. No package has been activated, configuration changed or model
trial run. A candidate package and separately bound mock lifecycle run are next.

New cases cover parsing/default/async refusal, ordered mixed-affinity spilling,
current/cancelled/finished/replaced/same-context-new-task publication, history-lock
contention, and native Code Mode held-hook cancellation with a legacy control.
The native integration test inherits the existing hooks suite's non-Windows
boundary and was not executed on Windows. The seven new core unit cases and
the hooks crate tests are included in the passing selection above. Separately
bound Windows mock-host runs are still unrun.

Independent source review caught a candidate-only edge before build: capturing at
registry dispatch could adopt a replacement that reused the same TurnContext
before a delayed callback arrived. Capture was moved into the existing locked
step snapshot, and a before-dispatch same-context replacement regression added.
This was not an observed production leak; the retained native003 failure remains
the separately observed old-turn publication. No new execution was performed.

## Native and Linux follow-on

The stable Windows package from759890dfc7 subsequently passed all four bounded
mock-native paths: active delivery, cancelled-task withholding, after-turn
withholding, and unmarked historical-context retention. No real model call or
Desktop activation occurred. The Athenaeum native-event-join-v0 RESULTS-R4.md
preserves exact inputs, outputs and the source/spec-only affinity custody limit.

Rigger's separate Linux attempt at759890dfc7, reviewed by Fae in Ath2010r1,
failed compilation before any requested case ran. An explicit child-module
pretty_assertions::assert_eq import repairs the new test's reported macro
ambiguity. The enclosing integration target also exposed an inherited test's
two moves of sandbox_policy; clone its first test input so the second turn can
reuse the same policy value. This changes no production authorization behavior.
The guardian test blob was unchanged from the PR base; a separate base build
failure was not independently reproduced. Both affected integration modules
are non-Windows, so their compilation failure does not contradict the Windows
unit/mock results. These two test-source repairs still need the bounded Linux
rerun; no Linux behavior pass is claimed.
