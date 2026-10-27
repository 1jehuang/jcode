# Phase 2: Synthesis & Architecture Design

## Overview
This phase synthesizes findings from all Phase 1 parallel research tasks into a coherent architecture document. It identifies gaps, validates assumptions, and designs the modular code structure for production readiness.

## File Tree Dependencies
```
Phase 2 (Synthesis)
├── Depends On: All Phase 1 Tasks (01-08)
├── Produces:
│   ├── 09-architecture-overview.md
│   ├── 10-module-boundaries.md
│   ├── 11-api-contracts.md
│   └── 12-testing-strategy.md
```

## Modular Code Requirements
- Synthesis must produce reusable architecture diagrams in Mermaid format
- Each module boundary must be documented with explicit input/output contracts
- API contracts must include request/response schemas with validation rules
- Testing strategy must cover unit, integration, and end-to-end scenarios

## Sub-Agent Structure
Each synthesis task will spawn:
1. **Parent Agent**: Integrates findings from Phase 1 and identifies architectural patterns
2. **Sub-Sub-Agents**: 
   - One per major architectural concern (e.g., caching, security, modularity)
   - Each validates cross-cutting concerns across all Phase 1 findings
   - Returns structured architecture decisions with rationale

## Tasks

### Task 2.1: Architecture Overview
**File**: `09-architecture-overview.md`
**Goal**: Create a comprehensive system architecture diagram showing all components and their interactions
**Key Questions**:
- How do the TUI, agent, and provider layers interact?
- Where are the cache boundaries (KV cache, prompt cache, tool registry)?
- What are the failure modes and recovery mechanisms?
**Sub-Sub-Agents Needed**:
- Agent A: Map the complete data flow from user input to model response
- Agent B: Identify all cache layers and their invalidation triggers
- Agent C: Document error handling and fallback mechanisms
- Agent D: Create Mermaid diagrams showing component interactions

### Task 2.2: Module Boundaries
**File**: `10-module-boundaries.md`
**Goal**: Define clear module boundaries to keep files under 500 lines and ensure reusability
**Key Questions**:
- Which functions should be extracted into separate modules?
- What are the dependencies between modules?
- How can we reduce coupling between skill injection and MCP tool registration?
**Sub-Sub-Agents Needed**:
- Agent A: Analyze file sizes and identify candidates for extraction
- Agent B: Map inter-module dependencies and suggest decoupling strategies
- Agent C: Design reusable utility modules (e.g., prompt building, tool reference handling)
- Agent D: Create a module dependency graph

### Task 2.3: API Contracts
**File**: `11-api-contracts.md`
**Goal**: Define explicit API contracts for all public interfaces
**Key Questions**:
- What are the input/output schemas for each major function?
- What validation rules apply to skill names, tool references, and prompts?
- How do wire protocol messages map to internal data structures?
**Sub-Sub-Agents Needed**:
- Agent A: Define schemas for skill invocation parsing
- Agent B: Define schemas for MCP tool registration
- Agent C: Define wire protocol message formats
- Agent D: Create validation rule documentation

### Task 2.4: Testing Strategy
**File**: `12-testing-strategy.md`
**Goal**: Design a comprehensive testing strategy covering all edge cases
**Key Questions**:
- What unit tests are needed for each module?
- How do we test cross-provider deferred loading behavior?
- What integration tests validate the full skill/MCP pipeline?
**Sub-Sub-Agents Needed**:
- Agent A: Design unit tests for parsing and validation logic
- Agent B: Design integration tests for TUI → Agent → Provider flow
- Agent C: Design end-to-end tests for late skill/MCP announcements
- Agent D: Create test fixtures and mock data structures

## Output Format
Each task must produce:
1. A structured architecture document with Mermaid diagrams
2. A module boundary definition with explicit dependencies
3. An API contract specification with validation rules
4. A testing strategy document with test case outlines

## Success Criteria
- All Phase 1 findings are integrated into a coherent architecture
- Module boundaries are clearly defined with no file exceeding 500 lines
- API contracts are explicit and validated against actual code
- Testing strategy covers all identified edge cases and failure modes
