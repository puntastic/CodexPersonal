# If Desktop will not open, start here

This kit gives you a way to inspect the environment without opening GPT.
It does not reset your settings, sign you out, launch another Codex instance,
rewrite history, or choose a different reader for you.

## First: find out what is selected and what is actually running

Open **Start here.cmd**. The window stays open so you can read the result.
The first lines separate the User selector, this shell's inherited selector,
the node_repl mirror, and the deployment ledger. Those are different surfaces.
Observed loaded CLI children are listed separately; settings alone do not prove
which binary Desktop loaded.

If Windows refuses the script under your execution policy, leave that protection
in place and ask your usual maintainer to inspect this kit's checked source.
If the Desktop app is inaccessible, Fae in the browser can still be given this
folder's instructions and any inspection output you choose to share.
Do not delete `.codex`, discard credentials or replace history mode labels to
make an error disappear.

**Unstable** means something changed while it was being read. Stop and let the
other writer finish, then inspect again. A pending journal means a transaction
needs a deliberate recovery decision, not a blind retry.

The inspection reads local metadata and selected file hashes. It does not
contact remote services. Appx metadata may use the built-in Windows PowerShell
reader if the calling shell cannot load Appx. Neither path executes Codex.

## If you need to hand the result to someone

Run this from PowerShell in the kit folder:

```powershell
.\recovery-start.ps1 -Json
```

The output omits full config contents, command lines, authentication data and
session payloads. It still contains local paths and process identifiers: share
within the project deliberately. It is a point-in-time observation, not proof
that startup, thread resume or all project features work.

## A terminal fallback is a separate choice

Only if you deliberately choose that route, run:

```powershell
.\recovery-start.ps1 -ShowManualLaunch
```

That checks the recorded package and executable hashes and **prints** its exact launch command;
it does not execute it. Quit Desktop and other Codex writers normally first.
Do not assume two processes can safely write the same home. A terminal opening
does not establish that the affected thread can resume. The current reader
must genuinely support the recorded history/checkpoint format; an enum rename
is not a migration.

The kit's `recovery-kit.json` identifies the approved package, its hashes and
the evidence locations used when this kit was made. Those locations may later
change. The package stays at its original path: this is not an offsite backup,
a copied binary, or a guarantee that a later Desktop update is compatible.
No Previous ledger entry, including null, is automatic rollback permission.
