# CodexPersonal Windows Desktop development lane

Reach for `codex-dev.ps1` when local fork work needs the configured Windows
toolchain, a canonical Desktop package, a reversible activation, or proof of
which package the restarted app is actually using.

The lane deliberately reuses repository owners:

- Cargo, `just`, and the existing focused test recipes own development checks.
- `scripts/build_codex_package.py` owns builds and canonical package layout.
- this lane owns workstation discovery, immutable local staging, selection for
  the next Desktop restart, rollback, and receipts.
- Git still owns source checkpoints and publication. A successful deployment
  does not imply a pushed or merged commit.

## First contact

```powershell
.\codex-dev.ps1 -Action Doctor
```

Doctor reports the bounded Git-visible source state, discovered
Cargo/Rustup/Python/MSVC/Just/Bazel environment, separate build/format/deploy
capabilities, deployment-state consistency, persistent User-scope selector,
config mirror, inherited process-live startup snapshot, restart requirement,
and free space. It does not install or change anything. If it reports
`needs_tooling`, provision the repository-owned local
helpers into the ignored tool cache:

```powershell
.\codex-dev.ps1 -Action Setup -WhatIf
.\codex-dev.ps1 -Action Setup
```

Setup installs DotSlash through Cargo, uv through pip, and the same pinned
Bazelisk release used by CI under
`.tooling/repo-tools`; it does not modify the user PATH or require an
administrator shell. The resulting paths and versions are recorded in a local
receipt. Their download caches also stay under that ignored directory, so
sandboxed runs do not depend on writable user-level cache folders.
Bazelisk's workspace wrapper keeps Bazel's output-user root under the system
temporary directory rather than relying on Bazel's profile-root default or an
embedded-JDK path beneath a OneDrive checkout.

The workstation may keep downloaded tools under the ignored `.tooling/`
directory. Cargo's cache defaults to the existing `C:\tmp\codex-cargo` when
present, otherwise `%LOCALAPPDATA%\CodexPersonal\cargo`; both can be overridden.
The Rust version is derived from `codex-rs/rust-toolchain.toml` rather than
being copied into another script.

## Develop and qualify

Run existing `just` recipes inside the discovered MSVC/Rust environment:

```powershell
.\codex-dev.ps1 -Action Just -JustArguments @("test", "-p", "codex-state")
.\codex-dev.ps1 -Action Just -JustArguments @("fmt")
.\codex-dev.ps1 -Action Just -JustArguments @("bazel-lock-update")
```

The lane does not guess a crate or invent a validation matrix. Choose focused
tests from the changed surface and the repository `AGENTS.md`; a full suite
remains an explicit consequential choice.

On a Codex host with a managed filesystem boundary, Bazel-backed recipes may
need an approved unsandboxed run: Bazel's embedded JDK resolves its generated
install tree through a DOS 8.3 alias that the host may not equate with the
allowed long path. This is a host execution exception, not a reason to run
ordinary Rust recipes unsandboxed.

## Build

```powershell
.\codex-dev.ps1 -Action Build -CargoProfile dev-small
.\codex-dev.ps1 -Action Build -CargoProfile release
.\codex-dev.ps1 -Action Build -CargoProfile dev-small -RemoteControlAppServerVersion 0.153.0
```

`dev-small` is the normal iteration profile. `release` is available when the
optimization shape matters. Both use a locked native-host Cargo build to share
the existing target cache, then pass the resulting group through the canonical
package builder. A fresh ignored package directory, provenance sidecar, and
append-only receipt record source HEAD, Git-visible source fingerprint, tool
paths, manifest, binary hashes, and package fingerprint.

The lane captures source state before and after the long build. If it changed,
the package and receipt are retained and marked `source_changed_during_build`,
but the task's implicit deploy pointer is not advanced. Each Codex task has its
own last-stable pointer through `CODEX_THREAD_ID`; an ordinary shell uses the
clearly named `manual` pointer. Pass `-PackageDirectory` when intentionally
moving a package between tasks. An implicit Deploy always means the most recent
stable build for the current task, not necessarily the most recent attempt.
If updating that convenience pointer fails, the completed package remains
usable and the Build result returns `built_pointer_failed`, `PointerError`, and
an explicit Deploy command.

`-RemoteControlAppServerVersion` is a diagnostic compatibility override for
the version sent in a fresh remote-control enrollment and returned when the
remote backend initializes. It leaves the CLI, package, local-client, and
WebSocket protocol versions unchanged and records the selected value in
`codex-dev-build.json`.

