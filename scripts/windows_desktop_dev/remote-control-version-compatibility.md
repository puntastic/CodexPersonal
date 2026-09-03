# Remote-control version compatibility

Status: deferred design work. The compile-time override described below is a
validated stopgap, not the intended long-term version-provenance model.

## Why this note exists

A source-built Codex Desktop environment was visible to ChatGPT mobile Remote,
but the phone rejected it with “Codex on this environment is out of date.” The
underlying Remote transport was working: the backend connected and sent exactly
one JSON-RPC `initialize` request, then sent no later request during the next two
minutes. The response exposed the source workspace version, `0.0.0`.

The source workspace deliberately uses `0.0.0` in
`codex-rs/Cargo.toml`. Ordinary Cargo builds therefore embed `0.0.0` through
`CARGO_PKG_VERSION`, even when the source is compatible with a current Desktop
release. `scripts/codex_package --package-version` changes
`codex-package.json`; it does not change the version compiled into an already
built binary. `codex-rs/default.nix` demonstrates a separate release precedent:
it stamps the workspace manifest before compilation.

## Evidence from the 2026-09-03 probe

1. Commit `e8aad2746` added a compile-time
   `CODEX_REMOTE_CONTROL_APP_SERVER_VERSION` override to fresh enrollment and
   exposed it through the Windows Desktop development lane.
2. A build advertising `0.153.0` in enrollment alone was accepted by the
   service, but the phone still reported the environment as out of date.
3. Live logs showed that the phone/backend reached `initialize`; its response
   still carried `0.0.0`, and no subsequent request followed.
4. Commit `cebda8fd7` applied the same override to the initialization response
   only when `ConnectionOrigin::RemoteControl` supplied the connection. A
   caller-provided name is deliberately not trusted for this decision.
5. The corrected package advertised the exact version bundled with the
   installed official Desktop application, `0.153.0-alpha.5`, on both remote
   surfaces. After a Desktop restart, ChatGPT mobile Remote opened the task and
   successfully sent a message.
6. The deployed build is `cebda8fd7216`, build ID
   `588ee13e9858483fbc70619a1655f7e3`, artifact fingerprint
   `5466a89d79ae67b741b4e2a1339f83d98d6702b9bfc35aafe9eab2dc9dc1f00d`.
   The managed deployment verifier reported it live, consistent, and
   rollback-capable.

This proves that the combined compatibility advertisement is sufficient on
that build and account. It strongly localizes the rejection to remote-facing
version presentation around initialization. It does **not** establish whether
the phone or an intermediary service performs the comparison, nor whether the
decisive difference was the added initialization version, the exact
`-alpha.5` value, or both. Enrollment caching and re-enrollment requirements
also remain unknown.

## What the stopgap changes

`-RemoteControlAppServerVersion` is carried into the build as
`CODEX_REMOTE_CONTROL_APP_SERVER_VERSION`. One shared constant currently feeds:

- `app_server_version` in remote-control enrollment; and
- the version portion of `InitializeResponse.userAgent` for connections whose
  actual origin is `RemoteControl`.

The override intentionally does not change:

- `codex --version` or the Cargo/package build identity;
- local, stdio, in-process, WebSocket, or daemon initialization responses;
- the remote WebSocket protocol version; or
- SQLite state or enrollment ownership.

The origin boundary matters. An earlier draft keyed the override to the
caller-supplied name `codex-backend`; review caught that a local caller could
spoof the name while a differently named remote caller could miss the
override. The committed implementation threads `ConnectionOrigin` into
initialization and has positive and negative regression tests.

## Proper-fix objective

Make a normal source build present one truthful, inspectable compatibility
identity automatically. Its provenance should be recorded with the build, and
the remote enrollment and remote initialization surfaces should agree without
requiring an operator to copy a version string by hand.

### Existing implementation seam

Do not begin by inventing another version provider. The repository already
contains `codex-build-info`, a currently unused typed owner that resolves a
packaged runtime SemVer from the `codex-package.json` adjacent to the running
executable while retaining a stamped Git commit for provenance. The canonical
package builder already accepts `--package-version` and writes that manifest.

The strongest local implementation path to evaluate first is therefore:

1. make the Windows Desktop development lane choose and record an authoritative
   package or compatibility version and pass it to `--package-version`;
2. initialize/read `codex-build-info` from the packaged executable;
3. feed that typed identity into remote enrollment and remote-origin
   initialization; and
4. retain the Git commit and selection rationale in the existing build and
   deployment receipts.

