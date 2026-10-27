# Task 3.4: Add Documentation Headers

## File Tree
```
Task 3.4 (Add Documentation Headers)
├── Depends On: Tasks 3.1-3.3 (All extraction tasks)
├── Used By: Phase 4 Testing, Future maintenance
├── Uses Modules:
│   ├── All newly extracted modules from Tasks 3.1-3.3
│   └── Existing source files being documented
├── Produces:
│   ├── Updated documentation headers in all files
│   └── Dependency tree comments in all modules
```

## Objective
Add comprehensive documentation headers to all existing and newly extracted files explaining functionality, module dependencies, and usage relationships.

## Sub-Sub-Agent Assignments

### Agent A: Add Headers to Skill-Related Files
**Goal**: Document all skill parsing, validation, and prompt building modules
**Questions**:
1. What functionality does each skill module provide?
2. What modules does each file import/use?
3. What modules depend on each file?
4. How do we format the file tree in comments?
**Output**: Documentation headers for all skill-related files

### Agent B: Add Headers to MCP-Related Files
**Goal**: Document all MCP tool reference and deferred loading modules
**Questions**:
1. What is the purpose of each MCP handler module?
2. What are the cross-provider differences documented?
3. What external dependencies exist (Anthropic/OpenAI APIs)?
4. How do modules interact at runtime?
**Output**: Documentation headers for all MCP-related files

### Agent C: Add Headers to TUI-Related Files
**Goal**: Document all TUI input routing and state management modules
**Questions**:
1. What is the flow of data through each TUI module?
2. How do local and remote routing differ?
3. What wire protocol messages are involved?
4. What state is managed by each module?
**Output**: Documentation headers for all TUI-related files

### Agent D: Verify Header Accuracy Against Actual Dependencies
**Goal**: Validate that all documentation headers match actual code
**Questions**:
1. Do the "Uses Modules" lists match actual `use` statements?
2. Do the "Used By" lists match actual imports from other files?
3. Are function descriptions accurate?
4. Are file trees complete and up-to-date?
**Output**: Verification report with any discrepancies found

## Validation Requirements
- Read every file to verify header accuracy
- Check that all `use` statements are reflected in "Uses Modules"
- Verify that all files importing the module are in "Used By"
- Ensure no file exceeds 500 lines after adding headers

## Deliverables
1. Documentation headers added to all skill-related files
2. Documentation headers added to all MCP-related files
3. Documentation headers added to all TUI-related files
4. Verification report confirming header accuracy
5. Updated dependency graphs reflecting documented relationships
