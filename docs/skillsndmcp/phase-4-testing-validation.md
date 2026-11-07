# Phase 4: Testing & Validation

## Overview
This phase implements comprehensive testing and validation for all modules created in Phase 3. It ensures production readiness through unit tests, integration tests, end-to-end tests, and performance validation.

## File Tree Dependencies
```
Phase 4 (Testing & Validation)
├── Depends On: Phase 3 Implementation
├── Produces:
│   ├── Unit tests for all modules
│   ├── Integration tests for cross-module interactions
│   ├── End-to-end tests for full pipelines
│   └── Performance benchmarks and validation reports
```

## Modular Code Requirements
- **Test Coverage**: Every module must have >90% code coverage
- **Test Modularity**: Test files must also be under 500 lines; split large test suites
- **Validation Reports**: Each test suite must produce a structured validation report
- **Sub-Agent Usage**: Use sub-agents for parallel test execution across different providers

## Sub-Agent Structure
Each testing task will spawn:
1. **Parent Agent**: Coordinates test execution and validates coverage thresholds
2. **Sub-Sub-Agents**: 
   - One per test category (unit, integration, e2e)
   - Each executes tests and produces validation reports
   - Returns structured test results with coverage metrics

## Tasks

### Task 4.1: Unit Testing
**Goal**: Implement unit tests for all extracted modules with >90% coverage
**Modules to Test**:
- `skill_parser.rs`: Test multi-word name resolution, quote handling, fallback parsing
- `skill_validator.rs`: Test character validation, security checks, file drop rejection
- `tool_reference_extractor.rs`: Test metadata extraction, reference conversion
- `tool_reference_serializer.rs`: Test Anthropic and OpenAI serialization
- `tui_input_router.rs`: Test local and remote input routing logic
**Sub-Sub-Agents Needed**:
- Agent A: Write unit tests for skill parsing utilities
- Agent B: Write unit tests for MCP tool reference handlers
- Agent C: Write unit tests for TUI input routing
- Agent D: Generate coverage reports and identify gaps

### Task 4.2: Integration Testing
**Goal**: Implement integration tests for cross-module interactions
**Integration Points to Test**:
- Skill invocation → `active_skill` state → prompt injection
- `mcp_search` → `tool_references` → provider deferred loading
- TUI input → wire protocol → agent processing
**Sub-Sub-Agents Needed**:
- Agent A: Test skill injection pipeline end-to-end
- Agent B: Test MCP tool registration pipeline end-to-end
- Agent C: Test TUI → Agent → Provider data flow
- Agent D: Validate cache stability during late announcements

### Task 4.3: End-to-End Testing
**Goal**: Implement end-to-end tests for full user workflows
**Workflows to Test**:
- User types `/optimization fix this` → skill activated → prompt injected → model responds
- User runs `mcp search calendar` → tools found → deferred loading → tools callable
- User installs new skill mid-session → late announcement → skill usable without restart
**Sub-Sub-Agents Needed**:
- Agent A: Test complete skill invocation workflow
- Agent B: Test complete MCP tool discovery workflow
- Agent C: Test late skill/MCP announcement workflow
- Agent D: Test error handling and fallback mechanisms

### Task 4.4: Performance Validation
**Goal**: Validate performance characteristics of all modules
**Metrics to Measure**:
- Skill parsing latency (< 1ms for single-token, < 5ms for multi-word)
- MCP tool reference serialization latency (< 10ms for 32 tools)
- KV cache hit rate during late announcements (> 95%)
- Memory usage of modularized code vs. original monolithic files
**Sub-Sub-Agents Needed**:
- Agent A: Benchmark skill parsing performance
- Agent B: Benchmark MCP tool reference processing
- Agent C: Measure KV cache hit rates
- Agent D: Compare memory usage before and after modularization

## Output Format
Each task must produce:
1. Test files with >90% coverage for each module
2. Integration test suites for all cross-module interactions
3. End-to-end test scenarios for full user workflows
4. Performance benchmark reports with metrics

## Success Criteria
- All modules have >90% test coverage
- All integration points are tested and validated
- End-to-end workflows pass all test scenarios
- Performance metrics meet or exceed targets
- No regressions in functionality or performance
