# Cue activation lab

This experimental receiver loads a closed, bounded cue-header catalogue and
observes whether the current turn resembles one cue. It does not search or
hydrate the cue's source. `shadow` records the decision without changing model
input; `advisory` additionally contributes one bounded internal pointer.

```toml
[features.cue_activation]
enabled = true
mode = "shadow"
catalog_path = "cue-exports/muse-work-cues.json"
```

Relative catalogue paths resolve beneath `CODEX_HOME`. Shadow advances the same
two-turn cue cooldown as advisory so its decisions simulate advisory burden.
Prior substantive requests participate only when the current turn is a small
recognized continuation such as `continue` or `back`.

## Inspect a trial

The receiver writes one bounded `codex.cue_activation.decision` event per
evaluated turn to the app-server's existing diagnostic log. Catalogue errors
are rate-limited to the first one in a task. It never logs the request,
attachments, cue body, risk, or discriminator. Read recent events from the
source checkout with:

```powershell
.\codex-dev.ps1 -Action Just -JustArguments @(
  "log", "--",
  "--level", "debug",
  "--module", "codex_cue_activation_extension",
  "--search", "codex.cue_activation.decision"
)
```

The statuses are `selected`, `cooldown`, `no_match`, `unrepresented`, and
`catalog_error`. Selected receipts include the cue ID, matched lexical handle,
current/prior scope, score, and exact catalogue SHA-256. A receipt proves only
what this build did with those catalogue bytes on that turn; it does not prove
semantic relevance, usefulness, freshness, or absence of missed cues.
