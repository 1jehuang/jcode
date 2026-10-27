# Task 1.5: TUI Skill Invocation Parsing Investigation

## File Tree
```
Task 1.5 (TUI Skill Invocation Parsing)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-base/src/skill/invocation.rs (resolve_invocation, parse_invocation)
│   ├── jcode-tui/src/tui/app/input.rs (submit_input_with_routing)
│   └── jcode-tui/src/tui/app/remote/input_dispatch.rs (submit_remote_slash_input)
```

## Objective
Trace the exact code path from a user's keystroke (`/skillname prompt`) to skill activation in both local and remote TUI modes.

## Sub-Sub-Agent Assignments

### Agent A: Trace `resolve_invocation` multi-word logic
**Target**: `jcode-base/src/skill/invocation.rs:14-44`
**Questions**:
1. How does it iterate over whitespace boundaries in reverse to find the longest matching skill name (lines 17-22)?
2. Why does it prefer the longest registered name over shorter prefixes?
3. How does it handle quoted prompts (single/double quotes) at lines 31-36?
4. What happens if no registered name matches (fallback to `parse_invocation` at line 15)?
**Output**: Multi-word boundary iteration algorithm with edge cases

### Agent B: Trace `parse_invocation` fallback
**Target**: `jcode-base/src/skill.rs:689-715`
**Questions**:
1. How does it restrict skill names to alphanumeric, hyphen, or underscore only (lines 697-701)?
2. Why does it reject path separators and filename punctuation (to prevent `/tmp/screenshot.png` from being parsed as a skill)?
3. How does it extract the trailing prompt after the first whitespace (lines 692-694)?
4. Is this fallback used for single-token skill names only?
**Output**: Fallback parser restrictions and security rationale

### Agent C: Trace local TUI input flow
**Target**: `jcode-tui/src/tui/app/input.rs:4145-4213`
**Questions**:
1. How does it call `snapshot.resolve_invocation(&routing_input)` at line 4148?
2. If a skill is found, how does it look up the skill in the registry (line 4158)?
3. If not found, why does it call `refresh_skills_snapshot()` synchronously (line 4166) before reporting an error?
4. How does it set `self.active_skill` and replace the `input` variable with just the trailing prompt (lines 4171-4184)?
**Output**: Local invocation flowchart with synchronous refresh logic

### Agent D: Trace remote TUI input flow
**Target**: `jcode-tui/src/tui/app/remote/input_dispatch.rs:206-258`
**Questions**:
1. How does it check if the input is "command-shaped" using `resolve_invocation` (lines 210-213)?
2. If a valid skill with a trailing prompt is found, how does it prepare the remote request (lines 248-257)?
3. Why does it call `app.submit_input()` locally first to show the activation notice (line 251)?
4. How does it ensure pasted images remain attached to the same user turn?
**Output**: Remote invocation flowchart with image attachment handling

### Agent E: Investigate endorsed-but-not-installed hint
**Target**: `jcode-tui/src/tui/app/input.rs:4189-4201`
**Questions**:
1. How does it check if the unknown skill is in the endorsed list (line 4191)?
2. What installation hint does it provide based on the `install` field (lines 4193-4196)?
3. Why is this distinction important for user experience (issue #445)?
4. How does this differ from a simple "Unknown skill" error?
**Output**: Endorsed skill hint logic with UX rationale

## Validation Requirements
- Read the exact regex or character checks used in `parse_invocation`
- Verify the synchronous refresh behavior by reading the test in `tests/skill_invocation_multi_word.rs`
- Cross-reference with remote tests in `remote_tests.rs`
- Check if file drops are explicitly guarded against in both local and remote paths

## Deliverables
1. A modular code snippet for `resolve_invocation` (< 50 lines)
2. A modular code snippet for `parse_invocation` (< 40 lines)
3. A flowchart comparing local vs. remote invocation paths
4. A security checklist confirming file drop rejection
