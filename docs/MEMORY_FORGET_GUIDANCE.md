# Memory forget guidance: tombstone vs hard removal

R11 privacy-pass companion doc (PROGRAM.md rec 11; evidence: MRMMIA + FSFM
selective-forgetting 2604.20300v2). Audience: anyone calling `forget()` or
`forget_with_policy()` on `MemoryManager`, and anyone deciding what to do
with a secret-bearing row the R11 detector (`contains_secret` in
`jcode-memory-types`) has down-ranked.

## Rule of thumb

- **Correctness disputes go to tombstone** (the default). The fact is wrong,
  stale, or superseded, but the ROW is not the hazard.
- **Secret content goes to hard removal** (`privacy=true`). The row bytes
  themselves are the hazard (credentials, tokens, private keys, PII the user
  asked to be unrecoverable).

Never hard-remove for ordinary staleness: it destroys the supersede chain
and invalidates provenance for rows that cite the removed one.

## What each operation does

### `forget(id)` — tombstone (default, audit-correct)

Sets `active=false, superseded_by=None` on the row, in project scope first,
then global (same order as the old removal). The row is preserved:

- invisible to every retrieval path (all of them filter on `active`;
  quarantined tool-ingested rows stay invisible under the R10 switch too),
- still listed by `list_all` (auditable: you can see what was forgotten),
- still linked in the graph (no phantom edges, no dangling citations).

Use for: wrong facts, outdated preferences, superseded corrections,
duplicates, anything where the dispute is about CORRECTNESS.

### `forget_with_policy(id, privacy=true)` — hard removal (secret-wrong)

Removes the row entirely via `remove_memory`: no tombstone, no edge, no
trace in the store. This is exactly what `forget` did before the R3 DELETE
policy, reserved for the one case tombstoning gets wrong.

Tombstones preserve rows, which is audit-correct but secret-wrong: a
tombstoned credential is still bytes on disk (in the graph JSON) and still
leaks through backups, `list_all` inspection, and any future code path
that reads rows without the `active` filter. If the content itself is the
hazard, preservation IS the vulnerability. Remove it.

Use for: API keys, tokens, passwords, private-key blocks, session cookies,
PII the user asked to be unrecoverable — anything where the dispute is
about the BYTES, not about correctness.

After hard removal, also check: backups of the graph file, shell history
(if the secret was pasted as a command), and conversation logs that may
hold a second copy. The store cannot reach those; say so when it matters.

## Decision table

| Situation | Call | Why |
|---|---|---|
| Fact turned out wrong | `forget(id)` | correctness dispute; keep the audit trail |
| Preference changed | `forget(id)` | history of the change is useful |
| Duplicate of another row | `forget(id)` | supersede chain stays intact |
| Row contains a credential/token/key | `forget_with_policy(id, true)` | bytes are the hazard |
| Row contains PII user asked to erase | `forget_with_policy(id, true)` | unrecoverable means unrecoverable |
| Ordinary staleness, no secret | `forget(id)` | never hard-remove; destroys provenance |

## Interaction with the R11 safety penalty

`contains_secret` down-ranks (0.5x via `safety_penalty` inside
`memory_score`); it never deletes and never hides. A penalized row is a
CANDIDATE for hard removal, not an automatic one: review it, then apply
the table above. False positives rank normally by design (fail-closed
toward recall); do not "fix" a false positive by forgetting it — fix the
detector and pin the case in `detector_silent_on_normal_content`.

## Interaction with R10 quarantine

Quarantine (`JCODE_MEMORY_QUARANTINE_TOOL_INGESTED=1`) hides tool-ingested
rows from recall; it does not delete them and does not apply to User or
AgentDistilled rows. A quarantined row that ALSO bears a secret is still a
row on disk: quarantine is a recall filter, not erasure. Apply the table
above independently of quarantine state.
