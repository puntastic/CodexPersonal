# October 20 continuation - reporting and focused failure triage

Continue [the qualification checkpoint](2026-10-18-qualification.md) from
`22605f0750f960d5f4554dbfb5fbccf4fd6a1654`. The owner elected to retain the
[known legacy limitation](2026-10-19-legacy-boundary-repair.md#owner-decision---retain-unchanged-with-known-limitation).
No refusal guard, legacy redesign, live policy change or installation belongs
to this pass. Other qualification/recovery duties remain.

## Current slices

1. **CI capture/reporting:** a harmless local discriminator reproduced waiting
   on descendant-held output pipes in the old PowerShell Tee shape. File-backed
   capture returned after root exit, before the self-expiring descendant ended.
   This supports the mechanism and avoidance, not its causal responsibility for
   the original 48-minute CI silence. A small Python stdlib helper records the
   original root exit before later report work, retains raw streams separately,
   and explicitly leaves descendant/log completeness unknown. Its latest twelve
   wrapper tests pass; source review found and repaired legacy Actions-command
   escaping and a timeout-cleanup guard. Hosted Python3.12.10 ran all twelve in
   3.656s. All six hosted Rust groups retained complete native/capture/report
   receipts, including the failing group. This run did not reproduce the stall.
2. **Five import failures:** source/output comparison identified the decoder's
   inability to reconstruct several punctuated path components together. The
   candidate searches existing prefixes within a fixed 512-probe ceiling instead
   of the old 128, without directory enumeration. Ambiguous or unfinished search
   still returns no result. Four new controls cover multi-ancestor paths, native
   roots, a dead-end branch and budget exhaustion after a match. All seventeen
   selected Cursor tests and the separate app-server end-to-end import case
   passed locally on Windows. POSIX-specific branches were not run.
3. **Two TUI failures:** fixture-only candidates use a valid file URL in Markdown
   and inspect the decoded Kitty file-reference payload rather than rejecting
   a base64 substring that can legitimately occur in a filename. Original
   rendering/soft-break/protocol expectations remain; no production renderer
   changed. Both focused Rust tests passed locally.
4. **Seven filesystem failures:** diagnostic-only test context distinguishes
   operation phase, immediate I/O error kind and raw OS error while preserving
   the original cause. Explicit unsupported-token markers distinguish early
   returns from observed operation success. The repeat run's seven failures are
   existing-file mutation stages: parent-seeded write/remove, junction test's
   existing-file write, and private-desktop's first existing-file write (before
   its lifecycle assertions). New-file write and directory controls passed.
   Immediate InvalidInput/raw-code-none wraps the helper's AccessDenied/OS5;
   this distinguishes an operation denial from outer launch/RPC failure, not
   which ACL/native open denied it. The private-desktop case follows symlinks;
   no-follow alone cannot explain all seven. No production repair is inferred.
5. **Cancellation timing:** test-only timestamps distinguish pre-cancellation
   preparation/readiness from response after cancellation. The original commands,
   30-second timeout, ten-second elapsed assertion and descendant oracles remain.
   The repeat passed: total1632ms, pre-cancel1621ms, post-cancel11ms. One passing
   repeat does not disprove the earlier17.656s timing failure or identify its cause.

## Checked results and source bindings

The primary branch checkpoints are `07c2449194cfdbe1937f0e6e33e377229b3eb36c`
(capture helper), `937efb776e3a737995bdfbf2eab4ecc79d963371` (diagnostic tests),
`968ec14b3a532e45aca36d470d025189e02ab87c` (Cursor decoder) and
`fe2159bcdc0e2d9a3b0c88276cfcaa0961858cb0` (TUI fixtures).

Full formatting and scoped Clippy/fix for external-agent-migration, tui,
exec-server and windows-sandbox completed successfully (compiler17m40s).
The local focused run passed19/19: seventeen Cursor tests and the two TUI cases;
Nextest `ed96b064-7e04-4074-9c38-90481d51e5a9`, execution1.325s after24m26s build.
JUnit SHA256 `7471aa3b2f97e97054dbef9e1e851b269289b289ec520be933408f729ae8e846`.
This is selected Windows coverage, not another full-workspace pass.

