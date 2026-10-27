# Task 4.2: Integration Testing

## File Tree
```
Task 4.2 (Integration Testing)
├── Depends On: Task 4.1 (Unit Testing)
├── Used By: Phase 5 Production Readiness
├── Uses Modules:
│   ├── All extracted modules from Tasks 3.1-3.3
│   └── Integration test framework
├── Produces:
│   ├── Integration test suites
│   └── Cross-module interaction validation reports
```

## Objective
Implement integration tests for all cross-module interactions, validating that skill injection and MCP tool registration work correctly end-to-end within each subsystem.

## Sub-Sub-Agent Assignments

### Agent A: Test Skill Injection Pipeline End-to-End
**Goal**: Validate flow from TUI input → parsing → active_skill state → prompt injection
**Questions**:
1. How does `/optimization fix this` flow through the system?
2. How is `active_skill` set and validated against the registry?
3. How is the skill's full prompt appended to dynamic instructions?
4. How does late skill announcement preserve cache stability?
5. What happens if the skill is not found in the registry?
**Output**: Integration test suite for skill injection pipeline

### Agent B: Test MCP Tool Registration Pipeline End-to-End
**Goal**: Validate flow from mcp_search → tool_references → provider deferred loading
**Questions**:
1. How does `mcp_search` return `tool_references` metadata?
2. How are references converted to `ContentBlock::ToolReference`?
3. How does Anthropic's `apply_tool_references` process them?
4. How does OpenAI's `insert_additional_tools` insert deferred definitions?
5. How is KV cache stability preserved during late MCP announcements?
**Output**: Integration test suite for MCP tool registration pipeline

### Agent C: Test TUI → Agent → Provider Data Flow
**Goal**: Validate complete data flow across all layers
**Questions**:
1. How does user input travel from TUI to agent processing?
2. How is `active_skill` serialized in the wire protocol?
3. How does the agent receive and validate skill state?
4. How do providers serialize skill prompts and tool references?
5. What validation occurs at each layer boundary?
**Output**: Integration test suite for cross-layer data flow

### Agent D: Validate Cache Stability During Late Announcements
**Goal**: Ensure KV cache is not invalidated by mid-session changes
**Questions**:
1. How does `announce_late_skills` inject transcript messages?
2. Why does the system prompt remain cache-stable?
3. How does `seed_announced_skills_from_transcript` prevent duplicates?
4. What happens to cache when a new MCP server connects?
5. How do we measure cache hit rates during announcements?
**Output**: Cache stability validation report with metrics

## Validation Requirements
- Run all integration tests and verify they pass
- Check that cross-module boundaries are correctly tested
- Verify cache stability metrics meet targets (>95% hit rate)
- Ensure tests cover both success and failure paths

## Deliverables
1. Integration test suite for skill injection pipeline
2. Integration test suite for MCP tool registration pipeline
3. Integration test suite for cross-layer data flow
4. Cache stability validation report with hit rate metrics
5. Cross-module interaction validation checklist
