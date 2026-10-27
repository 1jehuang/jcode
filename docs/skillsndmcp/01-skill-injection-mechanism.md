# Task 1.1: Skill Injection Mechanism Investigation

## File Tree
```
Task 1.1 (Skill Injection)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-base/src/skill.rs (resolve_invocation, parse_invocation)
│   ├── jcode-tui/src/tui/app/input.rs (submit_input_with_routing)
│   ├── jcode-base/src/prompt.rs (build_system_prompt_split, build_available_skills_section)
│   └── jcode-app-core/src/agent/turn_execution.rs (announce_late_skills)
```

## Objective
Map the complete code path from user typing `/skillname prompt` to the skill's full prompt being injected into the system instructions.

## Sub-Sub-Agent Assignments

### Agent A: Trace `resolve_invocation`
**Target**: `jcode-base/src/skill.rs:689-715` (`parse_invocation`) and `jcode-base/src/skill/invocation.rs:14-44` (`resolve_invocation`)
**Questions**:
1. How does the fallback `parse_invocation` restrict skill names (alphanumeric, hyphen, underscore only)?
2. How does `resolve_invocation` iterate over whitespace boundaries to find multi-word skill names?
3. How are quotes handled in trailing prompts?
4. What prevents file drops like `/tmp/screenshot.png` from being parsed as skills?
**Output**: Structured parsing rules with exact character class restrictions

### Agent B: Trace `submit_input_with_routing`
**Target**: `jcode-tui/src/tui/app/input.rs:4039-4317`
**Questions**:
1. At what line does `resolve_invocation` get called?
2. How is `active_skill` set when a valid skill is found?
3. How is the trailing prompt extracted and used to replace the input?
4. What happens when a skill is not found (synchronous refresh from disk)?
5. How are endorsed-but-not-installed skills handled differently?
**Output**: Step-by-step flow diagram with line numbers

### Agent C: Trace `build_system_prompt_split`
**Target**: `jcode-base/src/prompt.rs:587-672` (`build_system_prompt_split_with_capabilities_and_agents_md`)
**Questions**:
1. How does `build_available_skills_section` (line 265) format the static skill list?
2. How does `clip_skill_description` (line 251) clip descriptions to `SKILL_DESC_MAX_CHARS`?
3. Where exactly is the active skill prompt appended to `dynamic_parts` (line 655-656)?
4. What is the exact structure of the `SplitSystemPrompt` returned?
**Output**: Prompt structure diagram showing static vs. dynamic parts

### Agent D: Investigate `announce_late_skills`
**Target**: `jcode-app-core/src/agent/turn_execution.rs:774-842`
**Questions**:
1. What is the exact value of `LATE_SKILL_ANNOUNCEMENT_MARKER` (line 1552)?
2. How does `seed_announced_skills_from_transcript` (line 820) scan for previously announced skills?
3. How does it parse skill names from lines starting with "- `/"?
4. Why does the system prompt remain cache-stable during late announcements?
**Output**: Announcement mechanism flow with cache stability explanation

## Validation Requirements
- Read every function in full, not just grep snippets
- Verify all constant values by reading their definitions
- Confirm `#[cfg(test)]` gating by checking the actual Rust compiler directives
- Cross-reference findings with test files to understand expected behavior

## Deliverables
1. A modular code snippet file for each major function (< 100 lines each)
2. A flow diagram showing the complete path from input to prompt injection
3. A list of all constants and their exact values
4. A verification checklist confirming no assumptions were made
