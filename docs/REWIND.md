# Rewind checkpoints

Run `/rewind`, or press Escape twice with an empty prompt while Jcode is idle. Prompts appear in conversation order, followed by **(current)**. The picker opens on **(current)**: Enter closes without changing anything, and Up selects the latest submitted prompt. Select a past prompt, then choose what to restore:

- **Conversation and files** returns both to the state before that prompt.
- **Conversation only** keeps the files on disk.
- **Files only** keeps the conversation.

Conversation restore returns the selected prompt's text and image attachments to the editor without sending them. Undo recovers the previous draft, including collapsed pastes and image-marker ownership. Files-only restore preserves that draft. File options appear only when a usable checkpoint has tracked changes. Escape or **Never mind** returns from the mode picker to the same selected prompt; Escape from the prompt list closes it.

Existing `/rewind N` and `/rewind undo` remain conversation-only commands with their original message numbering. They are separate from the new prompt checkpoints. Undo does not undo a file restore.

## What is captured

Jcode records a boundary before genuine user input, including human steering received during a running turn, and records file contents before its editing tools change them. File snapshots survive resume and retain the most recent 100 prompt boundaries. Conversation targets are independent of that limit: older sessions without file snapshots still support conversation-only restore. Lightweight boundary anchors retain the original point before per-turn hidden context when a file snapshot is evicted. Backups preserve bytes that were already uncommitted; they do not use Git reset, checkout, or stash.

The editing tools track writes, edits, replacement operations, and patches. Shell commands, arbitrary programs, external tools, and edits made by another session are outside this history. Each session has its own checkpoints. This is a quick recovery tool, not a replacement for Git or backups.

With the Claude Code provider, an available Jcode tool executor exposes tracked editing tools through MCP and disables Claude's native `Write`, `Edit`, `MultiEdit`, and `NotebookEdit` tools. That sends edits through the before-write journal instead of trying to reconstruct them from after-the-fact tool events. Standalone provider callers without a host executor keep their native tools. Bash writes remain outside checkpoint coverage.

File restore checks the affected paths before changing anything. If a file has been changed externally, a path is a symlink or hard link, or its parent directory no longer points to the original location, restore refuses rather than overwriting unrelated work. A failed file preflight leaves the conversation intact too.

A checkpoint restore removes the selected prompt and everything after it from the active conversation. It resets provider continuation and compaction state before the next turn, including state held inside persistent provider connections. Targets use immutable stored-message IDs, so a removed target cannot select a new prompt that reuses its old index. Completion payloads include optional image tuples; older text-only payloads remain valid.

## Implementation research

The behavior was investigated on October 22, 2026 using the locally installed Claude Code **2.1.307**, not an assumed current release. REA inspected its arm64 Mach-O package and then parsed focused, inert JavaScript regions recovered from its Bun segment. No Claude application code was executed during that analysis.

Artifact SHA-256:

```text
5b113a7752119470f1da419cb0e608af3361cae25b614216b13022093958fb2a
```

The package contained a versioned file-history store, prompt-associated snapshots, a two-stage message/action selector, three restore modes, and checks against unsafe linked paths. The relevant source byte ranges were `227450792..227466324` for file-history functions and `252508302..252514661` for the selector. These ranges were read as evidence, not copied into Jcode's Rust implementation.

REA's native inspection evidence was `ev_6ab6a3afe555efd0f181a8fa0ae9b0216365608dcfdcf4f102959b3d3d36a23e`; focused JavaScript analysis was `ev_8d840d9a960ffacded53c39a933e04a3c15eab1a01f34c9b473c233f48d721b9`. The JavaScript graph had partial coverage and unresolved obfuscated references. Findings here are limited to the inspected regions and their literal control flow, cross-checked against Anthropic's checkpointing documentation.

Claude Code also offers selective summarization and recoverable conversation branches. Those are separate features and are not claimed by this rewind implementation.

### Runtime comparison, October 23, 2026

A second pass used REA process captures of both installed executables with matched disposable resumed sessions. It observed the reference's current-row no-op, chronological prompt navigation, conversation-only restore without file snapshots, Never mind returning to the selected row, and all three restore modes. File-mode fixtures used each program's persisted backup format; captures measured actual picker keys and resulting file bytes, not just static source or a mocked dialog.

Reference evidence includes `ev_54e2b53e59f4e20dd0c9901766f94c86ddf20c41ad3f2a187fbbfda8c6f02f24` for the legacy picker, `ev_6cc9ebced2a09dae243adbe6702d91ea6cfe0da6ef9eb8a49a6557d0536f7f11` for Enter on current, and `ev_419166e3f586fd0034c36ac6284bd08fdca5953cae79c0ed9b4e91878ae44ba9` for combined restoration. Installed Jcode evidence `ev_301083843a51be230b0bdada04a5ca0169bf3ef2a6c1809f82e8b89b7c6f6033` and `ev_520e026ba4bb4403b715a23d96af3394f9ccacd1dde2cf5ef6bcb31ebdaa5ca9` reproduced missing legacy rows and wrong initial selection. Genuine input submission, tracked editing tools, image transport, provider reset, and error recovery have separate integration regressions; they are not inferred from seeded fixtures.

The corrected Jcode executable, `v0.98.752-dev`, passed seven real TUI workflows with 73 observable checks: current-row no-op, conversation-only restore without a file sidecar, Never mind, combined restore, files-only restore, conversation-only restore with tracked files, and an external edit introduced after opening the picker. Checks covered complete terminal captures, exact persisted conversation boundaries, original prompt refill without submission, requested file bytes, and unrelated dirty files. The fixture sessions were disposable and preseeded; genuine submitted inputs and mutations were checked separately by the socket and MCP integration tests.

Candidate executable SHA-256:

```text
492b81258b133217fb37ad44d0b41f52ff273a99108daf7894485997813c7875
```

Final evidence includes `ev_f4377ea05889de43548048fcaecfc4a26939de649cb9494026014e693c45919c` for legacy conversation restore, `ev_0b0155eca1aa051df15cd08a7fbc8a46443d855b46ae0cb61e0f983e784d2006` for combined restore, `ev_e4a4e380875fc5566ffeedd31adb3a54f81aeb6e13af7b19a17dd9d1041a777b` for files-only, and `ev_453894129a55f0b6ca554cfee3d08edf099d66d0082af7985d20784173c3ce76` for external-conflict refusal. Initial replay captures exceeded the rendered-frame budget and were inconclusive; the final captures used a larger budget and were explicitly checked for truncation.

Installed Claude Code was also initialized under `bypassPermissions` without a model request. Its offered `Edit` tool disappeared when the mandatory native-editor deny list was supplied, while `Read` and `Bash` remained. Evidence is `ev_f83d8d1654fa96801ab30c3379bc230d71bab72a3f92b8218afff5efc1cf0219`. That observation verifies the deny behavior for the native editor actually offered by the probe; fake-child regressions separately check all four deny names, executor availability, sparse tool sets, and child restarts when routing changes.