This override is a validated stopgap, not the intended version-provenance
design. See [remote-control version compatibility](remote-control-version-compatibility.md)
for the evidence, design constraints, and deferred acceptance criteria.

## Deploy, restart, verify

```powershell
.\codex-dev.ps1 -Action Deploy
# Restart Codex Desktop.
.\codex-dev.ps1 -Action Verify
```

Deploy uses that task-scoped last-stable build unless `-PackageDirectory` is
supplied. It:

1. validates and smoke-runs the package with isolated `CODEX_HOME` and
   `CODEX_SQLITE_HOME` directories;
2. copies it into an immutable release under
   `%LOCALAPPDATA%\OpenAI\Codex\dev-overrides`;
3. backs up `~/.codex/config.toml` when its mirror must change;
4. aligns the existing config-mirror line and the User-scope environment
   `CODEX_CLI_PATH`; the User-scope value is the authoritative selector hydrated
   into the next Desktop process, whose startup reconciliation mirrors it back
   into config;
5. records the selected build occurrence, previous entrypoint, and a receipt.

Deployment and rollback share an exclusive deployment-root lock. Before the
selector/config/state seam, the lane writes a recoverable transaction record
containing persistent-selector Before/After values, both state snapshots, exact
config images, and the config backup when needed. A later Deploy or Rollback can
finish an unambiguous interrupted transaction; ambiguous pending state or plane
drift fails closed and is reported by Doctor and Verify. Pending is removed only
after exact final readback of all three durable planes.
This is a process/interpreter interruption guarantee, not a claim of durable
write ordering across sudden machine power loss.

One drift-shaped state is recoverable without guessing: both the persistent
selector and config mirror name the exact recorded `Previous` entrypoint while
state still names a different `Current`. Deploy can settle that observed
selection before continuing, and Rollback can settle it as the completed
rollback after validating the candidate. Settlement atomically swaps `Current`
and `Previous` in state without rewriting either selector or claiming a new
config backup. One-sided selector/mirror mismatch and all other ordinary drift
fail closed. Before the lane has state, a config-only mirror does not establish
launcher provenance and is never recorded as `Previous`; rollback becomes
available only after an authoritative User-scope selector or a managed selection
has been recorded.

Receipts are evidence, not the transaction owner. A completed setup, build, or
selection is not undone or reported as unperformed merely because its receipt
could not be written; the result carries `ReceiptError` so that evidence loss
remains visible.

It does not stop the app, mutate live SQLite state, delete a package, or claim
that a restart occurred. Use `-WhatIf` to inspect the plan. Verify exposes the
persistent next-launch selector, config mirror, and process-live startup snapshot
separately and reruns the isolated package smoke. Equality of that inherited
process snapshot is not attestation of which binary the GUI already loaded.

## Retention and disk use

Build packages, immutable releases, receipts, and config backups are retained;
the lane does not silently prune recovery evidence. Doctor reports counts and
bytes for each of those roots so disk growth is visible. Cleanup remains a
separate, deliberate lifecycle action: never delete the selected current or
previous release merely because it is old.

## Roll back

```powershell
.\codex-dev.ps1 -Action Rollback -WhatIf
.\codex-dev.ps1 -Action Rollback
# Restart Codex Desktop, then run Verify.
```

Rollback transactionally selects the recorded previous entrypoint in both the
User-scope selector and config mirror, and backs up config when its image changes.
For a lane-managed previous release it revalidates the canonical package,
fingerprint, executable targets, and isolated smoke before mutation. A
pre-lane selector without a recorded fingerprint remains an existence-only
fallback; Verify labels that weaker boundary as selector-only rather than
claiming canonical package proof. Rollback does not erase the failed package or
undo data migrations. Feature-specific data recovery remains with that
feature's tested migration/recovery path.

## Test the lane

```powershell
.\codex-dev.ps1 -Action SelfTest
```

The self-test uses disposable packages, config, and deployment state under the
system temporary directory. It exercises tool resolution and MSVC host
selection, config preservation, provenance, task-scoped pointers, all packaged
executable targets, WhatIf, selector/config/state transaction fault recovery,
two deployments, and rollback. Its persistent-selector adapter is in-memory and
the tests assert that the real User-scope `CODEX_CLI_PATH` is unchanged.
