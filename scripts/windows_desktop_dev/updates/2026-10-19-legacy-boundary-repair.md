# Legacy Windows deletion boundary - bounded repair arc

Kestrel authorized this repair on 19 October (Australia/Brisbane), after the
[qualification return](2026-10-18-qualification.md) demonstrated actual outside
and protected-directory deletion in a disposable Windows fixture. Source starts
at `4fb9f05bca840c2f28f658cfc7cb83ebc96bad1e`. Ath2368r5 retains the current
evidence and exact original custody.

## Goal and boundaries

Restore the legacy backend's promised write boundary without making ordinary
read/execute access or permitted workspace work silently unusable. Preserve the
actual caller's ordinary full-access baseline, including parent
`FILE_DELETE_CHILD`, and the existing deletion oracle. Blocking every operation
or preventing process startup does not qualify a repair.

This is source development and bounded disposable-VM testing. No local native
ACL/security trial, live backend selection, permission change, installation,
restart, account provisioning, upstream outreach or adoption is included.
The elevated backend's distinct account/object contracts must not change merely
because it shares a token constructor. Local compile/format/lint respects the
80 GiB free-space floor, 84 GiB start margin, two jobs and no incremental output.

The goal would need reframing if preserving unrestricted ordinary reads and
enforcing arbitrary write boundaries cannot coexist in the legacy mechanism
without substantial new privilege, broker or host-wide ACL machinery. Such a
result is not permission to choose another backend or conceal a capability loss.

## Source-grounded alternatives, not adopted fixes

The production constructor currently combines `DISABLE_MAX_PRIVILEGE`,
`LUA_TOKEN`, `WRITE_RESTRICTED` and restricting entries for capabilities,
additional entries, the logon SID and Everyone. Elevated callers additionally
include their dedicated account SID. Legacy entry points explicitly require
full-disk read access and reject deny-read overrides.

| Candidate | Possible gain | Material counterpressure |
| --- | --- | --- |
| Remove broad logon/Everyone restricting entries only | Avoid an outside object's broad ACE satisfying the restricting check | The earlier capability-only synthetic observation still permits parent delete-child under write restriction; default-object/IPC compatibility must also survive. |
| Remove `WRITE_RESTRICTED` only | Apply the restricting check to rights currently outside the write classification | Broad restricting identities can still match broad outside grants; user-private reads may fail. |
| Full restriction with capability-only entries | Strongest of these token-only isolation candidates | Required outside reads/execution and logon-scoped process/IPC access may fail. |
| Broader mediation or a different backend | Potentially separate arbitrary reads from authorized writes explicitly | New architecture, privileges, provisioning and adoption burden; not an automatic fallback in this bounded repair. |

Do not choose by a green deletion tuple alone. The same-source compatibility
and negative controls must answer back.

## Next actions and earned frontier

1. **Same-VM mechanism discriminator.** Add a test-only in-memory descriptor
   matrix in the existing token test home. Four fresh tokens share one base:
   write restriction on/off crossed with capability-only versus broad restricting
   SIDs. Keep the other flags unchanged. Descriptors carry the actual user's
   ordinary full-access grant and distinguish no matching root, write capability,
   read-only capability, unrelated capability, and broad Everyone/logon grants.
   Check explicit read, write, attributes, DELETE and DELETE_CHILD rights.
   API failure is not denial. No filesystem ACL is changed by this matrix.

   Prebound interpretation: a flag-dependent delete-child contrast on the same
   user-only descriptor isolates that classification effect; broad-SID effects
   on broad-grant rows isolate a separate authorization path. A lost private-read
   positive control is an observed compatibility cost. Unsupported predictions
   fail and retain observations; they are not rewritten after the run.

2. **Choose the smallest coherent production candidate.** Use the matrix and
   source review to choose a legacy-only repair, or report that none of the
   token-only options earns adoption. Constructor sharing, object ownership and
   logon access are continuing constraints, not incidental test details.

3. **Behavioral qualification if a candidate earns it.** On a disposable VM,
   retain the original deletion fixture and existing non-TTY, TTY, deny-read and
   token/object controls. Add focused workspace-write and read-only cases only
   as needed to check actual reads/execute plus create/write/rename/delete
   postconditions. Cross-boundary rename must preserve source and destination;
   missing paths or setup failure cannot masquerade as permission enforcement.

4. **Review and return.** Preserve exact source, tests, original failures and
   remaining limitations in Git/Ath. A source-qualified repair is not a selected
   runtime. Installation and any backend choice remain separate.

The first discriminator uses only three native token/AccessCheck cases on a
standard disposable Windows 2025 runner, with exact inventory, serial execution,
zero retries and bounded success/failure output. It does not replace actual
filesystem-operation qualification or imply all-host behavior. No new complete
workspace campaign is planned here.

## Current observations

- Fresh primary-source review still finds upstream issue32915 open; the cited
  third-party PR53 is not an upstream acceptance or demonstrated solution to the
  retained capability-only/delete-child counterexample.
- Current upstream token-file history's latest five entries contain bindings,
  default-object DACL and setup changes, not a newly identified deletion repair.
  This is a bounded path/history check, not an exhaustive upstream search.
- Microsoft documents restricting SIDs as write-only when `WRITE_RESTRICTED`
  is set. Its generic statement does not itself enumerate which NT filesystem
  rights take that branch; the proposed same-VM test addresses that gap.

Sources: [CreateRestrictedToken](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken),
[file rights](https://learn.microsoft.com/en-us/windows/win32/fileio/file-access-rights-constants),
[upstream issue](https://github.com/openai/codex/issues/32915).
