# October 27 startup-policy recovery

The repaired custom reader was selected, loaded by Windows Desktop, and used to
resume Coordinator. A subsequent resident check successfully exercised ordinary
scoped work. This dated receipt distinguishes those observations from full
runtime qualification, source publication and future package builds.

## Exact source and package

| Identity | Recorded value |
| --- | --- |
| Repair branch | `codex/startup-policy-20261027` |
| Shared-runtime base | `032de5589318bc382247299d3fe3e3459b1ea68d` |
| Implementation checkpoint | `4baa1c04a2c44a6b3504dc380ede37401087fae7` |
| Compiled source, including integration tests | `d018eebe0dca9695899f160fc49dd3f67f1685de` |
| Compiled source tree | `a52f26d7da6f8874b86eaeac03284803791ec5f3` |
| Package version / target | `0.163.0` / `x86_64-pc-windows-msvc` |
| Package fingerprint | `44f3a4bbd513aacae804b8b9b4c95844c399355273b105bc57dff99d41402f28` |
| Entrypoint SHA-256 | `d73b93502e6b542f972766f2096566ffd6f1c75a23eeaa86a9f308d3648a2300` |
| Observed Windows Desktop package | `26.1021.3705.0` |

The candidate was built from a clean worktree with identical Git-visible
HEAD/tree/status before and after packaging. The canonical direct builder
produced the six-file native package, with an external receipt recording the
command and individual artifact hashes. It did **not** produce a native
`codex-dev-build.json` sidecar. Native package provenance therefore remains
`unavailable`; the external receipt is useful evidence, not a substitute native
certificate. It does not attest ignored files or transient intermediate edits.

Documentation and integration may advance branch HEAD after this compiled
checkpoint. That does not rebuild, re-identify or requalify the installed
package. At the operating check, the clean candidate was exactly two commits
ahead of clean `codex/shared-runtime-20261016`; neither a push nor an integration
had yet been performed. Use the checked shared-runtime lane for the scoped
repair, not an automatic merge into the much older `main`. In this checkout,
`personal` is the project fork (`puntastic/CodexPersonal`); `origin` is upstream
(`openai/codex`). Recheck refs and authority before publication.

## Failure and repair boundary

The generic organization-settings warning did not identify its own cause.
The installed Desktop source required this one-shot startup command shape:

```text
app-server --analytics-default-enabled --print-startup-config --startup-client-info <JSON>
```

Sign-out adds `--startup-sign-out`. The client expects exit zero and a single
JSON object containing `appServerVersion` and `configRequirements.requirements`
(which may legitimately be null). The previously selected custom source lacked
these flags. Existing logs showed `phase=policy-read reason=protocol`, but did
not retain the precise subprocess error. The source-contract mismatch is
concrete; the exact original parser error remains unobserved.

Temporarily selecting stock exposed two further compatibility boundaries: it
rejected the custom nested cue table, then rejected `paginated_refs_v1`. That
history format needs reference-backed checkpoint resolution. Renaming the enum
to a stock value would not implement that resolution and was not attempted.
The repair adds startup policy, client identity, structured errors and
primary-auth sign-out using real configuration/authentication policy. It does
not invent unrestricted policy, change history implementation, migrate session
payloads or relax the custom reader's fail-closed reference checks.

## Checks and actual adoption

| Check | Observed result and scope |
| --- | --- |
| History/checkpoint tests | 79 passed; synthetic reference resolution, failure and reload cases |
| App-server startup tests | 8 passed; 389 filtered |
| CLI startup/parser tests | 11 passed; 315 filtered |
| Scoped lint and formatting | Passed; individual receipts retain their actual source/check ordering |
| Package build and isolated smoke | Passed; fresh credential-free home, exact Desktop preflight and strict custom-cue configuration |
| Reconciliation rehearsal | Ten recovery cases and independent review before approved application |
| Post-restart identity | Expected package path and executable file hash matched both CLI children of Desktop |
| Startup | Both initialization handshakes succeeded at 04:01:58.140 and 04:02:02.310 UTC |
| Coordinator resume | `thread/resume` succeeded at 04:04:22.177 UTC after 88.529 seconds; prior messages and composer visible |
| Resident operating check | 04:27-04:39 UTC: local commands, connected project-library reads, app chat discovery and a read-only subagent exchange succeeded |

