# October 18 qualification continuation — not deployment

Continue the shared-runtime arc in [October 16](2026-10-16.md) and the
[October 17 checkpoint](2026-10-17-host-fixture-checkpoint.md). Ath2075 retains
the history and routes to the compact current hand, Ath2368. Historical
package qualification does not cover later source changes.

## Current footing

- Latest Rust checkpoint: `3e46aa36e64a41cbb3949c839a1d773137a2e9cf`, pushed
  to `personal/codex/shared-runtime-20261016`. Its ordinary-user fixture baseline
  exposes a real legacy deletion-enforcement failure. Non-forcing package-smoke
  cleanup follows at `d7ed320d32f54e1491d813fc26bde854fe5854f3`.
  The full campaign remains frozen at `988f24639c...`; do not treat its older
  positive-control failure as equivalent to this qualified counterexample.
- `personal` is `puntastic/CodexPersonal`; `origin` remains read-only upstream.
  No main integration or installation occurred.
- The active Desktop CLI remains 0.161.0 at
  `8399793e3f65d9c9d931da6a47a0c5a420faa128`. The older staged 0.163.0 package
  at `389f1c663beb...` does not contain these repairs.
- No live peer policy, persistent delegate, directed grant, configuration
  change, restart or model-driven native-message trial was created.
