# October 18 qualification continuation — not deployment

Continue the shared-runtime arc in [October 16](2026-10-16.md) and the
[October 17 checkpoint](2026-10-17-host-fixture-checkpoint.md). Ath2075 carries
the evolving operational hand. Historical package qualification does not cover
later source changes.

## Current footing

- Candidate source: `d2e83ecdb4a5ed9e66a5cd96d147f93a13f2330b`, clean and
  pushed to `personal/codex/shared-runtime-20261016` before this report.
- `personal` is `puntastic/CodexPersonal`; `origin` remains read-only upstream.
  No main integration or installation occurred.
- The active Desktop CLI remains 0.161.0 at
  `8399793e3f65d9c9d931da6a47a0c5a420faa128`. The older staged 0.163.0 package
  at `389f1c663beb...` does not contain these repairs.
- No live peer policy, persistent delegate, directed grant, configuration
  change, restart or model-driven native-message trial was created.
- Latest dispatched qualification:
  [run 39128289847](https://github.com/puntastic/CodexPersonal/actions/runs/39128289847),
  workflow `6606b189666d458a4dd3ecc0eef4ac41e7df7de6`, source `d2e83ecdb...`.
  It was pending at this report checkpoint; admission is not a passing result.

The workflow replacement exists only on
`codex/windows-isolated-tests-20261017`. **Do not merge that replacement into
main.** It records source and workflow identities separately, checks exact
test inventory, uses serial execution without retries, and preserves failed
group receipts. It must run only on a disposable GitHub-hosted Windows VM.

## Isolation correction

The eleven-account-test exclusion is not a complete no-host-effects boundary.
Legacy capture also applies writable-root/protected-path ACL setup and calls a
helper that attempts to adjust the NUL device ACL. A temporary `CODEX_HOME`
does not isolate those effects. The native descendant fixture's workspace was
the checkout, not only its temporary marker directory.

Source inspection does not establish which ACL operations actually succeeded.
There was no before/after ACL audit, and the NUL helper's unusual path spelling
and ignored return further limit claims about successful device targeting.
Do not report either a verified permission change or verified unchanged ACLs.
No corrective ACL writes or permission-bypassing cleanup were attempted.

The local integration build was interrupted before an observed Nextest start
or fresh JUnit. Its private prefix is `20261018-capture-integration`. A zero
child return following console interruption was not accepted as success; the
private outer capture wrapper now preserves interruption as exit130.

The historical `20261018-full-safe-workspace.ps1` now stops before execution
with an explicit isolation warning. Local checks were narrowed to traced
non-legacy paths. Held cases remain outstanding, not excluded to obtain a
passing score. Subsequent native tests run on the disposable VM.

## Qualified components and remaining failures

Private logs, scripts, JUnit and frozen snapshot candidates are under
`codex-rs/target/desktop-dev/reports/`. Cargo artifacts use the established
external target; Nextest reports remain worktree-local. These are overlapping
populations and must not be added into a unique test total.

| Run / source | Observed result | Boundary |
| --- | --- | --- |
| `325ab31e-84dd-4ec5-a2e0-e75bc8f6ed1a`, `ae1b7e8e8` | 784 selected, 781 passed, 3 failed; no errors, retries or named leak warnings | Whole-workspace build graph, voice-host excluded; 7,436 tests and 279 binaries filtered. Not full-workspace execution. |
| `d34b4a78-1d66-48c9-9610-e51a83313941`, `ae1b7e8e8` | Disposable Windows26100: 6 selected, 4 passed, 2 failed | Capture, cancellation, console negative control and interrupt passed. TTY readiness and deletion positive controls failed. |
| `9acdc101-d617-4117-a0ef-b455af715d30`, `a7d98dc02a` | Disposable Windows26100: 8 selected, 6 passed, 2 failed | The same four passed plus two pure buffer checks. Diagnostics explain the failure phase, not its complete cause. |

Exact JUnit SHA256 values, in table order:

- `226c93a7588d0cf1f23215dffc7dff679b9bda79ee0fb4e2c8bdca0f66268019`
- `36ac980e6743b6011ce3296b00650ffbf22d72a87894a2ff167e3f015758fb2f`
- `40e63d807e7c3dd4828a5ac2be784fe28d35b1f7c8a47bf82f26396073446573`

The local failures comprised one expected enabled-retention snapshot and two
Guardian closure-message assertions. The retention case reached both modes
and passed behavioral assertions before its enabled snapshot failed. The
reviewed refresh preserves the generic, non-delegation identities and expected
tool fingerprints; `5d2136c55c` checkpoints it. A passing rerun is still owed.
Frozen candidate SHA256:
`53899f30f9f87355750c7b2f18645df642502142be252c9f4489cadfac43a566`.

The capture repair preserves root status and collected bytes, records each
collector's ending, and bounds post-root drainage without redefining ordinary
execution timeout. Earlier local capture qualification passed 13 cases; the
formerly stalled case returned in 5.044 s instead of a 32.887 s diagnostic wait.
The VM repetitions completed in 3.733 s and 3.503 s. These are scoped observations,
not a general performance benchmark. Existing startup JobObject qualification
and clean repeated startup results are retained in Ath2075; the new local
784-case selection reported no leaks under its wider build graph.

Both stable/experimental API exports and selected Python contracts were
regenerated and checked earlier; the 163 SDK cases overlap the earlier 24-case
run. EOF is not a retained-byte-completeness promise, and null/missing status
means unknown. No Windows legacy public-RPC end-to-end result is claimed.

## Protected approval-lane feedback

The child's actual input established a presentation defect: nested debug
formatting followed by generic middle truncation cut through the closure
notice. Filtering the correct worker request was useful but did not fix this
loss. The generic renderer retains 900 bytes plus its marker; it is not an
exact 900-byte total ceiling.

Commit `fc9c798c26` preserves the assessed rejection, including acting-model
`rejection_instructions` and intentional empty overrides. Only a genuine
Guardian-denied `ExecCommand` whose captured issuing turn is closed receives
typed internal closure metadata. The final exec error renderer applies its
unchanged ordinary diagnostic treatment, then appends the fixed host notice.
It does not infer closure from copied text or strip a guessed suffix.

The earlier diagnostic can contain a clipped or duplicate notice. That bounded
redundancy is intentional: removing model customization or introducing a
larger review-protocol channel was not justified for this repair. Other
consumers retain their original strings. Policy, enforcement, FullAccess,
user/hook denials, generic budgets and wire schemas remain unchanged.

**P0 context-size review flag:** the complete result is bounded to 1,600 bytes
(current conservative composition 1,507), not universally to 1,000 model tokens.
Repository approximate accounting is about 400 tokens, but an unknown tokenizer
must not be claimed checked. This exception received primary and independent
source review: it protects a small fixed control block while retaining the
existing bounded diagnostic. It is not a new unbounded context channel.

The independent review caught one missing exhaustive MCP match; the repair
preserves its existing denial result/string. Portable tests cover provenance,
copied text, captured/other/cleared turns, ordinary rendering and escaped
Unicode. Five mocked child/parent cases cover default/strict behavior, custom
and empty feedback, and long rationale. They check actual outgoing model
inputs, retained assessment rationale, no fourth review, safe continuation and
parent delivery. A scripted assistant answer is not spontaneous model behavior.
Concurrent interleavings remain outside the sequential state tests.

Full formatting and scoped Core/Guardian/Windows lint passed clean in 8m00s
at `04d3ef12`; after the input-control merge, formatting and Windows lint passed
clean in 1m58s at `d2e83ecdb`. Bazel 9.2.0 lock update and error-mode verification
passed without a lockfile diff. Existing dependency-resolution warnings were
not suppressed. These checks do not substitute for the pending runtime cases.

## Native discriminators, not softened oracles

Diagnostics now retain bounded streams, per-operation CMD observations and
phase markers, yield while observing async exit/output, and perform bounded
task-owned cleanup before failure. Original deletion and descendant-lifetime
assertions remain. Success still drops the session before releasing a
preserved child; cleanup cannot manufacture the saved survival observation.
Best-effort cleanup is not a guarantee that termination succeeds.

On Windows 26100, the TTY parent emitted its entry marker, then reported
`-1073741510` before child launch and before cleanup. Microsoft's
[NTSTATUS reference](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-erref/596a1078-e883-4972-9bbc-49e60bebca55)
identifies this as `STATUS_CONTROL_C_EXIT`; the status does not identify its
cause or establish a human keypress. The new paired control keeps stdin open
while retaining the original closed-input arm and every other lifecycle
setting. Its same-VM result remains pending.

All five deletion attempts reported access denied despite CMD errorlevels 0.
The three authorized files remained, so their failed positive controls do not
establish the outside boundary. This differs from the earlier Windows19045
permissive-rights diagnostic; do not call it replication or a security repair.
No token flags or broader ACL policy have been changed.

The pending 23-case slice is 8 Core library + 6 Core integration + 9 native/buffer.
Four older rationale variants are excluded at compile time on Windows and
remain explicit unrun coverage. The workflow builds a same-source Code Mode
helper rather than reusing the historical local helper. Availability of that
helper is not evidence that every selected test executed Code Mode.

## Continuation and release boundary

1. Recover the exact current CI group receipts, source/workflow identities and
   failed output. A failed group must not erase another group's result.
2. Resolve source-backed defects; retain distinct fixture/environment failures
   and the inherited deletion concern without weakening their oracles.
3. Finish source qualification and build a new immutable package. If current
   storage cannot support it, preserve the source and hold the large local step.
4. Rehearse necessary compatibility on isolated copies. Selector rollback is
   not data rollback. Keep the earlier package and private recovery copies.
5. Installation/restart, a persistent local colleague and directed grants are
   separate owner choices. Do not imply that this source checkpoint activates
   the peer capability or completes the wider Ath1988 goal.

Storage remains above the 80 GiB floor with an 84 GiB early-stop margin. Last
ordinary cleanup recovered 0.597 GiB and left 40 protected top-level remnants in
the marked task temp root. No force, ACL/attribute manipulation, alternate
delete route or PID-based Rust termination was used. No voice-host SDK was
installed; its coverage remains unavailable on this desktop.
