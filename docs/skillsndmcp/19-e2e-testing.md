# Task 4.3: End-to-End Testing

## File Tree
```
Task 4.3 (End-to-End Testing)
├── Depends On: Task 4.2 (Integration Testing)
├── Used By: Phase 5 Production Readiness
├── Uses Modules:
│   ├── All integrated modules from Phase 3
│   └── E2E test framework with TUI automation
├── Produces:
│   ├── E2E test scenarios for full user workflows
│   └── Workflow validation reports
```

## Objective
Implement end-to-end tests for complete user workflows, validating that the entire system works correctly from user keystroke to model response.

## Sub-Sub-Agent Assignments

### Agent A: Test Complete Skill Invocation Workflow
**Goal**: Validate `/optimization fix this` from keystroke to model response
**Questions**:
1. How does the user type `/optimization fix this` in the TUI?
2. How is the skill activated and the trailing prompt submitted?
3. How does the agent receive the skill state and inject the prompt?
4. How does the provider serialize the request with the skill prompt?
5. How do we verify the model receives the correct instructions?
**Output**: E2E test scenario for skill invocation workflow

### Agent B: Test Complete MCP Tool Discovery Workflow
**Goal**: Validate `mcp search calendar` from command to callable tools
**Questions**:
1. How does the user invoke `mcp search calendar`?
2. How does `mcp_search` return matching tools with schemas?
3. How are `tool_references` metadata added to the result?
4. How do providers load deferred tool definitions?
5. How do we verify the tools are callable in the next turn?
**Output**: E2E test scenario for MCP tool discovery workflow

### Agent C: Test Mid-Session Skill Installation and Late Announcement
**Goal**: Validate installing a new skill mid-session without restart
**Questions**:
1. How does a user add a new skill file to `.agents/skills/`?
2. How does `skill_manage reload_all` update the registry?
3. How does `announce_late_skills` inject the announcement?
4. How does the system prompt remain cache-stable?
5. How do we verify the new skill is immediately usable?
**Output**: E2E test scenario for mid-session skill installation

### Agent D: Test Error Handling and Fallback Mechanisms
**Goal**: Validate system behavior when things go wrong
**Questions**:
1. What happens if a skill name is invalid or contains special chars?
2. What happens if an MCP server connection fails?
3. What happens if a provider doesn't support deferred loading?
4. What happens if snapshot refresh fails mid-session?
5. How does the system recover from each error?
**Output**: E2E test scenarios for error handling and fallbacks

## Validation Requirements
- Run all E2E tests in a real TUI environment
- Verify complete workflows from user input to model response
- Check that error handling matches expected behavior
- Ensure tests cover both happy paths and failure modes

## Deliverables
1. E2E test scenario for skill invocation workflow
2. E2E test scenario for MCP tool discovery workflow
3. E2E test scenario for mid-session skill installation
4. E2E test scenarios for error handling and fallbacks
5. Workflow validation report with pass/fail status for each scenario
