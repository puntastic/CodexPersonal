# Legacy Windows deletion boundary - bounded repair arc

Kestrel authorized this repair on 19 October (Australia/Brisbane), after the
[qualification return](2026-10-18-qualification.md) demonstrated actual outside
and protected-directory deletion in a disposable Windows fixture. Source starts
at `4fb9f05bca840c2f28f658cfc7cb83ebc96bad1e`. Ath2368r5 retains the current
evidence and exact original custody.

## Owner decision - retain unchanged with known limitation

On 19 October (Australia/Brisbane), after the findings and routing explanation,
Kestrel chose to leave the legacy backend as it is and record the problem.
The additional permissiveness is an accepted risk for the current operating
posture, which already allows comparable or broader access. Do not implement
the proposed refusal guard or pursue a replacement as part of this pass.

This closes the pending guard-versus-redesign choice, not the technical defect.
The observed deletion boundary failure, original failing regression, matrix
and source limits remain intact. Retention is not proof of isolation and does
not authorize any additional access or action. Reassess if a future task or
less-trusted worker depends on this mode reliably enforcing restricted access.

The legacy issue alone is no longer a blocker to further update work under
the accepted current posture. Other failing tests, missing qualification,
package/recovery checks and the installation decision remain separate; this
decision neither waives those checks nor installs a runtime.

## Original repair goal and boundaries

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

## Completed same-VM discriminator

Source `9a37d8ddb47178d4d75dd7acc84349e9bf962238`, workflow
`922a5b1e1b9304fcb184508b6b5cd29c1b1a4ef2`, disposable
[run 39154549279](https://github.com/puntastic/CodexPersonal/actions/runs/39154549279):
Windows 10.0.26100, image `win25-vs2026/20261007.264.1`. The exact three native
token tests passed in 0.051 seconds with no failures, errors, skips or retries.
The matrix has 210 unique data rows plus one summary, no API failures or
prediction mismatches, and no projection truncation. Its unrestricted base
control succeeds across all descriptors/rights. The other two tests preserve
existing proxy-SID membership and same-logon/other-logon object-access controls;
they do not exercise a changed production token.

Full formatting and scoped `just fix -p codex-windows-sandbox` compiled test
code cleanly in 42.74 seconds locally. No native test executed on the user's
machine. Primary and independent source review checked handles, SID/ACL/descriptor
lifetimes, explicit mappings, prebound predictions and bounded output. A later
independent result readback matched all 211 projection lines to the run tail,
all exact JUnit identities, and both hashes below.

| Same descriptor / requested right | WR + capability only | WR + broad SIDs | Full restriction + capability only | Full restriction + broad SIDs |
| --- | --- | --- | --- | --- |
| User-only / ordinary read | allowed | allowed | denied | denied |
| User-only / parent delete-child | allowed | allowed | denied | denied |
| Everyone-full / write-data | denied | allowed | denied | allowed |
| Logon-full / write-data | denied | allowed | denied | allowed |

The first two rows isolate a flag-dependent distinction on this host while the
base token, other flags, descriptor and right are held fixed. Removing the
broad restricting identities alone does not repair this parent-delete path.
Removing WR closes that modeled path but also loses legitimate user-private
reads. Retaining broad identities still authorizes modeled outside writes where
the corresponding broad ACE exists. None of these four variants meets both
modeled ordinary-read and write-isolation requirements on this host.

**Green here means the predicted unsafe allowances and compatibility losses
were observed. It does not mean remediation passed.** This is `AccessCheck`
against synthetic in-memory descriptors, not a native file-operation or
process-startup trial of a new policy. It strengthens the mechanism account
beside the earlier actual deletion, without claiming all token designs or
all Windows versions have been exhausted.

Selected original artifacts have hash/size-bound Ath custody and checked
artifact metadata:

- Original projection, 38,524 bytes:
  `ath-asset:sha256:e097ae5a443db50e1322ec8c32645daf5546469e13fd1dac61ce9a7eb97e8f0b`.
- Original JUnit, 893 bytes:
  `ath-asset:sha256:c33bb7904b0f0c78483d90da3ee7d10eba52b92c5d82c94ff19051529e40ffd4`.

Local originals and exact source/tool/binary receipts remain under
`codex-rs/target/desktop-dev/reports/20261019-token-matrix-39154549279`.
Transfer URLs, accounts, credentials and test homes are not stored in this note.

## Bounded alternative challenge and disposition

A second source/documentation review looked for a sound smaller mechanism,
not merely another arrangement of these two toggles:

- [Mandatory integrity control](https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control)
  supplies an orthogonal no-write-up boundary, not a writable-root identity.
  Equal/lower-integrity outside objects remain a countercase under the modeled
  discretionary gap. Explicit no-read-up/no-execute-up labels also narrow reads.
- [AppContainer](https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer)
  is a genuine boundary but needs package/capability access to user-private
  resources. Its broad-filesystem capability is not a documented universal
  read-only allowance for arbitrary native path opens.
- A trusted read-only handle broker could preserve access for cooperating
  clients. The existing process handle-list seam only supplies controlled
  handles, not transparent filesystem mediation for arbitrary CMD/PowerShell
  and native programs. This would be a different architecture/contract.

These are source-reviewed options, not built or runtime-qualified candidates.
No sound drop-in emerged under this task's constraints; this is not a proof
that no Windows solution exists. API interception, command-text prediction or
preflighting only current known roots cannot certify arbitrary future access.

The arc has reached its explicit contract-choice boundary. No production token,
ACL policy, backend default, live permission, package or installed runtime was
changed. The original deletion regression remains failing and unchanged.
The proposed additional filesystem-operation cases were not run because no
capability-preserving production candidate earned that qualification step.

Earlier recommendation, not adopted: authorize a narrowly scoped **fail-closed
refusal for this unsafe legacy backend**, with clear recovery information and
no silent privilege/backend escalation, while any replacement architecture is
designed separately. Such a guard is loss of that legacy capability, not a
functional sandbox repair; it therefore remains a proposal. Unrestricted
execution and other backend routes would need explicit non-regression checks.
Alternatively, authorize a larger replacement/read-contract design. The later
owner decision above selects neither: preserve current behavior with the known
limitation. Do not revive this choice without materially changed needs or a
new owner request.
