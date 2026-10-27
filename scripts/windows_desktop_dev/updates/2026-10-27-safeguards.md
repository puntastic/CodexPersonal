# Desktop safeguards — 27 October 2026

This builds the three safeguards appraised in Ath2714: a genuinely nonmutating
inspector, exact Desktop/reader compatibility admission, and an independent
recovery kit. It changes the operator lane, not the running Rust binary.

## What is built

- `Inspect` and the standalone inspector separate the User selector, inherited
  environment, exact node_repl mirror, deployment ledger, registered Desktop and
  actual direct CLI children. No CLI execution, locks, toolchain initialization,
  remote calls, auth/history payloads or config writes.
- The config binder is structural and consumer-specific. Other same-named keys,
  comments and multiline examples cannot become the selected mirror. Unsupported
  or ambiguous forms fail visibly; this is not a complete TOML validator.
- `CheckCompatibility` uses exact reviewed Desktop archive and reader identities,
  then exercises strict startup, real local policy propagation and typed config
  failure in fresh file-only homes. It refuses machine-wide policy rather than
  overriding it. Deploy/Rollback use it automatically; WhatIf does not execute it.
- Admission binds the executable to its package, qualifies actual pending-recovery
  targets and refreshes Rollback's preview under locks. Final checks catch package,
  Desktop, policy, source, receipt and deployment-state changes.
- A new-folder recovery builder copies checked inspection code and readable
  instructions outside app caches. The default launcher only inspects. Optional
  terminal guidance prints an exact command, never starts or resumes a reader.

## Evidence

Focused tests on Windows PowerShell 5.1 and PowerShell 7.6.6:

- 66 config-binding assertions;
- 40 inspector/recovery-kit assertions;
- 159 compatibility-bridge assertions;
- 145 admission assertions;
- 20 Python tests, including actual inert child timeout/output-limit checks.

The complete existing PowerShell development-lane SelfTest passed on PS7,
including transaction interruption/compensation, staging retention, environment,
build-pointer and package tests. Mechanical transaction suites deliberately mock
compatibility for fake PE fixtures; the separate admission and native checks do
not pretend those fixtures are working readers. No Rust code changed or Rust
suite was rerun. Independent reviews found and prompted repairs for path binding,
machine policy, registry parse/hash consistency, typed failures and recovery
target/receipt binding.

Actual Windows PowerShell 5.1 qualification against registered Desktop
26.1021.3705.0 (ASAR04862a659c6a90fc50514d8c0b8016709d97dece58839726c57495ed72f1d9f0):

- Current package44f3a4bbd513aacae804b8b9b4c95844c399355273b105bc57dff99d41402f28,
  exe d73b93502e6b542f972766f2096566ffd6f1c75a23eeaa86a9f308d3648a2300:
  all three startup fixtures passed; final bound-receipt assertion passed.
- Old package9c274537f15ed80aaffd896e5e48e518afa2687e62e91c49d9e2ef9c8bf39d2e:
  first startup command failed at argument parsing, exit2, as expected. Both
  packages advertise0.163.0; version equality does not establish compatibility.
- Before/after selected config, User/inherited selectors, ledger, Desktop and
  selected package bytes matched. No Deploy, Rollback, restart, sign-out, real
  thread resume, live authentication or session payload inspection occurred.

Retained local verification receipt SHA256:
`639ae0758b824933dd1f4b5629844eaa444b5b520a888011d64504311eda2301`.
Ath's delivery account carries its operator path; private receipts are not copied
into this repository.

The native PS5 run exposed culture-dependent filename sorting in package hashes.
Ordinal sorting now matches the existing qualified package identities on both
shells. A historical package produced under another collation must be appraised;
do not silently rewrite its provenance to pass this check.

## Deliberate limits

Recorded history-reader qualification is not a new complete codec test. The
recovery build still lacks native provenance, as the preceding recovery receipt
records. These startup fixtures do not establish authenticated policy, whole live
config acceptance, actual thread resume, network isolation, process-tree
containment or all project behavior. The kit does not copy the binary or provide
off-machine backup. The current ledger's Previous=null is not ordinary Rollback.

No automatic monitor, updater interception, update-policy/security change or live
reader replacement is included. External Desktop updates can still change the
contract; the new lane gives a concrete check and an independent recovery route,
not a promise that an external updater can never break compatibility.
