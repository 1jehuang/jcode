# Task 3.1: Extract Skill Parsing Utilities

## File Tree
```
Task 3.1 (Extract Skill Parsing Utilities)
├── Depends On: Phase 2 Synthesis (09-12)
├── Used By: Phase 4 Testing
├── Uses Modules:
│   ├── Module boundaries from Task 2.2
│   └── API contracts from Task 2.3
├── Produces:
│   ├── skill_parser.rs (< 500 lines)
│   ├── skill_validator.rs (< 500 lines)
│   └── skill_prompt_builder.rs (< 500 lines)
```

## Objective
Extract skill invocation parsing logic into reusable modules under 500 lines each with comprehensive documentation headers.

## Sub-Sub-Agent Assignments

### Agent A: Extract and Modularize `resolve_invocation`
**Goal**: Create `skill_parser.rs` with multi-word name resolution
**Questions**:
1. How do we extract `resolve_invocation` from `jcode-base/src/skill/invocation.rs`?
2. What dependencies does it need (SkillRegistry, SkillInvocation)?
3. How do we handle the fallback to `parse_invocation`?
4. What tests need to be migrated?
**Output**: Modularized `skill_parser.rs` with documentation header

### Agent B: Extract and Modularize `parse_invocation` with Security Validations
**Goal**: Create `skill_validator.rs` with character validation and security checks
**Questions**:
1. How do we extract `parse_invocation` from `jcode-base/src/skill.rs`?
2. What character validation rules must be preserved (alphanumeric, hyphen, underscore)?
3. How do we prevent file drops like `/tmp/screenshot.png`?
4. What security checks are needed for prompt injection prevention?
**Output**: Modularized `skill_validator.rs` with documentation header

### Agent C: Extract and Modularize Prompt Building Utilities
**Goal**: Create `skill_prompt_builder.rs` with `build_available_skills_section` and `clip_skill_description`
**Questions**:
1. How do we extract `build_available_skills_section` from `jcode-base/src/prompt.rs`?
2. How do we extract `clip_skill_description` with UTF-8 safety?
3. What is the value of `SKILL_DESC_MAX_CHARS` constant?
4. How do we preserve the markdown formatting in the skills section?
**Output**: Modularized `skill_prompt_builder.rs` with documentation header

### Agent D: Add Documentation Headers with File Trees
**Goal**: Add comprehensive documentation to all new modules
**Questions**:
1. What functionality does each module provide?
2. What modules does each file use?
3. What modules use each file?
4. How do we keep headers up-to-date as dependencies change?
**Output**: Documentation headers for all three new modules

## Validation Requirements
- Verify each extracted file is under 500 lines
- Check that all tests pass after extraction
- Ensure documentation headers are accurate
- Validate that no functionality is lost in extraction

## Deliverables
1. `skill_parser.rs` (< 500 lines) with documentation header
2. `skill_validator.rs` (< 500 lines) with documentation header
3. `skill_prompt_builder.rs` (< 500 lines) with documentation header
4. Updated dependency graphs showing new module relationships
5. Migration guide for existing code using extracted functions