That path avoids patching `Cargo.toml` and avoids treating
`CARGO_PKG_VERSION=0.0.0` as the only runtime identity. The unresolved design
question is what makes the selected package version authoritative for a
personal source build. A version discovered from an installed official bundle,
a fork-maintained tested compatibility baseline, and an upstream release tag
carry different meanings and freshness behavior; the lane must record which it
used rather than flatten them into one unexplained string.

`BuildInfo` uses a process-wide `OnceLock`. Commit-aware initialization must
therefore happen before any lazy `BuildInfo::get()` call; otherwise the version
can still resolve from the package manifest, but build-commit provenance is
frozen as `dev` for that process.

There are two plausible design directions to resolve rather than blur:

1. **Explicit protocol/capability compatibility.** Give Remote an app-server
   protocol or capability version independent of product SemVer and gate on
   that. This is semantically preferable, but probably requires service/mobile
   support outside this repository.
2. **Packaged-build compatibility identity.** Derive a client-side compatibility
   version from an authoritative upstream or official Desktop baseline, record
   how it was chosen, and resolve and supply that identity to the narrowly
   relevant surfaces. This can be implemented locally, but it must not turn an
   unverified source build into a global product-version masquerade.

Whichever direction is chosen should reuse or deliberately extend
`codex-build-info` as the single typed owner for build and compatibility
metadata rather than add independent environment-variable reads.
The package manifest, enrollment request, initialization response, deployment
receipt, and diagnostics should either consume that owner or state explicitly
why a surface carries a different identity.

## Deferred investigation

Use a small matrix to separate the two variables left coupled by the successful
probe:

| Enrollment | Remote initialize | Purpose |
| --- | --- | --- |
| exact `0.153.0-alpha.5` | source `0.0.0` | Is initialization advertisement required? |
| stable `0.153.0` | exact `0.153.0-alpha.5` | Does enrollment require the exact prerelease? |
| exact `0.153.0-alpha.5` | exact `0.153.0-alpha.5` | Reconfirm the known-good control. |

Repeat only the cases that can change the design, and distinguish a fresh
enrollment from an ordinary reconnect. Inspect any public service contract or
current upstream implementation before choosing how prerelease versions and
minimum-supported versions should compare.

## Acceptance criteria

A replacement is ready when:

- an ordinary Windows development-lane build needs no manual
  `-RemoteControlAppServerVersion` argument;
- selected compatibility metadata and its source appear in the build and
  deployment receipts;
- enrollment and remote initialization agree by construction;
- a genuine remote-origin connection receives the compatibility identity while
  local and daemon paths retain their correct identity, regardless of client
  name;
- phone Remote works after fresh enrollment, reconnect, and Desktop restart;
- local CLI update behavior, daemon restart selection, and WebSocket protocol
  handling do not regress;
- rollback remains inspectable and requires no SQLite manipulation; and
- tests cover prerelease syntax, missing or invalid metadata, the remote-origin
  positive path, and local-name-spoof negative paths.

Retain the current override as an explicit diagnostic instrument until a
replacement meets those checks. Do not silently promote the successful
compatibility claim into permanent global version truth.

## Source anchors and recovery

- Enrollment/version owner:
  `codex-rs/app-server-transport/src/transport/remote_control/server_api.rs`
- Remote initialization selection:
  `codex-rs/app-server/src/request_processors/initialize_processor.rs`
- Connection-origin plumbing:
  `codex-rs/app-server/src/message_processor.rs` and
  `codex-rs/app-server/src/lib.rs`
- Integration and negative tests:
  `codex-rs/app-server/tests/suite/v2/remote_control.rs` and
  `codex-rs/app-server/tests/suite/v2/initialize.rs`
- Windows build provenance:
  `scripts/windows_desktop_dev/package.ps1`
- Canonical package-version behavior:
  `scripts/codex_package/README.md` and `scripts/codex_package/version.py`
- Existing packaged runtime identity:
  `codex-rs/build-info/src/lib.rs`
- Manifest-stamping precedent: `codex-rs/default.nix`
- Adjacent public evidence: <https://github.com/openai/codex/issues/23527>

The current managed rollback target is the package selected immediately before
fingerprint `5466a89d79ae67b741b4e2a1339f83d98d6702b9bfc35aafe9eab2dc9dc1f00d`.
Use the development lane's `Rollback`, `Doctor`, and `Verify` actions rather
than editing selectors or state databases by hand.
