# Task 2.3: API Contracts

## File Tree
```
Task 2.3 (API Contracts)
├── Depends On: Task 2.1 (Architecture Overview), Task 2.2 (Module Boundaries)
├── Used By: Phase 3 Implementation, Phase 4 Testing
├── Uses Modules:
│   ├── Module specifications from Task 2.2
│   └── Wire protocol analysis from Phase 1
```

## Objective
Define explicit API contracts for all public interfaces including input/output schemas, validation rules, and wire protocol message formats.

## Sub-Sub-Agent Assignments

### Agent A: Define Skill Invocation Parsing Schemas
**Goal**: Create JSON schemas for skill parsing inputs and outputs
**Questions**:
1. What is the exact format of `/skillname prompt` input?
2. What validation rules apply to skill names (alphanumeric, hyphen, underscore)?
3. What is the structure of `SkillInvocation` output?
4. How are quoted prompts handled?
5. What error responses are returned for invalid input?
**Output**: JSON Schema for skill invocation parsing

### Agent B: Define MCP Tool Registration Schemas
**Goal**: Create schemas for MCP tool registration and deferred loading
**Questions**:
1. What is the structure of `tool_references` metadata?
2. What is the schema for `ContentBlock::ToolReference`?
3. What validation rules apply to tool names and server IDs?
4. How are deferred tool definitions serialized for Anthropic vs OpenAI?
5. What error responses are returned for invalid tool references?
**Output**: JSON Schema for MCP tool registration

### Agent C: Define Wire Protocol Message Formats
**Goal**: Document wire protocol messages for skill/MCP state
**Questions**:
1. How is `active_skill` serialized in the `Request` enum?
2. What is the structure of skill activation messages?
3. How are late announcements transmitted?
4. What is the format for MCP tool discovery responses?
5. How are errors propagated through the wire protocol?
**Output**: Wire protocol message specifications

### Agent D: Create Validation Rule Documentation
**Goal**: Document all validation rules with examples
**Questions**:
1. What characters are allowed in skill names?
2. What is the maximum length for skill descriptions?
3. What are the limits for `MAX_SEARCH_TOOL_REFERENCES`?
4. What sanitization is applied to tool IDs?
5. What security checks prevent injection attacks?
**Output**: Comprehensive validation rule table with examples

## Validation Requirements
- Verify schemas against actual Rust struct definitions
- Check validation rules against source code implementations
- Ensure wire protocol matches `jcode-protocol/src/wire.rs`
- Cross-reference with test fixtures for expected formats

## Deliverables
1. JSON Schema for skill invocation parsing
2. JSON Schema for MCP tool registration
3. Wire protocol message specifications
4. Validation rule table with examples
5. Error response schema documentation
