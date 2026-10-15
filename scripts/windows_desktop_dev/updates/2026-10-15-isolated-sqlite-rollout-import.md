# Invocation-scoped SQLite rollout import selection

Source candidate on `codex/isolated-history-startup-20261015`, based on
`04c9a8858b1e481141b091fd037bf8b7d17e1e3e`. No Desktop selection, installation,
saved settings, authentication, permission policy or cloud requirements changed.

`-c sqlite_rollout_import="disabled"` opts out of automatic startup import of
existing `$CODEX_HOME/sessions` and archived rollout history into SQLite. Omission
or `"enabled"` retains the existing behavior. It requires an explicit
`sqlite_home` pointing at an existing directory, canonically distinct from
`CODEX_HOME`. An environment-only home is insufficient. Configuration resolution
still applies requirements first; a requirement redirecting the explicitly
selected home causes a visible configuration error rather than silent reuse.

The harness should pass both settings on each owned process invocation, including
helper `exec` processes. This is an ordinary typed configuration key, not a ban on
an owner intentionally persisting it. The guard does not attest task ownership,
freshness or exclusive use of an arbitrary alternate directory; those remain the
caller's responsibility. Directory aliases can also change after validation.

The policy travels with `SqliteConfig` through rollout owning initialization and
optional `get_state_db` reads, not just the app-server entry point. Database
migrations and normal new-thread persistence still run. Backfill state remains
truthfully pending/running; it is never marked complete merely to bypass the
startup gate. Reopening with the policy disabled preserves that state. Reopening
later with the default policy again follows normal backfill and lease handling;
a retained running lease may delay or fail that attempt. This is not an access
boundary: explicit history reads, selected-session resume/reconciliation and
filesystem fallback APIs remain separate and are not disabled.

The source inspection explains one credible startup cost: rollout initialization
awaits metadata import before its backfill wait-time check can run. A timed-out
owned invocation had an incomplete backfill checkpoint and imported rows; an
empty-home initialization completed quickly. Those observations motivate this
opt-in but do not prove that all startup delays have this cause. Historical
attempt receipts remain unchanged.

Focused tests cover default import, disabled import leaving old rows absent,
explicit rollout reconciliation and metadata reopening (not the full new-thread
recorder lifecycle), repeated owning/optional initialization preserving a running
checkpoint, configuration rejection and managed-home redirection. The alias
cases exercise a case alias on Windows and a symlink on Unix; neither alone
establishes every filesystem alias or later path replacement behavior.

## Focused Windows checks

Using the existing Rust 1.95 toolchain and shared `dev-small` cache, with two Cargo
jobs and incremental compilation disabled:

- `just write-config-schema --profile dev-small` passed. The generated fixture
  changes only the enum and setting described here.
- `just fmt` passed after correcting a missing brace in the new test source;
  `git diff --check` passed. No unrelated formatter changes were retained.
- `just test -p codex-rollout -p codex-core --lib --cargo-profile dev-small -E
  'test(state_db::tests) or test(sqlite_rollout_import)'` passed 14/14,
  with 2872 tests skipped; nextest run
  `fd49a192-a602-4f42-9a5b-47439cb96e15`, exit 0.
- The preceding selector mistakenly named `config::config_tests` rather than
  the mounted `config::tests` module. That earlier run passed 10 rollout tests
  only (`c68fd4dd-2305-495e-a7af-0aa5bfe4f041`); it did not establish the four
  new configuration cases. The corrected run above includes them.
- `just fix -p codex-rollout -p codex-core --lib --profile dev-small` passed,
  exit 0, with no automatic source repair observed. The received diagnostics
  contained no warnings. Tests were not rerun after this lint/format phase.

The Unix symlink case, full workspace suite, actual thread recorder lifecycle
and real-profile native startup remain outside this local test result. Native
availability and package binding are the parent's separate follow-on gate.
