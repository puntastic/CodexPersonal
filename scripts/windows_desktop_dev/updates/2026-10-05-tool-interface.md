# Coordinator tooling audition — working arc

Goal: smoother and more effective model-to-tool interaction without reducing
arbitrary execution or Windows-native capability. Source: Fae's Ath1698r5 and
Coordinator's observed Ath1699r2 seam. This is implementation authority from
Kestrel's 5 October request, not authority derived from either note.

Current qualification:346 focused command/shell/scenario checks passed, followed
by40 related checks for the user-requested mailbox test repair. Scoped Clippy
completed, and the repository formatter completed. Per repository instructions,
tests were run before final fix/fmt and will not be repeated afterward. The
canonical package and live activation remain separate pending steps. No running
Desktop selection, permission default, or global shell setting has changed.

## Boundaries

- Preserve current approval/sandbox/hook mediation and secondary-instance scope.
- Preserve the existing script/session/PTY route; add clarity, not a fixed palette.
- No Windows or Codex restart, global PATH change, or active-binary replacement
  without warning Kestrel first and allowing them to stop their current activity.
- Portable workspace-only tools and isolated test/build processes are in scope.
- Do not infer comparative model skill from deterministic transport fixtures.
- The full repository suite requires the user approval specified by AGENTS.md.

## Current source and recovery

Native source base: fff6b3c94035a3386b981dcaa63f8e3564943e78 on
puntastic/CodexPersonal; code matches deployed1dcf0e (two later documentation
files differ). Active executable remains the immutable d29db7ac release,
codex-cli0.157.1. Task branch: codex/tool-interface-20261005.

## Actions and checks

1. Inspect current runtime/source, existing Working Desk, and candidate surfaces.
   Done: model `shell` path chooses a type, misses Windows Git discovery and
   silently falls back to CMD; direct Bash binary works. Existing `command/exec`
   RPC already has argv, streams, EOF, termination and PTY controls. Reuse native
   process ownership rather than replacing it with capture_output subprocess.
2. Repair supported-shell resolution and its description, preserving type-only
   trusted discovery. Check successful Git Bash resolution and explicit errors
   for unsupported/unavailable requested shells. Default-shell behavior remains.
3. Audition additive direct argv at the model-facing unified-exec boundary.
   Keep literal arguments, current execution permissions, hook visibility and
   lifecycle. Explicit script commands remain available. Ambiguous requests or
   hook rewrites must fail clearly, not silently reinterpret payloads.
4. Compare PowerShell, directly launched Git Bash, portable pinned Nushell and
   direct-program execution on a small deterministic Windows corpus: argument
   fidelity, Unicode/binary I/O, exit/error separation, expected nonzero,
   Windows-native inspection, and retained-output recovery. Test session/PTY
   behavior through the existing native path; do not claim untested parity.
5. Use observed results to select a primary path and retain useful specialist
   access. Prepare coherent source commits, focused tests, staged package and
   exact rollback; expose any restart or full-suite decision before activation.

## Useful stopping points and fog

### Parked for the user's Workshop lookup, 5 October

Native Rust checkpoint: `fd267c4110215f97f9523319a10a96a54cf0e779` (local only;
not formatted/finalized, pushed, packaged, selected or live). Checks completed:
173 shell-command tests, 78 focused core tests, and the three new end-to-end
argv tests passed. The broader core run completed 4154 tests with204 failures
(and77 tests excluded by runner selection); do not call this a green full run.
It produced changed-tool-schema snapshots plus failures still needing triage.
One inspected failure is a missing `target/debug/codex.exe` fixture binary;
another is a pre-existing-looking acceptance_order expectation, not yet baseline
reproduced. No broad failure has been waived as unrelated without checking.

Exact broad-run JUnit is retained at
`C:/Users/xxobo/OneDrive/Documents/Skills/Artifact Staging/tool-interface-20261005/core-full.junit.xml`.
The actual current nextest report was written under THIS worktree's
`codex-rs/target/nextest/local`, despite the attempted CARGO_TARGET_DIR setting;
some missing-binary diagnostics point at the older shared target. Resolve that
test-environment split before treating every failure as a runtime defect.

Resume correction: `cargo metadata` and the generated test executable confirm
that the shared target was used correctly. Nextest's report store is rooted in
the current worktree independently of Cargo's target directory. This is not a
split build. The initial run lacked separately built `test_stdio_server`,
`codex-code-mode-host`, `codex`, and Windows sandbox helpers. Twelve snapshots
include both intended schema-hash changes and failed helper launches; they must
be regenerated after prerequisites are built, not accepted wholesale. Two
mailbox assertions concern unchanged source and still need a baseline check.

Additional regression cases cover literal arguments plus nonzero
exit/stderr, direct-program PTY input and completion, and the native Windows
Bash dispatch scar. Fixtures are synthetic; no live model or Desktop session
is involved. `just --command` requires an explicit working directory here;
unlike a recipe, it does not apply the justfile's recipe working-directory.

### Resumed qualification

Source checkpoint `3703e5444` includes those additional regression cases.
The missing helpers now build successfully (19m17s). The unfiltered queued
core run was interrupted before testing when inspection showed that the
Windows sandbox integration group provisions machine-level users and firewall
rules. That machine setup is not needed or selected for this change.

