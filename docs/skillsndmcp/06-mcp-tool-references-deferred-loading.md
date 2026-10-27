# Task 1.6: MCP Tool References and Deferred Loading Pipeline

## File Tree
```
Task 1.6 (MCP Tool References and Deferred Loading)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-app-core/src/tool/mcp.rs (mcp_search, tool_references metadata)
│   ├── jcode-app-core/src/agent/tools.rs (tool_output_to_content_blocks, tool_output_references)
│   ├── jcode-provider-anthropic/src/history_repair.rs (apply_tool_references)
│   └── jcode-provider-openai/src/request.rs (insert_additional_tools, build_tool)
```

## Objective
Trace the exact flow from `mcp_search` returning `tool_references` metadata to the provider loading deferred tool definitions without invalidating the KV cache.

## Sub-Sub-Agent Assignments

### Agent A: Trace `mcp_search` metadata injection
**Target**: `jcode-app-core/src/tool/mcp.rs:289-301`
**Questions**:
1. How does it collect up to 32 tool names into the `references` vector (lines 293-297)?
2. Why is the limit set to `MAX_SEARCH_TOOL_REFERENCES`?
3. How does it add this as `tool_references` metadata in the `ToolOutput` (line 300)?
4. What happens if the result set is larger than the budget (via `fit_search_results`)?
**Output**: Metadata injection logic with token budget handling

### Agent B: Trace `tool_output_to_content_blocks` conversion
**Target**: `jcode-app-core/src/agent/tools.rs:99-136`
**Questions**:
1. How does it extract references via `tool_output_references` (line 103)?
2. How does it convert each reference name into a `ContentBlock::ToolReference` (lines 127-134)?
3. Why are these blocks appended *after* the main tool result and any images (comment at line 124)?
4. How does it clone the `tool_use_id` for each reference block (line 131)?
**Output**: Content block conversion logic with ordering rationale

### Agent C: Trace Anthropic `apply_tool_references`
**Target**: `jcode-provider-anthropic/src/history_repair.rs:155-243`
**Questions**:
1. How does it group references by `tool_use_id` using `sanitize_tool_id` (lines 162-181)?
2. Why does it replace the tool result content with `ToolResultContentBlock::ToolReference` blocks (lines 199-207)?
3. How does it move original text content to a sibling text block to keep them contiguous (lines 208-227)?
4. What API constraint forces this separation of references from other content?
**Output**: Anthropic-specific processing with API constraint explanation

### Agent D: Trace OpenAI `insert_additional_tools`
**Target**: `jcode-provider-openai/src/request.rs:87-141`
**Questions**:
1. How does it filter tools by `defer_loading` flag (lines 92-96)?
2. How does it match `ContentBlock::ToolReference` blocks to their corresponding `function_call_output` (lines 102-123)?
3. How does it use `build_tool` to construct the deferred definition JSON (line 136)?
4. Where exactly does it insert the `additional_tools` block in the input array (lines 131-138)?
**Output**: OpenAI-specific insertion logic with schema building

### Agent E: Investigate `sanitize_tool_id` utility
**Target**: `jcode-message-types/src/lib.rs:589-608`
**Questions**:
1. How does it replace non-alphanumeric characters (except `_` and `-`) with underscores (lines 593-601)?
2. Why is this sanitization necessary for cross-provider compatibility?
3. What happens if the input ID is empty or results in an empty string (lines 590-591, 603-604)?
4. How does this affect tool call tracking across different providers?
**Output**: Sanitization logic with cross-provider compatibility rationale

## Validation Requirements
- Read the `ToolDefinition::defer_loading` field definition in `jcode-message-types/src/lib.rs`
- Verify the `MAX_SEARCH_TOOL_REFERENCES` constant value in `mcp.rs`
- Check the test in `deferred_tools.rs` that validates reference block creation
- Cross-reference with Anthropic's `ToolResultContentBlock::ToolReference` enum variant

## Deliverables
1. A modular code snippet for `tool_output_to_content_blocks` (< 40 lines)
2. A modular code snippet for `apply_tool_references` (< 50 lines)
3. A sequence diagram showing the full pipeline from search to provider injection
4. A compatibility matrix showing how different providers handle deferred tools
