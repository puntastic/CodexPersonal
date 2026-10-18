# Host qualification checkpoint — not release approval

Continued on October 18 in
[the qualification and isolation correction](2026-10-18-qualification.md).
The material below remains the historical checkpoint, not the latest status.

This follows `2026-10-17-qualification-followup.md`. The shared Desktop
runtime remains unchanged. No package installation, restart, peer grant or
persistent helper activation occurred. This checkpoint preserves unfinished
qualification while the owner's Ath authentication outage takes priority.

## Executed evidence

- First compiled repair: run `493345c4-c998-4170-9005-644faa9f04d5`,
  138 cases, 120 passing and 18 failing, zero errors/retries.
- Second compiled repair: run `dd12d3aa-8970-4842-b401-5a488dd2b00e`,
  278 cases, 275 passing and three failing, zero retries, 142.741 seconds.
  The runner also reported 22 leaky passing tests and one leaky failure;
  this is not a clean-teardown claim.
- All original stack-abort cases reached ordinary outcomes at the original
  8 MiB default; no stack abort remained. One reached a separate fixture
  ownership error. The native default was not increased.
- Two analytics plan cases timed out during embedded startup. Reusing the
  same TUI executable (SHA256
  `ecd40dc41939c854a3c6605ea64adc62cb99aa16d91a6af4db6ba8cb284d3dc3`)
  without rebuilding, all eleven plans passed at two threads and zero retries:
  run `c0f8ca84-e5c5-417f-ac0d-f980e8adc236`, 5.825 seconds.
  This establishes a non-reproduction, not the cause of the earlier timeouts.
- Four formerly timed-out Exec cases, private-socket fixtures, HTTP checks and
  the explicit V8 sandbox feature/native Code Mode assertion passed in the
  second selection. Positive native TLS classification on Windows is not
  claimed.

Reports are retained in `codex-rs/target/desktop-dev/reports/`:
`20261017-qualification-repair-first.junit.xml`,
`20261017-qualification-repair-second.junit.xml` and
`20261017-analytics-recheck.junit.xml`. The second report SHA256 is
`c504a252cafdf2281ead34213c7f0356e2f3fdd9e08b307e45b2aa8c2b828e25`.
Its exact pre-build patch and command are retained beside it as
`20261017-qualification-repair-second.patch` and `.ps1`.
These sets overlap and must not be summed as unique coverage.

## Candidate state newer than that run

Three final source changes still need execution:

1. The historical compression seed now gets a distinct worktree. The prior
   attempt incorrectly reused a deliberately invalid-bound owner fixture;
   the manager correctly refused it. Invalid-owner, modern-owner, fork-history
   and explicit Legacy-mode assertions remain.
2. HTTP fixtures explicitly bind native TLS for the real rejection check and
   retain a genuine positive bridge on Linux/macOS. The latter is not runnable
   on this Windows host. Injected mechanics tests remain truthfully separate.
3. Three legacy descendant fixtures put markers and redirected output in a
   temporary directory inside the already-authorized workspace, not private
   sandbox state. Their empty environment supplies no writable temp root under
   the current permission contract. No new grants, environment values,
   production token flags or timeouts were added. Bounded capture diagnostics
   expose early failures.

The separate legacy deletion-permission concern remains unresolved; no
production security repair is included. Detailed diagnostic evidence remains
local/private. Do not weaken or suppress its failing oracle.

`just fmt` and `git diff --check` passed after the final source changes.
Final scoped lint, the unrun candidate cases, wider current-source Core and
app-server coverage, leak disposition and a new clean package remain owed.
The earlier account-mutating CI coverage is still bound to its older source;
voice-host SDK coverage remains unavailable.

## Resume safely

Reuse the established dev-small workspace graph, locked dependencies, two
build jobs/test threads and incremental-off setting. Use `just test`, not
direct Cargo test. Include `codex-v8-poc/sandbox` for its positive assertion.
Use failure-only runner output; `all` emits thousands of filtered cases.

TEMP/TMP for this phase is `C:/tmp/codexfull20261017c`, with its task marker.
It has not been cleaned yet. Earlier protected residue and the denied
non-forcing cleanup remain untouched. Preserve the 80 GiB free-space floor
and 84 GiB early-stop margin; about 104.8 GiB remained at this checkpoint.
Do not turn full workspace access into permission changes or forced cleanup.