Run `33cd0543-9e5c-4612-9742-5ead8739b1d7` now exercises4151 core tests, excluding
`suite::windows_sandbox::` (83 tests excluded in total). It completed with4135
passes and16 failures in1286.053s. All six `unified_exec_argv` end-to-end cases
passed. The16 residuals are14 schema-hash snapshots and the same two mailbox
assertions. Snapshot normalization found the expected additional `argv` in one
visible tool signature. After reviewing that exact delta, a second guarded
normalization confirmed only tool hashes, `argv`, and assertion-line metadata
changed; all14 snapshots were accepted. Focused retesting is pending.
The exact report is `core-prerequisites-fixed.junit.xml` beside the original
204-failure report. The baseline mailbox discriminator, fix/fmt and canonical
package also remain pending.

The separately built candidate executable has SHA256
`a00bc24b489ca22a04985e1a59a3de77cd3e0cd76a1924646f54583280fa3b89`.
`candidate-probe-01/report.json` contains another48 native-RPC observations,
reproducing the prior pass/fail pattern exactly: four arms8/8 and the two
declared negative controls7/8. This exercises the backend, not model use of
the new tool field. The pinned probe script SHA256 remained
`364dc6e0f465e0ab341e17acc837bd2fc05e40d7c0efa4e7f08a7ee7273c4234`.
Single-run timings vary substantially across these two loaded-machine runs;
they are not a causal speed comparison. A real Nu read/filter/select query
over this report also returned the expected two negative controls.

The portable Nu0.116.1 probe and stream files are under the same Artifact Staging
root. `baseline-probe-05/report.json` records48 deterministic observations:
direct, current-Codex-equivalent PowerShell UTF-8, scoped-literal Bash and Nu each
pass8/8; raw PowerShell and default Git Bash each pass7/8. Raw PowerShell's Unicode
failure is already addressed by the existing Codex UTF-8 prefix; Bash's literal
path conversion is corrected by a per-invocation MSYS2_ARG_CONV_EXCL setting.
These are transport/structured-view checks, not model-generation superiority.
Earlier probe01 had an RPC enum error and probe04 stopped on an unhandled decode
error; they are not valid complete comparisons.

Working Desk store: `Artifact Staging/tool-interface-20261005/desk.sqlite`.
Exact stdout receipt: `desk:e8d6bc54a349403eab37aef968417ae0`.
Manual opportunity `opportunity:3eba1db640fc11fb00ea607f` records a long-build
capture limitation; it is a nominated seam, not a demonstrated automatic repair.
The Desk executor's3600s ceiling is unsuitable for uncertain-duration Rust builds;
use native execution and capture selected completed artifacts instead.

No tests remain running at this checkpoint. Next: inspect failed-test groups and
snapshot deltas, complete proportionate checks, run required fix/fmt, build and
stage, then warn the user before any disruptive activation. The full workspace
test suite still requires the AGENTS.md user approval. Do not restart Desktop.

Native tool schema activation may require a Codex restart. A staged and tested
package is not a live tool. Nushell's syntax/structured values are a separate
treatment from direct argv; installation alone proves no benefit. If integration
would lose existing hooks, permissions or lifecycle behavior, stop that candidate
and preserve the functioning baseline rather than conceal the loss. Ordinary
model use after activation remains the next evidence for real working benefit.

The preliminary portfolio from Fae covers PowerShell, Git Bash, WSL, Nu and a
structured native contract; Xonsh/process APIs supplied the farther-frame donor.
No second broad literature sweep is owed unless local contact changes the design.

### Baseline discrimination and user-requested mailbox repair

The two mailbox assertions failed identically on the untouched
`fff6b3c94035a3386b981dcaa63f8e3564943e78` baseline (run
`da1c2c97-3623-42d2-9377-fe6dfe21aaf9`, `baseline-mailbox.junit.xml`). After
restoring the candidate,346 focused shell, execution, and scenario tests passed
in62.098s (run `4d99c62e-80ea-4cf1-9158-91d672ea36ec`,
`focused-final-preformat.junit.xml`).

Kestrel then explicitly requested repair of the mailbox failures. Source
inspection showed stale test assumptions: `reserve_user_input_order` deliberately
returns `None` in Legacy mode, whereas ThreadOwned mode reserves a sequence.
The fixtures used default Legacy sessions but asserted `Some(0)`. Both tests now
select and exercise both modes, preserving exact user-input-before-mail delivery
assertions. This is a test repair, not a production messaging repair or a change
to live Guardian settings. All40 related mailbox, input-queue, turn-input and
retained-order checks passed without the initial fixture-construction warnings
(run `6cdc10ce-c79c-47fb-8f73-5db2da4b3b25`). The started linter was interrupted
when this new scope arrived and will be rerun after the repair's tests.

### Public-facing home

GitHub readback confirms `puntastic/CodexPersonal` is public; the earlier spoken
private-fork assumption was wrong. Kestrel authorized setup and user-space Git
for the separate public `puntastic/agent-toolbench` repository. Its purpose is
human-readable experiments, relevant fixes, runnable checks and bounded results.
Show and Tell is the intended announcement venue; no upstream contribution is
requested. Repository preparation/publication and posting an announcement are
separate actions. Preserve raw local records locally and publish selected,
sanitized evidence rather than whole transcripts or workstation state.
