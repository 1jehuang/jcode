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
