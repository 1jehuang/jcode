# Task 2.1: Architecture Overview

## File Tree
```
Task 2.1 (Architecture Overview)
├── Depends On: All Phase 1 Tasks (01-08)
├── Used By: Phase 3 Implementation
├── Uses Modules:
│   ├── Findings from Task 1.1-1.8
│   └── jcode-base/src/prompt.rs (system prompt structure)
```

## Objective
Create a comprehensive system architecture diagram showing all components involved in skill injection and MCP tool registration, their interactions, cache boundaries, and failure modes.

## Sub-Sub-Agent Assignments

### Agent A: Map Complete Data Flow
**Goal**: Trace data from user input → TUI parsing → wire protocol → agent processing → provider serialization → model response
**Questions**:
1. What are the exact layers involved (TUI, CLI, Agent, Provider)?
2. Where does data transformation occur at each layer?
3. How does `active_skill` state flow through the system?
4. How do `tool_references` metadata flow from `mcp_search` to provider serialization?
**Output**: Mermaid sequence diagram showing complete data flow

### Agent B: Identify Cache Boundaries
**Goal**: Document all cache layers and their invalidation triggers
**Questions**:
1. What is cached in the KV cache (static prompt prefix)?
2. What is cached in the prompt cache (full system prompt)?
3. What is cached in the tool registry (MCP tool definitions)?
4. What triggers invalidation of each cache layer?
5. How do late skill/MCP announcements preserve cache stability?
**Output**: Cache hierarchy diagram with invalidation triggers

### Agent C: Document Failure Modes
**Goal**: Identify all possible failure points and recovery mechanisms
**Questions**:
1. What happens if skill parsing fails (invalid name, file drop)?
2. What happens if MCP server connection fails?
3. What happens if provider doesn't support deferred loading?
4. What happens if snapshot refresh fails mid-session?
5. How does the system recover from each failure?
**Output**: Failure mode table with recovery strategies

### Agent D: Create Component Interaction Diagrams
**Goal**: Produce Mermaid diagrams showing component interactions
**Questions**:
1. How do TUI, Agent, and Provider components interact?
2. What are the synchronization points?
3. Where are the async boundaries?
**Output**: Multiple Mermaid diagrams (component, sequence, state)

## Validation Requirements
- Read all Phase 1 findings before creating diagrams
- Verify cache boundaries against actual code (`turn_execution.rs`, `prompting.rs`)
- Cross-reference failure modes with error handling in source code
- Ensure diagrams match actual implementation, not assumptions

## Deliverables
1. Mermaid sequence diagram showing complete data flow
2. Cache hierarchy diagram with invalidation triggers
3. Failure mode table with recovery strategies
4. Component interaction diagrams (Mermaid format)
5. Architecture decision record explaining key design choices
