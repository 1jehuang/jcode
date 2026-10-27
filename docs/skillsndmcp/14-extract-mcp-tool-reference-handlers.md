# Task 3.2: Extract MCP Tool Reference Handlers

## File Tree
```
Task 3.2 (Extract MCP Tool Reference Handlers)
├── Depends On: Phase 2 Synthesis (09-12)
├── Used By: Phase 4 Testing
├── Uses Modules:
│   ├── Module boundaries from Task 2.2
│   └── API contracts from Task 2.3
├── Produces:
│   ├── tool_reference_extractor.rs (< 500 lines)
│   ├── tool_reference_serializer.rs (< 500 lines)
│   └── deferred_tool_loader.rs (< 500 lines)
```

## Objective
Extract MCP tool reference processing into reusable modules under 500 lines each with comprehensive documentation headers.

## Sub-Sub-Agent Assignments

### Agent A: Extract and Modularize Tool Reference Extraction
**Goal**: Create `tool_reference_extractor.rs` with `tool_output_references` and metadata extraction
**Questions**:
1. How do we extract `tool_output_references` from `jcode-app-core/src/agent/tools.rs`?
2. What is the structure of `TOOL_REFERENCES_METADATA_KEY`?
3. How do we extract references from `ToolOutput` metadata?
4. What validation is needed for reference names?
**Output**: Modularized `tool_reference_extractor.rs` with documentation header

### Agent B: Extract and Modularize Anthropic `apply_tool_references`
**Goal**: Create provider-specific serialization in `tool_reference_serializer.rs`
**Questions**:
1. How do we extract `apply_tool_references` from `jcode-provider-anthropic/src/history_repair.rs`?
2. How does it group references by `tool_use_id` using `sanitize_tool_id`?
3. How does it replace tool result content with `ToolResultContentBlock::ToolReference` blocks?
4. Why must original text be moved to sibling blocks?
**Output**: Anthropic-specific serialization module with documentation header

### Agent C: Extract and Modularize OpenAI `insert_additional_tools`
**Goal**: Create OpenAI-specific deferred loading logic
**Questions**:
1. How do we extract `insert_additional_tools` from `jcode-provider-openai/src/request.rs`?
2. How does it filter tools by `defer_loading` flag?
3. How does it match `ContentBlock::ToolReference` to `function_call_output` items?
4. How does `build_tool` construct the deferred definition JSON?
**Output**: OpenAI-specific serialization module with documentation header

### Agent D: Extract and Modularize `native_deferred_mcp` Checks
**Goal**: Create `deferred_tool_loader.rs` with capability detection
**Questions**:
1. How do we extract `native_deferred_mcp` from `jcode-app-core/src/agent/turn_execution.rs`?
2. What is the difference between eager, auto, and deferred modes?
3. How does `announce_late_mcp_tools` skip announcement when native deferred is active?
4. What is `LATE_MCP_ANNOUNCEMENT_MARKER`?
**Output**: Deferred loading capability detection module with documentation header

## Validation Requirements
- Verify each extracted file is under 500 lines
- Check that all provider-specific tests pass after extraction
- Ensure documentation headers are accurate
- Validate that cross-provider compatibility is preserved

## Deliverables
1. `tool_reference_extractor.rs` (< 500 lines) with documentation header
2. `tool_reference_serializer.rs` (< 500 lines) with documentation header
3. `deferred_tool_loader.rs` (< 500 lines) with documentation header
4. Updated dependency graphs showing new module relationships
5. Provider compatibility matrix documenting differences
