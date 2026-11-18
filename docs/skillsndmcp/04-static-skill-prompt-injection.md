# Task 1.4: Static Skill Prompt Injection Investigation

## File Tree
```
Task 1.4 (Static Skill Prompt Injection)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-base/src/prompt.rs (build_available_skills_section, clip_skill_description)
│   ├── jcode-base/src/skill.rs (SkillInfo struct)
│   └── jcode-app-core/src/agent/prompting.rs (build_jcode_system_prompt_split)
```

## Objective
Map how skill names and descriptions are embedded into the static system prompt, and how the full skill prompt is appended to dynamic instructions during explicit invocation.

## Sub-Sub-Agent Assignments

### Agent A: Trace `build_available_skills_section`
**Target**: `jcode-base/src/prompt.rs:265-282`
**Questions**:
1. How does it format the "Available Skills" section header (line 270)?
2. How does it iterate over `available_skills` and format each entry (lines 271-277)?
3. Why is there a trailing space after `/skillname` in the format string (line 273)?
4. What instruction is appended at the end of the section (lines 278-280)?
**Output**: Section formatting logic with exact markdown structure

### Agent B: Trace `clip_skill_description`
**Target**: `jcode-base/src/prompt.rs:251-263`
**Questions**:
1. How does it collapse all whitespace into single spaces (line 252)?
2. What is the value of `SKILL_DESC_MAX_CHARS` constant?
3. How does it handle clipping with an ellipsis when the description exceeds the limit (lines 257-262)?
4. Is the clipping UTF-8 safe (using `.chars()` instead of bytes)?
**Output**: Clipping algorithm with character limit details

### Agent C: Trace `build_jcode_system_prompt_split`
**Target**: `jcode-app-core/src/agent/prompting.rs:177-225`
**Questions**:
1. How does it check for `active_skill` and fetch the skill from the registry (lines 191-194)?
2. How does it call `skill.get_prompt()` to retrieve the full skill content?
3. How does it append the skill prompt to the `dynamic_part` (line 198)?
4. Why is this done in the dynamic part rather than the static part (to preserve KV cache stability)?
**Output**: Dynamic prompt injection flow with cache-stability rationale

### Agent D: Investigate `SkillInfo` and `get_prompt`
**Target**: `jcode-base/src/skill.rs:923-930`
**Questions**:
1. What fields does `SkillInfo` contain (name, description only)?
2. How does `get_prompt()` return the full skill content (including frontmatter and body)?
3. Is the prompt cached or read fresh from disk every time?
4. How does this differ from the clipped description used in the static list?
**Output**: Skill data structure comparison table

### Agent E: Verify prompt caching behavior
**Target**: `jcode-app-core/src/agent.rs:400-420`
**Questions**:
1. How does `refresh_prompt_skills_snapshot()` update `prompt_skills_snapshot` (lines 400-409)?
2. When is this called (at session startup, on reload)?
3. Does changing a skill file mid-session require a restart to see changes in the static list?
4. How does this interact with `announce_late_skills` for newly installed skills?
**Output**: Snapshot refresh timing and cache interaction details

## Validation Requirements
- Read the exact markdown output produced by `build_available_skills_section`
- Verify the character limit by finding the `SKILL_DESC_MAX_CHARS` constant definition
- Compare static list vs. full prompt injection side-by-side
- Cross-reference with test files in `jcode-base/src/prompt_tests.rs`

## Deliverables
1. A modular code snippet for `build_available_skills_section` (< 50 lines)
2. A modular code snippet for `clip_skill_description` (< 30 lines)
3. A comparison table showing static list vs. dynamic prompt differences
4. A verification checklist confirming UTF-8 safety in clipping