Disposable run [39350468947](https://github.com/puntastic/CodexPersonal/actions/runs/39350468947)
used source937efb7 and workflow `1eac41fbac0c741dac4b7f57f5c18125dd97188e`.
The exact24 outcomes are17pass/7fail across six groups. All original eleven
previously unobserved elevated/account cases passed, as did the three legacy
capture cases. The filesystem group passed three and failed seven. No
unsupported-token early-return marker was recorded. The accepted legacy
deletion regression was not selected; this does not relabel it passing.
All inventories, JUnit identities and phase receipts were checked. No wrapper
or report failure obscured the native nonzero status.

The immediately preceding run39350101485 failed before Rust execution: a
PowerShell Get-Command lookup returned both the selected Python executable and
a WindowsApps alias. The workflow correction selects the first application
for Python/Cargo/Just; no environment policy or system PATH was changed.

## Additional qualification preparations

The cloud-config OAuth test now uses the existing no-browser path, keeps its
manual-input pipe open while the HTTP callback completes, and requires stdout
EOF after direct-child exit as well as the existing success and credential
assertions. This fixture repair does not establish the source of the earlier
LEAK or later campaign silence. Independent source review found no new
lifecycle blocker. Scoped CLI Clippy/fix passed in10m29s. The corrected-launch
OAuth login/logout case passed, with the manual-input path observed and no
unsupported-host skip marker or reported leak.

The first local end-to-end import attempt compiled in18m39s, then stopped at
app-server startup before import: the private launch script's PowerShell null
environment setter left an empty OPENAI_FEDERATION_RULE_ID. A synthetic
non-secret variable reproduced present/empty after that setter and absent
after Remove-Item Env:. The launcher now removes and checks absence instead;
the original failed Nextest3995877f-7014-4ac0-bfb5-3efa3c21ba39/JUnit remains.
This failure does not test or refute the decoder repair. No real credential was
passed. The already-started first login attempt had the same startup failure
after18m55s build (Nextest3b28b888-288b-4438-8fad-c1d9317f32b2); it did not reach
the mocked service. Neither failed startup is attributed to application logic.

Corrected-launch results, with all existing test assertions retained:

- Import: Nextest `87878c7a-80a0-487d-8c7d-1373f3aa26ec`, one test passed in1.426s,
  no retries/skips for the selected case. JUnit SHA256
  `7f19f16dcb9e305d15c1ab0d937e6cb36beab0d9872b4ce9c754dab22d1e23f0`.
- Login/logout: Nextest `f5c8c755-249b-4ec6-bf69-8033eb649924`, one test passed
  in0.334s, no selected-case retries/skips or reported leak. JUnit SHA256
  `7038cffd0d00082ed72ed6d731958a571718a7364b5ba47e87a10a1d5889490d`.

Both capture receipts record root0 and capture-complete. The local Windows
checks do not establish all-platform support or the cause of the older LEAK.

The copied-state probe now refuses Windows Known-Folder system requirements or
config presence before copying and before each launch. This deliberately also
refuses benign policy until separately qualified; it does not parse secrets or
change policy. It removes identified inherited auth/workload/test-config markers,
pins File auth and trial SQLite/log paths, and requires trial auth/config absence.
All original18 synthetic tests plus9 isolation controls passed; after formatting,
the27 probe tests and12 capture tests passed together (39/39,18.425s).
These controls are not network isolation or protection against concurrent
machine-policy changes. The later actual copied-state rehearsal is recorded below.

The new test-only filesystem discriminator separates native fixture inspection
from the experiment/reporting module. It is ignored in ordinary runs and
explicitly refuses outside the named disposable hosted Windows environment.
It compares two fresh roots under the original temp ancestry: inherited
baseline and a verified actual-user full-access OI/CI baseline established
before children. Parent-before, helper-created and parent-after files each
exercise actual sandboxed follow=true/false overwrite/removal. Bounded owner,
DACL and operation/data/NotFound observations preserve failures; the final test
remains nonzero if required operations fail or an arm is incomplete. An allow
ACE is not AccessCheck, environment gating is not VM attestation, and no
helper-internal syscall stage or isolation repair is claimed. Main and
independent static review found no functional blocker; subsequent runtime results
follow below.

## Execution plan and boundaries

- Local Rust work is serialized: the first scoped lint/import/TUI checks above
  and CLI lint, plus both end-to-end cases, are complete. Native
  sandbox, desktop, account and ACL tests stay on disposable runners.
- The completed disposable workflow selected 24 cases in six serial groups:
  nine filesystem cases plus one pure diagnostic check, three legacy capture
  cases, and the original eleven elevated/account cases. Exact fresh inventories,
  JUnit identity checks and native/capture/report phase receipts stay separate.
- The workflow purpose branch is not a main-branch replacement. Exact source
  and workflow identities are above. No complete-workspace rerun was performed
  for this diagnostic campaign. A reported pass with an unsupported-host marker
  is not exercised native behavior.
- Keep the 80 GiB local floor, 84 GiB start margin, two build jobs, incremental
  disabled, non-forcing cleanup and no PID-based Rust termination. Initial free
  space was101.44GiB; latest post-focused-run observation98.419GiB. No installed
  selectors or real-home state is changed.

Private exact working evidence remains under
`codex-rs/target/desktop-dev/reports/`: `20261020-capture-discriminator.*`,
`20261020-capture-repair.md`, `20261020-capture-helper-review-fixes-003806/`,
`20261020-scoped-fix-01/`, `20261020-focused-cur-tui-01/`,
`20261020-scoped-39350468947/`, and `20261020-python-qualified-01/`.
The scoped-fix capture was launched before the final Python
console/cleanup hardening; do not use it alone as validation of those later
helper changes. Original October 18 artifacts remain unchanged.

The six exact hosted JUnit files and the checked triage account are retained
under [selected scoped-run evidence](evidence/2026-10-20-scoped-native/).
They are small outcome/source records, not test homes or credentials. The
triage account's proposed discriminator is historical planning; the current
test deliberately omits production grant-result/native-call hooks.

## Native baseline result and fixture-only repair

Run [39357755251](https://github.com/puntastic/CodexPersonal/actions/runs/39357755251)
used source `ed187364700f4c0e23aa2c2cf09d3bfdf2f07633` and workflow
`8137bc3323c32b60a11d80b112c6dca585e312f5`. Eleven exact cases ran: the original
diagnostics still3pass/7fail, and the intentionally failure-preserving
discriminator failed. Build/list/capture/report receipts were complete; no
selected skips, retries or JUnit errors occurred.

The discriminator retained67 bounded rows:35ACL,28operation,2scope,1prediction
and1summary;31,722 accounted bytes, largest row976, no missing operations or
incomplete arms. Both roots began with identical descriptors and Administrator
ownership, without an actual-user full-access OI/CI ACE. The comparison added
exactly that one allow ACE, preserved every prior ACE and ownership, and
verified its inheritance onto parent-seeded children.

| Ordinary fixture baseline | Parent-before/after overwrite and remove | Helper-created create/overwrite/remove |
| --- | --- | --- |
| Inherited | 8 failures, both follow modes | 6 successes |
| Qualified actual user | 8 successes, both follow modes | 6 successes |

Content/NotFound oracles independently confirmed these outcomes. The additive
baseline was sufficient in this ordered VM trial. This is not minimal-rights,
selected-token AccessCheck, helper-internal syscall, all-host or isolation proof.
The original seven cases subsequently passed their confirmation below.
Exact [original diagnostic output and JUnit](evidence/2026-10-20-filesystem-baseline/)
remain available; later mutability-only lint cleanup is not part of that CI pin.

The test-only repair establishes that ordinary-user baseline on fresh, empty,
owned roots before seeding the four source cases and checks child inheritance.
Shared cases apply it only to sandboxed Windows variants. All original oracles,
the ignored inherited countercase and production behavior remain unchanged.
The native helper moved to a shared fixture module; its previous native bodies
were retained. Main and independent source review passed, followed by full
formatting and scoped Clippy/fix (1m06s); no local native execution occurred.

## Unselected build path

The Desktop Build lane now accepts explicit Build-only
`-SkipLatestPackagePointer`. It keeps package validation, provenance and the
Build receipt but neither resolves nor updates the task convenience pointer.
Successful builds report `built_unselected`; actual build/source/receipt failures
retain their own failure. Default behavior is unchanged. Twenty mocked build/
router scenarios passed, alongside the timestamp (three cultures), Git-state
and non-forcing smoke-cleanup regressions. These checks did not build, deploy,
execute a package or touch live selectors.

The existing selected package was read-only verified again: version0.161.0,
source8399793e3f65d9c9d931da6a47a0c5a420faa128, six-file fingerprint
`5328848f19ce219673d8f9aa5a6fda77e9ac3ec6597719091f6e8908648f2301`.
Persistent and inherited entrypoint selectors agree; this is not attestation
of the loaded Desktop binary. No selector or package was changed. All five new
local test-temp roots were empty and removed without recursion/force; older
protected remnants and recovery evidence remain untouched.

## Candidate result and remaining adoption boundary

Canonical Build completed in2m55s from clean unchanged source
`3c9af802b8c8505fdd2d4e7c5828f0baec6b1667`, before/after Git-visible fingerprint
`506ad218df1e84ecf165cd0b36eaa7b082b3c7de38c6df73cfb3125b1766c0a9`.
Package `3c9af802b8c8-dev-small-20261020-unselected-local` reports0.163.0;
all six files were independently rehashed against fingerprint
`9c274537f15ed80aaffd896e5e48e518afa2687e62e91c49d9e2ef9c8bf39d2e`.
Build returned `built_unselected`, no pointer/receipt error, and verified the
task convenience pointer and persistent runtime selector unchanged. It was
not deployed. Private capture: reports/20261020-unselected-build-01.

### Copied-state rehearsal

The hardened fresh-copy rehearsal passed and was independently checked:
candidate0.163.0 -> previous0.161.0 -> candidate0.163.0 all exited0. Exactly
migrations59/60/61 were added; original ledger rows and logical evidence for
2,158threads/2,232attachments remained identical through all readbacks. Primary
and both companions passed integrity/file-identity checks; all nine recorded
untouched-backup DB/WAL/SHM hashes matched. No recovery backup was created.

Receipt SHA256 `8dd32ac304f15d7a520bea81c3216daa664b156c3c960db18ddad6dec79864b7`.
Private originals remain under
`%LOCALAPPDATA%/CodexPersonal/upgrade-probes/20261020-shared-runtime-3c9af802b8c8`;
capture reports/20261020-upgrade-probe-01. Do not publish those databases or the
full private receipt. Startup used isolated copies, not the original source
home. Read-only SQLite backup connections can still have sidecar/locking
effects; this is not an audit of concurrent live writers. Companion row
contents, model work, real-rollout replay, live Desktop behavior and general
rollback safety are outside the evidence. Local copies are not external
disaster-recovery proof.

### Original filesystem confirmation

Run [39362517655](https://github.com/puntastic/CodexPersonal/actions/runs/39362517655),
source3c9af802b8c8505fdd2d4e7c5828f0baec6b1667,
workflowf30fb035fffadd61083169919c3835023ccc9ccb, passed all original ten selected
cases, including all seven formerly failing variants. Nextest
`4980dc55-d87f-4aef-96d3-99788954a251`,5.156s; selected failures/errors/skips/
retries zero. Exact identity/hash/native/capture/report checks passed, and the
single-plan/results JSON retained array shape. Complete successful output had
no unsupported-token markers; those marked early-return branches were not taken.
The optional file-symlink branch in the junction test remains unobserved
separately. This is not a full-workspace rerun or a production isolation fix.

Exact [confirmation JUnit](evidence/2026-10-20-filesystem-baseline/confirmation-39362517655.xml),
2,465bytes, SHA256
`11301cffd9743e1f045162b27a6df5e9bdc05e8f9978966d031be69d5bb0f104`, is retained
beside its complete output/provenance and the earlier failed observations.
The ignored inherited countercase remains unchanged and was not selected for
this confirmation. The separately accepted legacy deletion limitation remains.

### Correction: exact old-reader source, not a version assumption

The earlier probe scope text singled out new V2 `retained_source` as an unknown
field that the older reader would discard before hashing. That claim is not
supported for the actual previous8399793/candidate3c9af802 pair: the previous
fork already declares the field, both complete `history/src/retained_source.rs`
files have Git blob `78ef1ca6ff2856f6c03c552ae9acb546c093465c`, and metadata
serialized fields plus the digest prefix/function block are unchanged. The
1,857-character LF-normalized digest block SHA256 is
`f936740c15166f9eab30837f097b6d9f9646a0eb6331b5e27343f0f5a5298d32`.
The probe now states its actual general codec/replay evidence ceiling without
that unsupported specific explanation. The original private receipt is retained
unchanged and must be read beside this correction;27 synthetic tests pass after
the scope-wording repair. The compiled candidate is not rebuilt for this Python
report-text/documentation correction.

Public paginated requests map to V1, while persisted/internal V2 modes remain
conditional paths. A read-only grouped-mode query on the mutable isolated
rehearsal copy (not the untouched backups or live source) found all2,158 stored
rows labelled `paginated`. This is not a universal future-mode/replay guarantee.
One separately scoped future codec discriminator could compare actual pinned
old/new libraries on a message with the newly serialized `status` field under
V2; see candidate `protocol/src/models.rs:1040`, `core/src/stream_events_utils.rs:93`
and `core/tests/suite/output_token_replay.rs:45` at the pinned3c9af802 source.
No such experiment was run or made an adoption gate here. Do not replace
the corrected warning with an invented blanket compatibility guarantee.

The targeted repairs/rechecks, clean candidate build and copied-state rehearsal
are complete. Installation/restart and live Desktop behavior remain separate.
No selector, live permission, backend or persistent delegate was changed.
Last free space92.309GiB; private recovery copies intentionally remain local.
The wider Ath1988 memory/recall goal remains unfinished.
