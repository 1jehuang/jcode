# AGENTS.md Changes: Operator Context Management

**Session**: 20250913-tui-polish
**Date**: 2026-09-14
**Task**: Add "Operator Context Management" section to all 3 harness AGENTS.md files

## Summary

Added a new section titled "Operator Context Management" to all 4 agent configuration files across the 3 active harnesses. This section standardizes how agents communicate context back to the operator (Koosha) who runs 16-128 agents simultaneously and cannot remember the context of each chat.

## Files Modified

| File | Harness | Lines Before | Lines After | Added Lines |
|------|---------|-------------|-------------|-------------|
| `~/AGENTS.md` | Jcode (primary) | 3781 | ~3828 | +47 (section 26) + TOC update |
| `~/.codex/AGENTS.md` | Codex CLI | 339 | ~411 | +73 |
| `~/.forge/AGENTS.md` | Forge CLI | 259 | ~331 | +73 |
| `~/.jcode/memories/global/harness-agents.md` | Jcode global | 216 | ~288 | +73 |

## What Was Added

### 1. End-of-Turn Context Summary (REQUIRED)
- Structured context block using `──` box drawing and `█░` progress bars
- Per-repo status line: `[repo: name | status: X]`
- 1-3 concrete next steps so operator never has to ask "what's next?"
- Uses terminal glyphs (✓ ✗ ▸ ● ◆) instead of emoji

### 2. Strict Coordinator Role
- Agents MUST delegate all implementation work to subagents
- Main context stays clean for project/product/program management only
- Never read code directly; spawn workers for all code tasks
- Especially applies to Jcode as the primary harness

### 3. /poke Command
- Creates a fresh Jcode session with loaded context
- Loads global memories, project memories, session history
- Auto-proceeds ONLY on non-harmful / trivially reversible actions
- Reports current state of all active repos and next actions
- Format: `/poke [repo-name]` or `/poke` for all repos

## Insertion Points

- **~/AGENTS.md**: Inserted as section 26 after "29. References", before closing paragraph. Also added to Table of Contents.
- **~/.codex/AGENTS.md**: Appended after existing "Context Recovery Template" section.
- **~/.forge/AGENTS.md**: Appended after existing "Context Recovery Template" section.
- **~/.jcode/memories/global/harness-agents.md**: Appended after "Git Workflow > PR Pattern" section.

## No Existing Conflicts

Codex and Forge already had a "Coordinator Role & Operator Interaction Protocol" section with similar content. The new "Operator Context Management" section adds the specific context summary format and /poke command that were not previously defined. The two sections complement each other rather than conflict.

## Validation

- All 4 files were read before editing to confirm existing structure
- All 4 edits succeeded on first attempt (no reversions needed)
- No Rust code was modified (documentation-only task)
- Line counts checked: Jcode 3828, Codex 411, Forge 331, Global 288

## Follow-up

- The /poke command needs to be implemented as a Jcode skill (currently a spec only)
- Consider adding to Jcode skill registry: `/poke` handler that loads context and reports status
