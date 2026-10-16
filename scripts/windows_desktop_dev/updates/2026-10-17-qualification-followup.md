# Remaining Windows qualification after the shared-runtime repair

This is a continuation hand, not release or deployment approval. The owning
arc and historical package are in `2026-10-16.md`. The running Desktop remains
0.161.0. Do not replace it merely because a source branch or test run exists.

## Checked result and resource boundary

The checkpoint/backup-generation repair and explicit Guardian fixture modes
passed 93 focused checks, without skips or retries, in nextest run
`0c0977f9-6341-4403-8bf5-fe8319e51a10`. That includes actual backup eviction and
delta reuse, source-change countercases, retained restrictions, root reset and
the test-local 8 MiB stack adjustment. Independent source review found no
concrete defect in that bounded reuse repair; it was not a concurrency proof.

The next batch completed 14,637 cases outside Core/app-server and the separately
assigned account-mutating tests: 14,553 passed (six after retry), 84 failed.
Run `d017e306-63d9-4a08-8737-c6dd8134efa2` and its saved JUnit in
`codex-rs/target/desktop-dev/reports/20261017-workspace-other-first.junit.xml`
own the exact failures. Do not add overlapping earlier runs as unique coverage.
The complete current-source Core/app-server batches remain unrun.

Kestrel's free-space floor is 80 GiB; costly phases use an 84 GiB early-stop
margin. About 85 GiB remained after that batch. Cleanup is non-forcing only.
The second owned temp root is `C:/tmp/codexfull20261017b`; an attempted
non-forcing cleanup script was rejected before execution. Leave its contents
and protected residue alone unless a permitted cleanup path is established.
Never use another API, permission/attribute changes or a policy edit to bypass
that refusal. Preserve the reports, source patches and recovery artifacts.

## TUI: 65 failures, no snapshots accepted

All 38 pending snapshots were read during triage. The 65 failures group as:
34 version-dependent artifacts, four color expectations, six geometry cases,
19 stack overflows, one contradictory Daybreak fixture, one analytics timeout.
These are source-based classifications and proposed repairs, not passing reruns.

### Stable display fixtures without changing version policy

- For 26 title-bearing cases, use an opt-in test helper matching only the exact
  displayed `OpenAI Codex (v{CODEX_CLI_VERSION})` title. Existing funnels are
  `tui/src/status/tests.rs`'s `sanitize_directory` and the normalizer in
  `tui/src/app/tests/realtime_requests.rs`; three other title assertions remain
  in fresh-thread, replacement and startup-tip fixtures. Do not normalize
  server versions, warnings or arbitrary version-like text. Post-render
  substitution does not erase genuine wrapping differences.
- The two update-dialog fixtures can set the existing `current_version` field
  in `tui/src/update_prompt.rs::tests::new_prompt` before layout.
- Three bordered update-history fixtures need a local version input on
  `UpdateAvailableHistoryCell` (`history_cell/notices.rs`). Production startup
  passes the real version; its three tests and animation fixture pass their
  explicit display version. Inspect all five constructor calls; do not perform
  post-render border surgery.
- `startup_draft_tests.rs` already trims row padding before version substitution.
  Apply that convention to `startup_draft_layout_tests.rs`, preserving cursor
  and rectangle checks, or use the existing session-header version input.
- Daemon confirmation can normalize its exact displayed package-version token
  only after asserting the actual manifest/CLI selection. The manifest can
  legitimately supply a different version.
- `active_reconnect_tests.rs` can pass explicit `"0.0.0"` to the already
  parameterized `finish_reconnect` fixture. Keep the mismatch warning itself;
  existing `update_versions.rs` and daemon-menu tests cover release semantics.

Leave `tui/src/version.rs` and production version reporting unchanged. A global
test version constant would conflate display fixtures with policy behavior.

### Other TUI groups

- Four cursor cases ran with `NO_COLOR=1`; production deliberately suppresses
  colors in that mode. Establish explicit colored/no-color fixture contracts,
  retaining glyph, hyperlink, cursor-position and style assertions.
- `tui/test_support.rs::make_test_tui` fixes its fake backend to 80x24. Tests
  resize only cached geometry, while `tui/screen_size.rs` re-queries the backend
  unless event geometry is reserved. Synchronize simulated backend and draw
  sizes before judging the three out-of-bounds panics, two lost-content
  snapshots and missing `esc latest`. Do not accept missing history rows.
- Nineteen stack aborts establish exhaustion, not recursive call chains or a
  production diagnosis. Test an isolated stack/layout hypothesis before broad
  boxing or a global stack increase. Production startup already boxes its
  large future; existing fixtures use explicit 8/16 MiB stacks.
- `status_and_layout.rs` enables `CliDaybreak` then expects the first branch to
  act as disabled. Set up that negative branch explicitly and retain the later
  enabled path.
- `analytics/account_layout_tests.rs` creates eleven embedded servers in one
  test. Both attempts timed out. Parameterize by plan and bound startup/cleanup
  to distinguish cumulative cost from a stalled plan before raising timeouts.

## Non-TUI: 19 failures

- Exec: four timeouts remain unexplained; one resume fixture expects
  `paginated` rather than the fork's `paginated_refs_v1`. Preserve the actual
  resume/history assertions when updating a representation expectation.
- Exec-server: two fixtures invoke Unix `true` on Windows. Use an appropriate
  successful native command; this is not evidence the executor cannot launch
  valid Windows programs.
- TLS: five alert fixtures returned Schannel `0x80090326`; the deliberate
  fallback recognizes `0x80090302`. The emitted bytes are a protocol-version
  alert. Resolve the Windows-version/fixture discriminator before expanding
  classification. Generic handshake failure must not become a blanket route
  around certificate, hostname or revocation errors; retain non-replayable
  request and destination/route cache tests.
- Private-socket ACL checks: a paired read-only child-process probe reproduced
  the module import failure with inherited PowerShell 7 `PSModulePath` and
  succeeded when only the child's module path was omitted. Windows PowerShell
  then loaded its own security module. The original two tests have not been
  rerun. Do not change host permissions or execution policy.
- V8: current Cargo metadata shows `code-mode-runtime` enabling the shared
  dependency's `v8_enable_sandbox` feature, while `v8-poc`'s own `sandbox` feature
  is off. The runtime reports an enabled sandbox, consistent with the unified
  dependency graph. The proof-of-concept assertion wrongly equates its own
  feature with the aggregate graph in this run. Do not disable the sandbox to
  make that oracle pass; preserve isolated feature/build coverage separately.
- Four legacy Windows sandbox tests failed around descendant startup, output,
  cancellation or deletion behavior. Their cause remains open. Do not assign
  them the PowerShell explanation without a discriminating check.

## Completion path

Retain the checked production repair separately from these proposed fixture
repairs. Qualify each coherent group, preserve negative cases and source
identity, then complete the remaining safe workspace coverage with adequate
headroom. Account-mutating tests belong on the disposable Windows runner;
voice-host still needs its separate matching development SDK qualification.
A new clean package and the established installation/restart boundary follow
source settlement. No persistent memory-helper instance or live peer grant
has been created by this test campaign.
