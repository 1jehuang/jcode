# Rewind checkpoints

Run `/rewind`, or press Escape twice with an empty prompt while Jcode is idle. Select a user prompt, then choose what to restore:

- **Conversation and files** returns both to the state before that prompt.
- **Conversation only** keeps the files on disk.
- **Files only** keeps the conversation.

Conversation restore puts the selected prompt's text back in the editor. It does not send it. The file restore options appear when the checkpoint has tracked file changes. Escape goes back or closes the picker without restoring anything.

Existing `/rewind N` and `/rewind undo` remain conversation-only commands with their original message numbering. They are separate from the new prompt checkpoints. Undo does not undo a file restore.

## What is captured

Jcode creates a checkpoint before a user prompt starts a turn and records file contents before its editing tools change them. Checkpoints belong to the session, survive resume, and retain the most recent 100 prompt boundaries. They preserve the bytes that were on disk, including work that was already uncommitted. They do not use Git reset, checkout, or stash.

The editing tools track writes, edits, replacement operations, and patches. Shell commands, arbitrary programs, external tools, and edits made by another session are outside this history. Each session has its own checkpoints. This is a quick recovery tool, not a replacement for Git or backups.

File restore checks the affected paths before changing anything. If a file has been changed externally, a path is a symlink or hard link, or its parent directory no longer points to the original location, restore refuses rather than overwriting unrelated work. A failed file preflight leaves the conversation intact too.

A new checkpoint restore removes the selected prompt and everything after it from the active conversation. It resets provider continuation and compaction state, so the next turn uses the restored history. The picker restores prompt text, not image attachments.

## Implementation research

The behavior was investigated on October 22, 2026 using the locally installed Claude Code **2.1.307**, not an assumed current release. REA inspected its arm64 Mach-O package and then parsed focused, inert JavaScript regions recovered from its Bun segment. No Claude application code was executed during that analysis.

Artifact SHA-256:

```text
5b113a7752119470f1da419cb0e608af3361cae25b614216b13022093958fb2a
```

The package contained a versioned file-history store, prompt-associated snapshots, a two-stage message/action selector, three restore modes, and checks against unsafe linked paths. The relevant source byte ranges were `227450792..227466324` for file-history functions and `252508302..252514661` for the selector. These ranges were read as evidence, not copied into Jcode's Rust implementation.

REA's native inspection evidence was `ev_6ab6a3afe555efd0f181a8fa0ae9b0216365608dcfdcf4f102959b3d3d36a23e`; focused JavaScript analysis was `ev_8d840d9a960ffacded53c39a933e04a3c15eab1a01f34c9b473c233f48d721b9`. The JavaScript graph had partial coverage and unresolved obfuscated references. Findings here are limited to the inspected regions and their literal control flow, cross-checked against Anthropic's checkpointing documentation.

Claude Code also offers selective summarization and recovery of a conversation cleared in the same process. Those are separate features and are not part of this rewind implementation.
