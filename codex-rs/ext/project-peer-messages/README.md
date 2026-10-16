# Project peer messages v0

Native communication for independent, already-loaded root chats sharing one
Desktop host process. No listener, sidecar, backlog database, cold resume,
parent/child registration, automatic reply, or transport retry is installed.

## Host-owned opt-in

The fixed file is `CODEX_HOME/project-peer-messages.json`, resolved from the
actual app-server owner's home, not a repository configuration or tool argument.
This implementation does not create that file. Its absence exposes no tools.
Invalid, oversized, non-file, or symlink policies disable the capability.

An operator-authored policy has this shape (example identifiers only):

```json
{
  "version": 1,
  "grants": [
    {
      "sender": "00000000-0000-4000-8000-000000000001",
      "recipient": "00000000-0000-4000-8000-000000000002",
      "active_send": true,
      "idle_wake": false
    }
  ]
}
```

Grants are directed; replies need their own grant. There are no wildcards,
self grants, or model-controlled sender/grant fields. A delivery grant does not
authorize the receiving model to perform requests in the message.

The first registry installation caches the policy (including absence or failure)
for the **entire process**, with a SHA-256 generation over its exact bytes.
MCP/registry refresh does not reload it. Editing or removing the file does not
revoke live grants: stop/restart the host process to apply a new policy. Another
CODEX_HOME in the same process fails closed. Each thread runtime also receives
a fresh token, so captured tools from an old runtime cannot impersonate a new one.

## Delivery and limits

`send_peer_message` has only `recipient_id`, `message`, and an explicit delivery
mode. `active_turn` adds typed input only to an accepting active regular turn.
`idle_turn` starts only if idle and separately granted; it preserves normal
automatic-start admission gates. Races return refusal rather than changing mode.
Cold or hidden recipients are not resumed or replaced. Use the ordinary app
message route only as a separately authorized, explicit operation.

Policy: 16 KiB, at most 32 grants and eight recipients per sender. Message body:
4096 UTF-8 bytes, never clipped. Tool arguments: 32 KiB including JSON escaping.
Each active sender turn permits four authorized attempts, including refusals;
the counter lives with that turn rather than in an unbounded history map.

The host builds the MESSAGE/NEW_TASK envelope, warning, sender/recipient addresses
and real `amsg_` ID. Payload text cannot replace the top-level kind. Flat UUID
addresses do not establish agent-tree ownership. Admission/start receipts are
not evidence of reading, use, model exposure or durable storage. Unknown outcomes
are not retried. The ordinary AgentMessage history/rollout and bounded compaction
policies still apply; exact content is not promised to survive every compaction.

The 4096-byte body plus bounded envelope can exceed 1K tokens. This item size
received explicit task-scoped P0 context review on 2026-10-16: it is bounded,
attributed peer content, at most four authorized attempts per turn, not standing
instruction injection or user authority. The envelope remains below 10K tokens.
