# Task 2.2: Module Boundaries

## File Tree
```
Task 2.2 (Module Boundaries)
├── Depends On: Task 2.1 (Architecture Overview)
├── Used By: Phase 3 Implementation
├── Uses Modules:
│   ├── Architecture diagrams from Task 2.1
│   └── Source code analysis from Phase 1
```

## Objective
Define clear module boundaries to keep files under 500 lines and ensure reusability. Identify which functions should be extracted into separate modules and map inter-module dependencies.

## Sub-Sub-Agent Assignments

### Agent A: Analyze File Sizes and Extraction Candidates
**Goal**: Identify all files exceeding 500 lines and propose extraction strategies
**Questions**:
1. Which files in `jcode-base/src/prompt.rs`, `jcode-app-core/src/tool/mcp.rs`, etc. exceed 500 lines?
2. What are natural boundaries within these large files?
3. Which functions are reusable across multiple contexts?
4. What would be the impact of extracting each candidate?
**Output**: List of extraction candidates with rationale

### Agent B: Map Inter-Module Dependencies
**Goal**: Create dependency graph showing relationships between proposed modules
**Questions**:
1. What does each proposed module depend on?
2. What depends on each proposed module?
3. Are there circular dependencies that need breaking?
4. How can we reduce coupling between skill injection and MCP tool registration?
**Output**: Dependency graph (Mermaid format)

### Agent C: Design Reusable Utility Modules
**Goal**: Propose shared utility modules for common functionality
**Questions**:
1. What logic is duplicated across skill parsing and MCP handling?
2. What prompt-building utilities could be shared?
3. What tool reference handling could be generalized?
4. What validation logic could be centralized?
**Output**: Proposed utility module specifications

### Agent D: Create Module Dependency Graph
**Goal**: Produce comprehensive dependency visualization
**Questions**:
1. What is the complete module hierarchy after extraction?
2. What are the import/export boundaries?
3. How do modules interact at runtime?
**Output**: Complete module dependency graph (Mermaid)

## Validation Requirements
- Verify file sizes by reading actual source files
- Check for circular dependencies in proposed structure
- Ensure proposed modules align with Rust crate organization
- Validate that extraction doesn't break existing tests

## Deliverables
1. List of extraction candidates with file:line references
2. Dependency graph showing module relationships
3. Utility module specifications with API contracts
4. Complete module hierarchy diagram
5. Refactoring roadmap with priority ordering
