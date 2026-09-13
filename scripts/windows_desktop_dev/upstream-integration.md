# Upstream integration contract

CodexPersonal stays reasonably close to `openai/codex` because the Desktop app,
remote clients, backend protocols, and model catalog evolve together. Upstream
is an input to qualify, not an automatic deployment source. Integrate it in an
isolated branch and worktree, preserve the personal fork's intentional seams,
regenerate derived artifacts, run focused tests, build a reversible package,
and only then select it for the next Desktop restart.

## Current update and last qualification

The September13 qualified update is on `codex/upstream-sync-20260913`, based on stable
`rust-v0.154.0` (`6b9826e3aa83b1a5947db50f4332cb9c65f1b340`) and selected later
fixes. Runtime source `c196cb9040fbfba882e1ff5ae539736b7a8ab81b` is published to
personal/main and packaged as0.154.0. The package is selected for the next
restart; actual post-restart Desktop and phone use remain unobserved.
Its evidence, exclusions and remaining gates live in
[`updates/2026-09-13.md`](updates/2026-09-13.md). The live source verified at the
start of this pass was `620e488dcbbb2613fa4e723a1d6633c10d8a1ad2`.

The following block records the prior September5 source qualification; its
then-pending deployment state is historical, not a current deployment receipt.

- Personal-fork base: `54382846b5461d7c851dd6f72cc31e3beac3a978`
- Upstream candidate: `83b62a02fab5c0fc797cbc9896c332148f1fd9d0`
- Integration branch: `codex/upstream-astra-integration-v1`
- Remote compatibility advertisement: `0.153.0-alpha.5`
- Source-qualified on: 2026-09-05
- Deployment state: package build, selection, restart, and phone check pending

Update this block when a candidate is accepted. A fetched commit, completed
merge, passing source tests, built package, selected package, restarted app, and
working phone connection are different states.

The source qualification covered the fork's state/history/rollout/thread-store,
compaction/resume/fork, Guardian, context-management, model-catalog,
app-server protocol/transport, and Remote seams. The broad app-server run passed
1,405 of 1,411 cases inside Codex Desktop's own Windows sandbox. All six
non-passing cases then passed in the appropriate host environment: telemetry,
image attachment, and both approved filesystem writes passed outside the nested
sandbox; the stale three-denial fixture passed after being aligned with the
fork's lane-only circuit breaker. The Windows development-lane self-test also
passed. This is source evidence, not a substitute for package provenance or the
post-restart phone check.

## Project-sensitive merge seams

### History and compaction

The fork's `PaginatedRefsV1` and `PaginatedRefsV2` modes are real paginated
histories. Adapt upstream checks that mean “any paginated history” to the
`ThreadHistoryMode::is_paginated()` contract. Keep `replacement_history_entries`
and V2 source digests alongside upstream's `retained_context` and
`guardian_history` checkpoints.

Compaction must attach the model `comp_hash` before encoding digest-bound
replacement entries. Resume must select the newest surviving complete
checkpoint, resolve legacy or reference-backed history, fail closed on missing
or mismatched references, restore retained context and Guardian history, and
then replay the surviving suffix. A child fork re-encodes inherited references
for its own rollout but clears parent-local retained authorization and Guardian
history.

Model history, host-owned Guardian history, and retained authorization are
independent rollback evidence surfaces. Count inter-agent instructions as task
boundaries, but never pretend they are retained user-authority ledger rows. A
metadata-free rollback may use exact source-call and retained-ledger evidence;
if that evidence is incomplete, discard ambiguous retained facts. Preserve a
compaction summary while rolling back only its explicit suffix, but clear the
model window when rollback crosses into turns absorbed by that summary because
the summary cannot be safely edited in place.

An oversized rollback after compaction can remain ambiguous even when the
retained user-message ledger is complete: empty-input turns are not ledger rows,
and inter-agent instruction boundaries are retained on a separately bounded
surface. In that case remove the old answer and restriction text, keep the fixed
incomplete-evidence notice, and force fresh review. Do not “repair” the notice by
saturating against user messages alone; that can over-remove an older project
instruction after inter-agent history eviction. Exact silent completion needs a
durable all-instruction-boundary account or an equivalent completeness proof.

### Context management

Keep upstream activation semantics. Experimental context management activates
once at thread startup, only for eligible ChatGPT plans on the Codex backend,
enables token-budget handling, and turns on the history-notes extension. It
must not activate through API keys, custom providers, custom bearer tokens, or
AWS configuration. Substantive user prompt settings take precedence over model
defaults, and later model switches do not silently change the thread's
activation state.

