# Task 1.3: Skill Invocation Parsing Investigation

## File Tree
```
Task 1.3 (Skill Invocation Parsing)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-base/src/skill/invocation.rs (resolve_invocation, parse_invocation)
│   ├── jcode-tui/src/tui/app/input.rs (submit_input_with_routing)
│   ├── jcode-tui/src/tui/app/remote/input_dispatch.rs (submit_remote_slash_input)
│   └── jcode-tui/src/tui/app/state_ui_runtime.rs (refresh_skills_snapshot)
```

## Objective
Map the exact code path from user typing `/skillname prompt` to `active_skill` being set and the skill's full prompt being injected into the system instructions.

## Sub-Sub-Agent Assignments

### Agent A: Trace `resolve_invocation`
**Target**: `jcode-base/src/skill/invocation.rs:10-45`
**Questions**:
1. How does it iterate over whitespace boundaries to find the longest matching registered name (lines 17-28)?
2. How does it handle quoted prompts (stripping surrounding quotes at lines 31-36)?
3. What is the fallback to `parse_invocation` when no registered name matches (line 15)?
4. Why are multi-word skill names supported but file drops like `/tmp/screenshot.png` rejected?
**Output**: Name resolution algorithm with boundary iteration details

### Agent B: Trace `parse_invocation` fallback
**Target**: `jcode-base/src/skill.rs:689-715`
**Questions**:
1. Why are skill names restricted to alphanumeric, hyphen, or underscore only (lines 697-701)?
2. How does it split on first whitespace to separate name from prompt (lines 692-694)?
3. How does it handle quoted prompts identically to `resolve_invocation` (lines 705-710)?
4. What happens if the name is empty or contains invalid characters?
**Output**: Fallback parser logic with character validation rules

### Agent C: Trace TUI local input routing
**Target**: `jcode-tui/src/tui/app/input.rs:4140-4213`
**Questions**:
1. How does `parse_dropped_paths` prevent file drops from being parsed as skills (line 4146)?
2. How does it call `snapshot.resolve_invocation(&routing_input)` (line 4148)?
3. What happens when a skill is not found in the initial snapshot (lines 4165-4168)?
4. How does `refresh_skills_snapshot()` synchronously reload from disk to catch project-local skills?
5. How is `self.active_skill = Some(skill_name.clone())` set (line 4171)?
6. How is the trailing prompt extracted and the input replaced (lines 4180-4184)?
**Output**: Local TUI input flow diagram

### Agent D: Trace remote input routing
**Target**: `jcode-tui/src/tui/app/remote/input_dispatch.rs:205-265`
**Questions**:
1. How does it check if input is command-shaped using `contains_registered_slash_command` (line 213)?
2. How does it extract the trailing prompt for remote submission (lines 224-228)?
3. Why does it reuse the normal bare invocation path to update `active_skill` (line 248)?
4. How does it prepare only the trailing prompt for the remote request (lines 253-265)?
**Output**: Remote TUI input flow diagram

### Agent E: Investigate `refresh_skills_snapshot`
**Target**: `jcode-tui/src/tui/app/state_ui_runtime.rs:37-54`
**Questions**:
1. Why does it skip refresh in SSH remote mode (line 38)?
2. How does `SkillRegistry::load_global()` read ONLY global skills from disk (line 44)?
3. Why does it NOT load project-local skills here (to keep them session-scoped, issue #457)?
4. How does it update the shared registry and invalidate caches (lines 46-52)?
**Output**: Snapshot refresh logic with session isolation details

## Validation Requirements
- Read every function in full, including helper functions like `parse_dropped_paths`
- Verify all character validation rules by reading the actual regex/logic
- Compare local and remote input flows side-by-side
- Cross-reference with test files in `jcode-tui/src/tui/app/tests.rs`

## Deliverables
1. A modular code snippet file for each major function (< 100 lines each)
2. A comparison table showing local vs. remote skill invocation differences
3. A list of all character validation rules for skill names
4. A verification checklist confirming no assumptions were made about file drop handling