- Latest completed qualification:
  [run 39143321895](https://github.com/puntastic/CodexPersonal/actions/runs/39143321895),
  workflow `acf31dae3542dcb6abb01da5600a817cc3da8ab8`, source `3e46aa36e6...`:
  11/12 passed. With ordinary-user full access established, all intended
  deletions work, but the outside file and protected empty `.git` also disappear.
  The unchanged failing oracle is retained. No production permission repair or
  live adoption follows; decisions depending on legacy isolation remain held.
  The full campaign reached its three-hour job ceiling. Its retained console
  proves the ordinary Nextest group finished: 21,344 passed (11 slow, 1 leaky),
  16 failed, 150 skipped. It does not supply a complete campaign receipt or
  establish execution of the eleven later account-sensitive cases.
- A clean, stable-source local package was built at
  `5a29619d7fd356373743dbbeeceb0935f1d8f2dc`, reporting `codex-cli 0.163.0`.
  All six packaged files match the retained hashes/sizes. It is **unapproved**
  and has not been installed, selected by default, or used with the real home.
- Package-pointer timestamp handling is repaired at
  `6d8c4b9fb9fcba4f39be5c5589579272e463d3b9`. This later script change is not
  retrospectively part of the compiled package's source identity.

The workflow replacement exists only on
`codex/windows-isolated-tests-20261017`. **Do not merge that replacement into
main.** It records source and workflow identities separately, checks exact
test inventory, uses declared execution concurrency without retries, and preserves failed
group receipts. It must run only on a disposable GitHub-hosted Windows VM.

## Isolation correction

The eleven-account-test exclusion is not a complete no-host-effects boundary.
Legacy capture also applies writable-root/protected-path ACL setup and calls a
helper that attempts to adjust the NUL device ACL. A temporary `CODEX_HOME`
does not isolate those effects. The native descendant fixture's workspace was
the checkout, not only its temporary marker directory.

For the earlier local runs, source inspection does not establish which ACL operations actually succeeded.
There was no before/after ACL audit, and the NUL helper's unusual path spelling
and ignored return further limit claims about successful device targeting.
Do not report either a verified permission change or verified unchanged ACLs.
No corrective ACL writes or permission-bypassing cleanup were attempted.
The later disposable-VM DACL readback below does not retroactively audit that
local host.

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
| `45dcbb74-99d3-47a1-a97f-7e290933215d`, `d2e83ecdb` | 8 Core library cases passed in 2.692 s | Provenance, captured-turn state, diagnostic preservation and ordinary/FullAccess controls. |
| `9b9018e1-e9fb-4c88-8f42-e0b39b91b94c`, `d2e83ecdb` | 6 Core integration cases passed in 10.791 s | Five child/parent feedback variants plus retention, including both enabled/disabled arms. |
| `4967a78b-b7c4-44b6-b24b-9b8536e8740b`, `d2e83ecdb` | 9 native/buffer cases: 7 passed, 2 failed | The open-input lifecycle control passed in 2.758 s. Closed-input readiness and deletion positive controls still failed. |
| `f379145b-f8cd-4574-ad40-7b257162ba54`, `b103d6a796` | 8 native/pure cases: 7 passed, 1 failed, 3.587 s | Corrected original TTY passed in 2.768 s. Deletion still failed; complete pre/post DACLs distinguish grant installation from effective access. |
| `ac3bfa71-bba3-4780-9577-92f3946718d8`, `8dd65c2e81` | 12 cases: 11 passed, 1 failed, 3.190 s | Actual root capabilities present; ordinary group grants inadequate for seeded-file deletion. All 21 access-check API calls succeeded, separately from their allow/deny results. |
| `c362fdb9-50aa-4bfe-af0f-ab5b8d15ec70`, `3e46aa36e6` | 12 cases: 11 passed, 1 failed, 4.135 s | Full ordinary-user baseline restores intended deletions and exposes outside/protected deletion. Native counterexample, not another failed positive control. |

Exact JUnit SHA256 values, in table order:

- `226c93a7588d0cf1f23215dffc7dff679b9bda79ee0fb4e2c8bdca0f66268019`
- `36ac980e6743b6011ce3296b00650ffbf22d72a87894a2ff167e3f015758fb2f`
- `40e63d807e7c3dd4828a5ac2be784fe28d35b1f7c8a47bf82f26396073446573`
- `c9baebc2d65617b26da33d90a8b48ec20ccf8e729e5848837249dd2e475d1944`
- `c76140b8776d639b56a63bcc39e70b378bb3a4072d26fb86446e0c86668cc074`
- `6f348b2a36921100e06da4ac0d9b521ce699d81e374ca0005fa449b6af399d26`
- `9b8aafeea9c04ac4f575db8444bbbdaac3a42d3e321a0708757c61ae82de05d1`
- `d707abdabeeaf01a894cfa97fff08cef3e9f48c39b84bf90f7af08d794e76cef`
- `844f3d07e106c05760f02e30cdeb1d5f5dff3a3c910ab6a3935249bf8583035a`

The local failures comprised one expected enabled-retention snapshot and two
Guardian closure-message assertions. The retention case reached both modes
and passed behavioral assertions before its enabled snapshot failed. The
reviewed refresh preserves the generic, non-delegation identities and expected
tool fingerprints; `5d2136c55c` checkpoints it. The later six-case Core group
passed the complete retention test, including both modes.
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
clean in 1m58s at `d2e83ecdb`. The final production-aligned TTY fixture then
passed formatting and Windows lint in 11.79 s before checkpoint `988f24639c`.
Bazel 9.2.0 lock update and error-mode verification
passed without a lockfile diff. Existing dependency-resolution warnings were
not suppressed. Runtime qualification remains scoped to the table, not an
overall full-workspace pass or a live model trial.

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
setting. That same-VM control passed both Terminate and Preserve while the
closed-input case again failed before readiness.

Independent review recovered introducing commit
`9b33613db62526359686349bf717f93532807849`: the original oracle concerns job
termination and normal-root descendant preservation, not stdin closure.
Both production TTY callers keep stdin open (`stdin_open: tty` in Core and
`params.tty || params.pipe_stdin` in exec-server). No inspected contract requires
PowerShell to continue startup after immediate input EOF. The bounded repair
therefore aligns the original fixture with production input lifetime, retaining
every lifecycle assertion, deadline, cleanup and drop boundary. The temporary
duplicate control and enum are retired; paired source `f05b6d4a0...` and the
exact receipts remain recoverable. This is a fixture correction, not a claim
that closed-input behavior was fixed or exhaustively diagnosed.

All five deletion operations (four `del` and one `rmdir`) reported access denied despite CMD errorlevels 0.
The three authorized files remained, so their failed positive controls do not
establish the outside boundary. This differs from the earlier Windows19045
permissive-rights diagnostic; do not call it replication or a security repair.
No token flags or broader ACL policy have been changed.

The completed 23-case slice is 8 Core library + 6 Core integration + 9 native/buffer.
Four older rationale variants are excluded at compile time on Windows and
remain explicit unrun coverage. The workflow built a same-source Code Mode
helper (SHA256 `9af88610d8b38175f48ee3d365ef311d39a26e5171856fb0b67bc1f34b430f74`)
rather than reusing the historical local helper. Availability of that helper
is not evidence that every selected test executed Code Mode. All three groups
produced fresh exact receipts; none had a retry or error. The overall workflow
correctly remains failed because the native group failed.

## Full supported Windows campaign

Run [39133843074](https://github.com/puntastic/CodexPersonal/actions/runs/39133843074)
started on 18 October at 13:57:43 UTC, workflow
`c92d4a63074b7a499041b3e6a5d3056a5df232b9`, code
`988f24639c26960426cea0de51f90c81b407f47d`. It inventories the supported Windows
workspace afresh, excludes only voice-host for its unavailable matching native
SDK, retains ordinary ignored handling and requires the unresolved deletion
case. The ordinary complement runs first with two threads; the eleven exact
account-mutating cases follow in serial groups. Locked `dev-small`, two build
jobs, incremental disabled, explicit V8 sandbox feature and zero retries apply.

YAML, six PowerShell blocks, the historical eleven-case identity list,
synthetic inventory partition controls and retained passing/failing JUnit
replays were checked before dispatch. Missing cases and stale reports are
rejected. Independent source review found no concrete blocker; it did not
repeat the semantic account-mutator audit. A complete failed receipt remains
valid evidence but does not pass qualification.

The canonical candidate Build was attempted before tests. Any accepted artifact
is labelled **NOT DEPLOYED / NOT APPROVED**; workspace test builds can change
unified features, so same source does not establish packaged-byte execution.
The candidate step failed in four seconds with a null-valued method invocation;
its upload was skipped. Its `continue-on-error` UI conclusion is not successful
preparation. The detailed stack was absent from that run. A separate bounded
preparation-only diagnostic retained the historical source/setup/order and
reproduced the failure with a stack; it ran no tests and cannot qualify or
deploy a runtime. The final campaign
gate requires both candidate and qualification success and did not pass.

### Recovered ordinary-group outcome and incomplete campaign

The retained full job log, SHA256
`499369f77adfd761c7be555185694b922534d2f54420f2af27be19e6099a238d`,
contains a completed Nextest summary at line 24114 / 16:09:23 UTC:
21,360 tests run, 21,344 passed (11 slow, 1 leaky), 16 failed, 150 skipped,
3,647.189 seconds. Nextest and `just test` then explicitly report failure.
The next test-step output is cancellation at 16:57:58 UTC. That silent interval
does not identify its blocking process or prove another test group ran.

GitHub's annotation identifies the three-hour job ceiling. The retained
artifact contains the complete inventory/partition, source/tool provenance and
resource observations, but no `results.json`, JUnit or ordinary-run-tail.
Console outcomes are evidence, not a fabricated replacement JUnit. Exact
identity reconciliation found 21,360 unique terminal rows matching the frozen
ordinary set, each progress index present once: 21,343 `PASS`, one `LEAK` and
16 `FAIL`. The failure recap repeats those same sixteen, not another attempt.
Only one Nextest run/start is visible. The 150 skipped entries comprise 139
ignored cases and the eleven filtered account-sensitive cases reserved for
later. None of those later eleven has an observed execution outcome.

The frozen workflow waits on `just ... 2>&1 | Tee-Object` before reading,
validating and copying JUnit and writing `results.json`. There are no markers
between those operations. The 2,914.315-second silence after the explicit
`just` failure therefore does not distinguish a pipe/descendant-lifetime wait
from stalled report finalization. Do not call Nextest itself hung. The earlier
`cloud_config::login_and_logout_persist_only_cloud_managed_mcp_oauth_credentials`
test's `LEAK` status is retained as a clue, not proof of the finalization cause.

The [compact recovered account](evidence/2026-10-18-workspace-console-recovery.json)
preserves all sixteen failure identities and eleven later unobserved identities,
exact source/input hashes and log anchors. Independent PowerShell counting
agreed with the read-only Python recovery. It does not audit test early returns
or replace the missing JUnit. Retained failure messages group as follows:

- Five external-agent import/migration mismatches (empty or missing detected
  sessions/project paths; common cause not established).
- Seven Windows filesystem cases reporting OS error 5; setup, operation and
  cleanup causes remain undiscriminated.
- Two TUI cases: an absolute-path snapshot difference and a Kitty output
  substring assertion; neither is silently accepted as a snapshot update.
- One legacy cancellation timeliness failure (17.656 s), distinct from deletion.
- The older pre-baseline deletion-positive-control failure, not the later
  confirmed enforcement counterexample.

Before another full campaign, instrument and bound wrapper completion/report
retention with a small wrapper-only fixture. Preserve native exit, pipe-ending,
JUnit read/validation and copy phases independently. This next check is not run;
it is a more informative step than repeating hours of tests with the same
missing-receipt seam.

The CI null-method failure is not established as the local timestamp problem:
the fresh CI had no previous task package pointer. Preparation diagnostic
[39149134451](https://github.com/puntastic/CodexPersonal/actions/runs/39149134451)
uses workflow `e6d836de9292e6e4e2b1987481f66f856444ebf8`, unchanged source
`988f24639c26960426cea0de51f90c81b407f47d`, a 15-minute disposable Windows job
and a two-minute canonical Build ceiling. Only bounded allowlisted diagnostics
are retained for one day; an unreproduced failure would not be qualification
success. The actual diagnostic completed in about one minute; Build again
failed in four seconds with the exact same null-method message. Its untruncated
stack locates `Get-CodexDevGitState`, `common.ps1:156`, at the detached-branch
name read, then `Invoke-CodexDevBuild` / canonical entrypoint. Exact exception
JSON SHA256:
`c246d63db3c9500e11760fd842e35ca3bf693895c5e990beb81cb89de73f6eb2`.
Eight allowlisted evidence files total 8,766 bytes before the manifest.
Post-attempt tool observations confirm actual Rust/Cargo 1.95.0 and Nextest
0.9.143 despite the ignored toolchain-input warning. This isolates a specific
preparation seam; it does not yet qualify a corrected detached Build.

The narrow repair at `dddfba8becf0619775a1365e911aa2e66ba7fdde` normalizes only
the legitimate zero-row branch result before trimming. On PowerShell 7.6.6,
the original cast of an empty pipeline reproduces `InvokeMethodOnNull`, unlike
a literal `$null` cast; joining produces an actual empty string. Mocked Git
checks passed for detached/named branches, exact HEAD fields, dirty rows,
unchanged source fingerprint, command/fingerprint error propagation, missing
HEAD failure and the changing-HEAD guard. Primary source review and whitespace
checks passed. No corrected full Build or new Rust/native test ran afterward;
the local package remains bound to its earlier `5a29619d7...` source.

Read-only deletion-path audit found that `spawn_prep.rs` silently skips a
writable root without a matching capability SID and discards the allow/deny
ACL-update results. A successful spawn therefore does not establish that the
intended grants were installed. The allow mask intentionally grants inherited
child `DELETE`, not parent `FILE_DELETE_CHILD`. This is a source gap in the
available evidence, not a diagnosed cause of the current failure. If the
already-running campaign reproduces it, the smallest next discriminator is
the exact post-preparation roots/SIDs, update results and root/child descriptor
readback on a disposable VM; intact grants would then motivate checks with
the actual launch token. No token-flag or permission change follows from this
audit. The earlier local synthetic-descriptor result remains separate.

Test-only diagnostic checkpoint `b103d6a7968cf462372f8fdcb0422f86e3624cef`
is committed and pushed separately from that campaign's frozen source. It
records nine fixed fixture-relative DACL snapshots before spawn and, on a
failure, after the unchanged completion/cleanup sequence. The original
filesystem tuple is frozen before post-observation reads; no oracle, command,
timeout or permission setup is weakened. The existing read-only helper opens
with `READ_CONTROL` and requests DACL information only. Returned descriptor
allocations have RAII ownership. No account-name/owner/SACL/token inspection,
PID reopening, path enumeration or extra sandbox-state read was added.

Each path retains at most 512 exact ACL bytes and each snapshot at most 4 KiB,
hex-encoded with length/truncation indicators and DACL-specific control flags.
Absent, null, empty, missing, unavailable and truncated are distinct. A prefix
can omit a relevant ACE. These snapshots are not immediate post-preparation
observations, original setup return codes or effective-token access evidence.
Microsoft's [restricted-token contract](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken)
requires both normal and restricting-SID access checks; a granting capability
ACE alone cannot establish effective access.

Independent source review found no confirmed defect. Full formatting and
scoped `just fix` passed in 11.10 s. A source-traced local run executed only
three pure DACL-format/budget tests and the two existing pure output-buffer
tests: 5/5 passed, no errors/retries, 292 filtered, 0.092 s, Nextest
`a4fefb4d-e89f-4f63-a06e-b5375eface4c`. JUnit SHA256
`7f9ce12d8e3e93494e50d4f3d2f7cef0fb68c15e5ac45a92a0e47b2cf5e6fe80`.
Prefixes `20261018-dacl-diagnostics-fix` and `20261018-dacl-pure-tests`
retain local receipts. No local native ACL reader or sandbox launch ran in
this check. The actual snapshot path is now assigned to disposable run
[39137145645](https://github.com/puntastic/CodexPersonal/actions/runs/39137145645),
workflow `bbe66632f26b5566124c6f653c90ea07c32dd29f`, exact code `b103d6a796`.
Its eight selected cases are the five pure tests, ordinary non-TTY output,
the unchanged deletion test and the corrected TTY lifecycle fixture. It uses
a separate disposable VM/concurrency group, a 45-minute ceiling and no Core,
V8, workspace or package build. Both fixed DACL phases/18 rows have a bounded
projection retained separately from later log output. The original full
campaign continues at its frozen source and workflow. The small run's result
is now in the table and interpretation below; the full run remains unfinished.

## Installed DACLs versus selected-token access

All eighteen readback rows in run `39137145645` are complete and untruncated.
Each positive root gained an allow ACE `0x001301bf`, flags `0x03`; its existing
file inherited the same SID/mask with flags `0x10`. Outside paths were unchanged;
`.git` gained the expected deny entries and inherited allow. No previous ACE
disappeared. Missing allow installation/inheritance is therefore substantially
weakened, not silently assumed from successful spawn.

An independent .NET `RawAcl` decode matched the private decoder for all 147
ACEs' type, mask, flags and SID across the eighteen rows. That is a binary
interpretation cross-check, not an independent native access experiment.
The 157-line private decoder passed 21 invented cases; review caught and repaired
duplicate-key last-wins interpretation before real-data use. Its hash is
`c379ba8243e25fa302b5ac341359fc88b9fa91021dd4c4861f44bf67bd38c282`.

The directories have Users add-file/add-subdirectory bits `0x2`/`0x4`, while
seeded files have Users read access. Marker creation can therefore coexist
with inability to delete existing files. A normal-user/group grant gap is the
leading hypothesis, not a diagnosis: actual token user, group attributes and
restricting membership have not yet been observed. No production permission
repair or token-flag change is justified by the DACLs alone.

Original small evidence has Ath custody, with hash/size-bound upload and exact
metadata readback. The DACL projection also passed a full GET/hash round trip:

- DACL projection, 11,312 bytes:
  `ath-asset:sha256:b50f1d8c371e807d581d911cbfb23897beafc9c26d5cdf2078c0af95f46b183f`.
- Exact JUnit, 15,051 bytes:
  `ath-asset:sha256:9b8aafeea9c04ac4f575db8444bbbdaac3a42d3e321a0708757c61ae82de05d1`.

Ath2368 links both; expiring transfer URLs, homes and sandbox secrets are not
retained. A local OneDrive path alone is not an external-backup claim.

The next test-only observer at `8dd65c2e81...` runs synchronously after ACL
preparation and before launch, borrowing the actual selected primary token and
existing root SIDs through an exact-cwd, one-shot thread-local guard. It reads
bounded token metadata and uses a scoped impersonation duplicate for explicit
`AccessCheck` queries against actual owner/group/DACL descriptors on the same
nine fixture paths. It does not impersonate a thread, print owner/group names,
query SACLs, reopen a PID, construct another restricted token, change flags,
mutate grants or extend the original token's lifetime. Query failure and access
denial remain separate. It is not a running-child token query, a CMD-operation
oracle, or timing-neutral work.

Independent review repaired two diagnostic-only semantics: restricting SIDs
are always enabled for their check despite raw zero attributes, and large SID
authorities require canonical hexadecimal notation. Final narrow review found
no remaining issue. Full formatting/scoped `just fix` passed in 15.79 s. The
four pure TLS/format tests passed in 0.073 s, Nextest
`477632c6-fa34-4d26-a6d0-469c270c0439`, 297 filtered, no errors/retries; JUnit
`6daa8786b6f6cf3548f5a5623dab0cd176c506c87be839c9d41c0c32cae3266b`.
These do not invoke native token/descriptor queries or `AccessCheck`. Native
qualification completed as disposable slice
[39140858454](https://github.com/puntastic/CodexPersonal/actions/runs/39140858454),
workflow `b413fcb5e7205e696cb8865afbc0011c3ed64100`, code
`8dd65c2e81e8544ccf4e5e4b3819482900d35c67`. The earlier eight cases plus these
four pure tests make an exact twelve-case selection. Its result and subsequent
baseline correction are below; the original full campaign continues separately.

## Settled native counterexample, not an automatic policy repair

The selected-token run found all three current root capabilities, but its actual
user had no seeded-file delete grant and Administrators was deny-only. Positive
files allowed reads but denied `DELETE`/`FILE_WRITE_ATTRIBUTES`; positive parents
allowed file creation but denied `FILE_DELETE_CHILD`. This strongly supported
a missing ordinary-user precondition, not absent sandbox capabilities.

The bounded correction establishes actual caller-user `FILE_ALL_ACCESS`, with
OI|CI inheritance, on the fresh owned fixture root before creating children.
All eleven root/child paths are read back before sandbox setup. Null, deny-bearing,
unsupported and oversized baselines fail explicitly; no deny is stripped or
descriptor replaced. The helper uses `GRANT_ACCESS` and deliberately retains
parent `FILE_DELETE_CHILD`, rather than weakening the baseline to make green.
Only `cfg(test)` helpers/setup changed. Full formatting and scoped lint passed
in 15.61 s; independent source review found no concrete issue.

Run `39143321895` then removed all three intended files **and** the outside file
and protected empty `.git`. Post-readback reports `NotFound`, OS error 2, for
all five; their parent directories remain. Completion was normal, stderr empty,
and failure cleanup did not run. The outside path is a sibling of the three
explicit workspace/TEMP/TMP roots, not an allowed root. All three corresponding
capabilities were present. This is an actual deletion failure on owned disposable
test data, not an operation on Kestrel's desktop.

The exact selected-token checks deny direct outside-file/`.git` `DELETE`, yet
permit the outside/workspace parents' `FILE_DELETE_CHILD`. This is consistent
with the parent-delete alternative and the earlier Windows19045 synthetic
finding, not a same-VM flag contrast isolating `WRITE_RESTRICTED`. Production
flags/policy were unchanged by the fixture repair. Independent receiving review
confirmed the source delta, same-run baseline, outside-root relation and absence
of cleanup as a competing cause.

Stop this investigation at the qualified counterexample. Preserve the failed
regression; no ignore, expected-failure annotation, narrower grant, backend
disable or automatic permission-policy choice is adopted. Hold live adoption
or claims that depend on this legacy workspace-write boundary. A repair/redesign,
backend choice or further causal experiment needs separate ownership; unrelated
CI or packaging success cannot turn the boundary failure into a pass.

Ath2368 retains exact small original evidence with checked hash/size custody:

- Baseline JUnit, 18,776 bytes:
  `ath-asset:sha256:844f3d07e106c05760f02e30cdeb1d5f5dff3a3c910ab6a3935249bf8583035a`.
- Selected-token projection, 5,150 bytes:
  `ath-asset:sha256:1ff89411ccdebef204bfa4085bee34cb317472023dfb084e2a9229972f570cbe`.
- DACL projection, 9,730 bytes:
  `ath-asset:sha256:67be57d0365f872ff471dfd43ebc012e16ec5360896566f60155a15a48f3170e`.

## Non-forcing package smoke cleanup

The older package-info smoke cleanup used recursive `-Force`. The narrow repair
at `d7ed320d32...` accepts only an exact direct GUID child under the expected
smoke base, rejects base/sibling/reparse escapes, and uses ordinary literal-path
recursive removal. Refusal/failure warns, retains remaining files and cannot
mask the original smoke error, even with a stopping warning preference.
Version execution, environment restoration and package return fields remain
unchanged. Mocked filesystem/native checks and independent re-review passed;
no actual deletion or binary execution occurred in those checks. This is not
an identified cause or repair of the CI candidate-preparation failure.

## Local package and culture-independent pointer repair

The local locked `dev-small` build completed in 9m40s, with two build jobs and
incremental compilation disabled. Before/after source was clean and unchanged
at `5a29619d7fd356373743dbbeeceb0935f1d8f2dc`, fingerprint
`6efa3ade61df06e253adc343bc527aace49758eb219b12e3ad6295aa295b9ab3`.
Package `5a29619d7fd3-dev-small-20261018-unapproved-local` has fingerprint
`bec8a0c12049d33592d9d84797b5541b1f2b0d8817e438d83cc9f103f383f3e0`;
its version smoke returned `codex-cli 0.163.0`. A separate readback verified
all six packaged files against the receipt's hashes and sizes.

The original Build result remains `built_pointer_failed`: the artifact and
stable-source receipt exist, but `ConvertFrom-Json` materialized a UTC
`DateTime`; casting it to text produced `10/16/2026 14:13:06`, which failed
the current en-AU parser. The source ISO timestamp was valid. This is a concrete
local bookkeeping defect, not evidence that the binaries failed to build.

The repair retains typed `DateTime`/`DateTimeOffset` values and full tick
precision, accepts explicit-zone ISO strings with invariant parsing, and
rejects unspecified/ambiguous/malformed values without overwriting a pointer.
The existing lock, newer-wins/tie semantics and source-stability guard remain.
Tests reproduced the old Australian-culture failure and passed under en-AU,
en-US and fr-FR, including one-tick ordering, invalid-existing/input handling,
and lock release. The existing mocked non-forcing smoke cleanup checks also
passed. PowerShell 7.6.6 was executed; 7.2 compatibility was reviewed, not run.

The task's real latest-package pointer remains unchanged deliberately. Keeping
this unapproved artifact out of the default selection is preferable while
qualification and the legacy isolation decision remain unresolved. The
original failed Build result/receipt is preserved, not rewritten as success.
No Rust rebuild is necessary merely to repair later bookkeeping source.

## Continuation and release boundary

1. Recover the exact current CI group receipts, source/workflow identities and
   failed output. A failed group must not erase another group's result.
2. Resolve source-backed defects; retain distinct fixture/environment failures
   and the inherited deletion concern without weakening their oracles.
3. Retain the existing unapproved immutable package. Complete or explicitly
   bound qualification before an adoption decision; do not rebuild for a
   bookkeeping-only source change or reinterpret the security counterexample.
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