The model catalog's `include_*_usage_instructions` flags control duplicated
usage prose; they do not remove the skill, app, or plugin catalog. Keep the
personal cue extension and capability catalog installed through their existing
developer/context layers rather than copying them into a model prompt.

Do not enable `context_management` merely because an upstream candidate ships
it. It remains an under-development, default-off feature and needs its own
adoption decision. The 2026-09-05 qualification snapshot left it disabled in
the live user configuration; its eligible-auth, startup-only activation,
model-switch, token-budget, notes, and compaction behavior was tested in source.

Keep experimental in-turn `step_model_switching` disabled as well. Its current
world-state path freezes some base instructions, personality, and capability
usage flags from the turn model while other settings follow the step model.
That split is acceptable behind the default-off gate, but it needs a dedicated
repair and qualification before project adoption.

### Guardian and delegated authority

The project is not adopting Guardian as an active supervisor through an upstream
update. Preserve the user's existing authority and autonomy balance. Peer review
contributes evidence; it does not automatically acquire permission authority.
Compatibility alone does not justify changing approval defaults or adding a
reviewer. The following contracts preserve behavior where the review path exists;
they do not instruct an operator to activate it.

Model-specific Guardian policy controls ordinary review coverage, not project
authority. An absent model policy preserves legacy coverage; a supplied policy
map is complete, so omitted scopes are disabled and ordinary requests continue
through their native user-approval path. A host-owned Guardian requirement or a
force-fresh retry/sensitive-action rule overrides a disabled model scope and
still requires synchronous review. If the Guardian extension cannot answer,
Core retains its synchronous fail-closed fallback.

Resolve policy, review model, reasoning effort and summary, personality,
rejection wording, circuit-breaker class, timeout wording, and analytics from
the immutable step that issued the action. Do not route those decisions through
a mutable thread-wide model slot: captured steps can coexist across a live model
switch. Map shell, file-change, network, MCP/computer-use, and permission
requests to their exact model-policy scopes. Keep the fork's approval-lane
circuit breaker and use this order:

1. Full Access approves immediately without reading or mutating the breaker.
2. A closed approval lane rejects without another model review.
3. The issuing model's policy chooses disabled, adaptive, or synchronous
   coverage for the exact request scope.
4. Guardian V2 may satisfy an eligible adaptive request from complete current
   evidence; otherwise the synchronous reviewer decides or an ordinary disabled
   request reaches the user.
5. Only an explicit denial advances the breaker. Reaching its threshold closes
   the lane for the current turn; it does not abort the turn or emit thread idle.

The breaker is session-local and resets for a new turn, resume, or fork.
Durable Guardian history and retained root authorization may survive compaction
and resume under their own bounded contracts. Parent-local review evidence must
not become child authorization.

The 2026-09-05 live Astra metadata snapshot supplied no `guardian` map and kept
the legacy computer-use review bit enabled, so it follows the existing project
coverage. Keep tests for supplied per-scope maps because backend model metadata
can change independently of this branch.

### State migrations

Never reuse an upstream SQLx migration number. When a released fork migration
collides, keep upstream's numbered migration, move the fork SQL byte-identically
to a timestamp version, and repair only the exact historical checksum before
SQLx validation. Test fresh, upstream-only, and every shipped fork ledger.
Retain a pre-upgrade database when an older binary cannot understand the new
canonical ledger.

### Phone Remote version

Development main has historically used workspace version `0.0.0`; release tags
may stamp a real version (the September13 stable candidate declares `0.154.0`).
Preserve that source identity rather than changing it to bypass a compatibility
check. Until a typed compatibility identity replaces the stopgap, record any
explicit `-RemoteControlAppServerVersion` and its source. It may affect only
remote enrollment and the initialize user agent for an actual
`ConnectionOrigin::RemoteControl`, never local/daemon identities or WebSocket
protocol version. A source-release version is evidence for choosing the value,
not proof of successful phone use.

Recheck the advertised value against the installed official Desktop build when
upstream changes the minimum client version or Remote rejects the environment.
Only a fresh phone connection and message proves end-to-end compatibility.

## Release gates

Before selecting an upstream integration package:

1. Resolve every merge marker and regenerate config and app-server schemas.
2. Run formatting and focused tests for state, history, rollout, thread-store,
   core compaction/resume/fork, Guardian, context management, model catalog,
   app-server protocol/transport, and Remote initialization.
3. Run the Windows development-lane self-test.
4. Build with the recorded Remote compatibility value and keep its provenance
   receipt.
5. Deploy through the lane, restart Desktop, verify the selected package, then
   test phone Remote. Keep the prior package and pre-migration database recovery
   path until the new build has settled.
