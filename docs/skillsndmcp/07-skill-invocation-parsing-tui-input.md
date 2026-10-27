# Task 1.7: Skill Invocation Parsing and TUI Input Flow

## File Tree
```
Task 1.7 (Skill Invocation Parsing and TUI Input Flow)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-base/src/skill/invocation.rs (resolve_invocation, parse_invocation)
│   ├── jcode-tui/src/tui/app/input.rs (submit_input_with_routing)
│   ├── jcode-tui/src/tui/app/remote/input_dispatch.rs (submit_remote_slash_input)
│   └── jcode-tui/src/tui/app/state_ui_runtime.rs (refresh_skills_snapshot)
```

## Objective
Trace the exact code path from a user typing `/skillname prompt` to the skill being activated and the trailing prompt being submitted as the user message.

## Sub-Sub-Agent Assignments

### Agent A: Trace `resolve_invocation` multi-word logic
**Target**: `jcode-base/src/skill/invocation.rs:14-45`
**Questions**:
1. How does it iterate through whitespace boundaries in reverse to find the longest matching registered name (lines 17-28)?
2. Why does it prefer longer matches over shorter ones (e.g., "My Custom Skill" vs "My")?
3. How does it handle quoted prompts with single or double quotes (lines 31-36)?
4. What happens if no registered name is found—how does it fall back to `parse_invocation` (line 43)?
**Output**: Multi-word name resolution logic with fallback behavior

### Agent B: Trace `parse_invocation` fallback parser
**Target**: `jcode-base/src/skill.rs:689-715`
**Questions**:
1. Why does it restrict skill names to only alphanumeric, hyphen, or underscore characters (lines 697-701)?
2. How does this restriction prevent file drops like `/tmp/screenshot.png` from being parsed as skills (comment at line 687)?
3. How does it extract the trailing prompt after the first whitespace (lines 692-694)?
4. Why does it strip surrounding quotes from the prompt but not interpret them as shell syntax (lines 705-710)?
**Output**: Fallback parsing logic with security rationale

### Agent C: Trace local TUI input flow
**Target**: `jcode-tui/src/tui/app/input.rs:4145-4213`
**Questions**:
1. How does it call `snapshot.resolve_invocation(&routing_input)` to check for skill invocations (line 4148)?
2. Why does it synchronously refresh the snapshot from disk if the skill is not found (lines 4165-4168)?
3. How does it set `self.active_skill = Some(skill_name.clone())` and show the activation notice (lines 4171-4179)?
4. How does it replace the `input` variable with just the trailing prompt or clear it if there are pending images (lines 4180-4184)?
**Output**: Local TUI input handling with synchronous refresh logic

### Agent D: Trace remote TUI input flow
**Target**: `jcode-tui/src/tui/app/remote/input_dispatch.rs:206-265`
**Questions**:
1. How does it determine if the input is "command-shaped" by checking for slash commands or skill invocations (lines 210-213)?
2. Why does it restore the prepared input and fall back to normal routing if no valid skill is found (lines 218-228)?
3. How does it reuse the normal bare invocation path to update `active_skill` and show the activation notice (lines 248-251)?
4. How does it prepare only the trailing prompt for the remote request while keeping pasted images attached (lines 253-265)?
**Output**: Remote TUI input handling with image attachment preservation

### Agent E: Investigate `refresh_skills_snapshot` mechanism
**Target**: `jcode-tui/src/tui/app/state_ui_runtime.rs:37-54`
**Questions**:
1. Why does it skip refreshing if running in SSH remote mode (lines 38-40)?
2. How does it read ONLY global skills via `SkillRegistry::load_global()` to keep project-local skills session-scoped (lines 44-48)?
3. Why does it invalidate the command candidates cache and header prep cache after refreshing (lines 49-52)?
4. How does this ensure that project-local skills added via `.agents/skills/` are still visible without polluting other sessions?
**Output**: Snapshot refresh logic with session isolation rationale

## Validation Requirements
- Read the test in `skill_invocation_multi_word.rs` that validates multi-word skill name matching
- Verify the `parse_dropped_paths` function that prevents file drops from being parsed as skills
- Check the `endorsed_skills()` list that provides installation hints for missing endorsed skills
- Cross-reference with the `active_skill` field in the `App` struct and its serialization in the wire protocol

## Deliverables
1. A modular code snippet for `resolve_invocation` (< 50 lines)
2. A modular code snippet for `parse_invocation` (< 40 lines)
3. A flowchart showing the decision tree from input submission to skill activation
4. A comparison table of local vs remote TUI input handling differences
