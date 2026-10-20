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
   selected Cursor tests passed locally on Windows. The separate app-server
   end-to-end import case remains to run; POSIX-specific branches were not run.
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
lifecycle blocker. Scoped CLI Clippy/fix passed in10m29s; runtime qualification
remains in progress.

The first local end-to-end import attempt compiled in18m39s, then stopped at
app-server startup before import: the private launch script's PowerShell null
environment setter left an empty OPENAI_FEDERATION_RULE_ID. A synthetic
non-secret variable reproduced present/empty after that setter and absent
after Remove-Item Env:. The launcher now removes and checks absence instead;
the original failed Nextest3995877f-7014-4ac0-bfb5-3efa3c21ba39/JUnit remains.
This failure does not test or refute the decoder repair. No real credential was
passed. Repaired-launch runtime results must be added separately.

The copied-state probe now refuses Windows Known-Folder system requirements or
config presence before copying and before each launch. This deliberately also
refuses benign policy until separately qualified; it does not parse secrets or
change policy. It removes identified inherited auth/workload/test-config markers,
pins File auth and trial SQLite/log paths, and requires trial auth/config absence.
All original18 synthetic tests plus9 isolation controls passed; after formatting,
the27 probe tests and12 capture tests passed together (39/39,18.425s).
These controls are not network isolation or protection against concurrent
machine-policy changes. No current real DB copy or candidate rehearsal is claimed.

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
independent static review found no functional blocker; runtime is pending.

## Execution plan and boundaries

- Local Rust work is serialized: the first scoped lint/import/TUI checks above
  and CLI lint are complete; two end-to-end cases remain. Native
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

This is a continuation checkpoint, not completed qualification or runtime
adoption. Add actual returns and exact committed identities when available.
