# Task 4.1: Unit Testing

## File Tree
```
Task 4.1 (Unit Testing)
├── Depends On: Phase 3 Implementation (13-16)
├── Used By: Phase 5 Production Readiness
├── Uses Modules:
│   ├── All extracted modules from Tasks 3.1-3.3
│   └── Test framework utilities
├── Produces:
│   ├── Unit test files for each module
│   └── Coverage reports (>90% target)
```

## Objective
Implement comprehensive unit tests for all extracted modules with >90% code coverage, ensuring every function and edge case is tested.

## Sub-Sub-Agent Assignments

### Agent A: Write Unit Tests for Skill Parsing Utilities
**Goal**: Test `skill_parser.rs`, `skill_validator.rs`, and `skill_prompt_builder.rs`
**Questions**:
1. How do we test multi-word skill name resolution?
2. What are the edge cases for quote handling in prompts?
3. How do we test file drop rejection (`/tmp/screenshot.png`)?
4. What are the boundary conditions for `clip_skill_description`?
5. How do we test UTF-8 safety in clipping?
**Output**: Unit test file with >90% coverage for skill parsing modules

### Agent B: Write Unit Tests for MCP Tool Reference Handlers
**Goal**: Test `tool_reference_extractor.rs`, `tool_reference_serializer.rs`, and `deferred_tool_loader.rs`
**Questions**:
1. How do we test metadata extraction from `ToolOutput`?
2. How do we test Anthropic-specific `apply_tool_references` logic?
3. How do we test OpenAI-specific `insert_additional_tools` logic?
4. How do we test `sanitize_tool_id` with various inputs?
5. How do we test deferred loading capability detection?
**Output**: Unit test file with >90% coverage for MCP handler modules

### Agent C: Write Unit Tests for TUI Input Routing
**Goal**: Test `tui_input_router.rs`, `tui_skill_activator.rs`, and `tui_snapshot_refresher.rs`
**Questions**:
1. How do we test local vs remote input routing differences?
2. How do we test skill activation state management?
3. How do we test synchronous snapshot refresh from disk?
4. How do we test endorsed-but-not-installed skill hints?
5. How do we test SSH remote mode skipping?
**Output**: Unit test file with >90% coverage for TUI routing modules

### Agent D: Generate Coverage Reports and Identify Gaps
**Goal**: Run coverage analysis and identify untested code paths
**Questions**:
1. Which functions have <90% coverage?
2. What edge cases are missing from current tests?
3. Are there any unreachable code paths?
4. What additional tests are needed to reach 90% target?
**Output**: Coverage report with gap analysis and remediation plan

## Validation Requirements
- Run all tests and verify they pass
- Check coverage reports show >90% for each module
- Verify edge cases are covered (empty input, special chars, etc.)
- Ensure tests are modular and don't exceed 500 lines each

## Deliverables
1. Unit test file for skill parsing utilities
2. Unit test file for MCP tool reference handlers
3. Unit test file for TUI input routing
4. Coverage report showing >90% for all modules
5. Gap analysis with remediation plan for any uncovered code
