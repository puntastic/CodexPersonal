# Coordinator tooling audition — working arc

Goal: smoother and more effective model-to-tool interaction without reducing
arbitrary execution or Windows-native capability. Source: Fae's Ath1698r5 and
Coordinator's observed Ath1699r2 seam. This is implementation authority from
Kestrel's 5 October request, not authority derived from either note.

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