The 98 focused tests were not all rerun at the final source checkpoint: the
history and app-server runs preceded later test-only cleanup; final formatting
and CLI checks followed. The build and smoke used the final compiled source.
Keep this sequence rather than claiming every check tested identical bytes.
No live sign-out was tested. Process path plus the file hash is not mapped-memory
attestation. Successful resume is not a comprehensive historical-integrity
audit, and the resident tool checks do not establish all project behavior.

One later `thread/read` briefly returned `thread_not_found`; the immediately
following read succeeded. The primary conversation remained visible. Preserve
that recovered transient as an observation, not a diagnosed or repaired cause.
The existing paused goal stayed paused; no new paid computation was started.

## Selector reconciliation is not ordinary rollback

The normal guarded deployment preview correctly stopped at
`durable_selector_mismatch`: the User selector was absent, the config mirror
selected stock, and the ledger still recorded the prior custom package. The
recovery did not bypass that guard through a blind retry. A narrowly journaled,
owner-approved reconciliation preserved actual before-images, selected the
exact approved package, and left the unchanged guard reporting `already_selected`
with no pending transaction.

`Previous=null` records the actual absent User selector before recovery. It does
not mean the previous custom package is a tested rollback target, and it does
not create an ordinary selector rollback pair. Preserve the recovery receipt,
saved configuration/state and both historical package directories. Any reversal
needs a separately checked choice that accounts for the current Desktop and
custom history. The old custom packages lack the startup contract and stock
lacks the custom history reader; selector rollback is not data rollback.

The recovery packet retains a task-owned null-preserving adapter used during
reconciliation. This receipt does not install it as a new general deployment
mechanism or certify arbitrary future mismatch recovery.

## Configuration and operating limits

The four cue-activation lines remain commented, including the former
`enabled=false`, advisory mode and catalog setting. At the compiled source,
`CueActivation` defaults false (`codex-rs/features/src/lib.rs`), and
`resolve_cue_activation_config` returns `None` while disabled
(`codex-rs/core/src/config/mod.rs`). If a different effective layer enables it
without the table, its defaults are Shadow mode and no catalog. No full
effective-layer audit was performed. There is no reason to uncomment or partly
restore the table as a recovery ritual.

At the resident check, User/inherited selectors, the config mirror and ledger
agreed. A changed config hash was inspected rather than blindly reverted: the
parsed differences from the saved before-image were the intended CLI selector,
the native pipe directory and an added priority service tier. The later writer
of the latter two was not established; other parsed settings matched. The
resident check made no configuration, package or history changes.

The exercised environment supports ordinary scoped continuation. Audio/video
perception, every connector, long jobs, live sign-out, comprehensive historical
integrity and fresh phone Remote were not qualified here. Retain those limits
alongside successful startup rather than treating either as a universal verdict.

## Evidence custody

The project operator account is Ath2696, with the resident check added in r5;
Ath2695 retains the separate operational-knowledge maintenance item. The exact
local recovery packet, retained outside this public source tree, includes:

- `build-support/compatibility-evidence-report.md` and
  `build-support/logs/candidate-package-20261027-025303013-535e197c.json`;
- the associated focused-test, package-smoke and reconciliation-rehearsal receipts;
- `deployment-support/reconciliation/recovery-20261027-033717239-db6a4a52c3454e87a80d1c65232318b8/recovery-receipt.json`
  and its preserved before-images;
- `deployment-support/post-restart-outcome.json` and
  `deployment-support/coordinator-operating-check.md`.

The preparation report's original not-deployed statements are dated preparation
evidence, superseded for adoption status by the later deployment and restart
receipts, not retroactively edited. Raw configuration, session content, account
data and machine-specific identities are not copied into this repository.
Compiled environment views and their reconciliation are separate knowledge-store
artifacts; publication of one is not package, backup or live-state verification.
