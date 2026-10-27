# Task 1.8: Static Skill List Injection and Late Announcements

## File Tree
```
Task 1.8 (Static Skill List Injection and Late Announcements)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-base/src/prompt.rs (build_available_skills_section, clip_skill_description)
│   ├── jcode-app-core/src/agent/turn_execution.rs (announce_late_skills, seed_announced_skills_from_transcript)
│   └── jcode-tui/src/tui/app/state_ui.rs (build_skills_report)
```

## Objective
Trace how skills are injected into the system prompt both statically at startup and dynamically mid-session without invalidating the KV cache.

## Sub-Sub-Agent Assignments

### Agent A: Trace `build_available_skills_section` implementation
**Target**: `jcode-base/src/prompt.rs:265-282`
**Questions**:
1. How does it format the "# Available Skills" section with skill names and descriptions (lines 270-277)?
2. Why does it use `clip_skill_description` to collapse whitespace and limit description length (line 275)?
3. What is the value of `SKILL_DESC_MAX_CHARS` and how does it handle clipping with an ellipsis?
4. Why does it add a note at the end telling the agent to mention these skills when asked about capabilities (lines 278-280)?
**Output**: Static list formatting logic with character clipping details

### Agent B: Trace `clip_skill_description` helper
**Target**: `jcode-base/src/prompt.rs:251-263`
**Questions**:
1. How does it split whitespace and rejoin with single spaces to normalize the description (line 252)?
2. If the one-line version fits within `SKILL_DESC_MAX_CHARS`, why does it return early (lines 253-255)?
3. How does it take exactly `SKILL_DESC_MAX_CHARS - 1` characters and append an ellipsis for longer descriptions (lines 257-262)?
4. Why is this function critical for keeping the static prompt prefix small and cache-efficient?
**Output**: Description clipping logic with cache efficiency rationale

### Agent C: Trace `announce_late_skills` implementation
**Target**: `jcode-app-core/src/agent/turn_execution.rs:774-816`
**Questions**:
1. Why does it return early if `self.session.system_prompt.is_some()` (lines 775-777)?
2. How does it call `seed_announced_skills_from_transcript()` to prevent duplicate announcements after session restore (line 778)?
3. How does it filter out already-announced skills using `self.announced_skills` (lines 779-783)?
4. How does it format the `<system-reminder>` message with `LATE_SKILL_ANNOUNCEMENT_MARKER = "New skills were installed."` (lines 787-800)?
5. Why does it add the message as a `Role::User` message instead of a system message (lines 808-814)?
**Output**: Late announcement logic with cache-stability preservation

### Agent D: Trace `seed_announced_skills_from_transcript` scanner
**Target**: `jcode-app-core/src/agent/turn_execution.rs:820-842`
**Questions**:
1. How does it scan messages starting from `announced_skills_scan_index` to avoid re-scanning old messages (lines 821-823)?
2. How does it look for `ContentBlock::Text` blocks containing `LATE_SKILL_ANNOUNCEMENT_MARKER` (lines 825-827)?
3. How does it extract skill names from lines starting with "- `/" by splitting on whitespace (lines 829-833)?
4. Why does it update `announced_skills_scan_index` to the current message count after scanning (line 841)?
**Output**: Transcript scanning logic with incremental index tracking

### Agent E: Investigate `build_skills_report` TUI display
**Target**: `jcode-tui/src/tui/app/state_ui.rs:1790-1891`
**Questions**:
1. How does it get the active skill via `app.active_skill()` and mark it in the report (line 1793)?
2. How does it iterate through the skills snapshot and format each skill with its name, description, and path (lines 1820-1860)?
3. How does it distinguish between global and project-local skills based on their paths?
4. Why does it include endorsed but not installed skills with installation hints in the report?
**Output**: TUI report generation with active skill highlighting

## Validation Requirements
- Read the test in `skill_installed_mid_session_keeps_system_prompt_stable_and_is_announced_once` that validates late skill announcements
- Verify the `LATE_SKILL_ANNOUNCEMENT_MARKER` constant value and its usage in the transcript scanner
- Check the `announced_skills` HashSet and how it persists across session restores
- Cross-reference with the `prompt_skills_snapshot` field in the `Agent` struct

## Deliverables
1. A modular code snippet for `build_available_skills_section` (< 30 lines)
2. A modular code snippet for `clip_skill_description` (< 20 lines)
3. A diagram showing the flow from skill installation to late announcement injection
4. A table comparing static vs dynamic skill injection mechanisms
