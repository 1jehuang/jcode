# Task 1.2: MCP Tool Registration Investigation

## File Tree
```
Task 1.2 (MCP Tool Registration)
├── Used By: Phase 2 Synthesis
├── Uses Modules:
│   ├── jcode-app-core/src/tool/mcp.rs (mcp_search, McpSearchTool)
│   ├── jcode-app-core/src/agent/tools.rs (tool_output_references, tool_output_to_content_blocks)
│   ├── jcode-provider-anthropic/src/history_repair.rs (apply_tool_references)
│   ├── jcode-provider-openai/src/request.rs (insert_additional_tools, build_tool)
│   └── jcode-app-core/src/agent/turn_execution.rs (native_deferred_mcp)
```

## Objective
Map the complete code path from `mcp_search` returning results to deferred tools being loaded natively by providers without invalidating the KV cache.

## Sub-Sub-Agent Assignments

### Agent A: Trace `mcp_search`
**Target**: `jcode-app-core/src/tool/mcp.rs:154-302` (`McpSearchTool`)
**Questions**:
1. How does `fit_search_results` (line 91) adaptively trim descriptions and schemas to fit `SEARCH_TOKEN_BUDGET` (6000 tokens)?
2. How are `tool_references` added to metadata at line 300?
3. What is `MAX_SEARCH_TOOL_REFERENCES` (32) and how is it enforced?
4. How does `match_rank` prioritize name matches over description matches?
**Output**: Search result formatting logic with token budgeting details

### Agent B: Trace `tool_output_to_content_blocks`
**Target**: `jcode-app-core/src/agent/tools.rs:99-137`
**Questions**:
1. How does `tool_output_references` (line 83) extract names from metadata?
2. How are `ContentBlock::ToolReference` blocks created and appended (lines 127-134)?
3. Why do references come last (to avoid interfering with image folding)?
4. What is the exact structure of `TOOL_REFERENCES_METADATA_KEY`?
**Output**: Content block transformation diagram

### Agent C: Trace Anthropic `apply_tool_references`
**Target**: `jcode-provider-anthropic/src/history_repair.rs:155-243`
**Questions**:
1. How does it group references by `tool_use_id` using `sanitize_tool_id` (line 177)?
2. How does it replace tool result content with `ToolResultContentBlock::ToolReference` blocks (lines 199-206)?
3. How does it move original text to sibling blocks to keep them contiguous (lines 208-224)?
4. Why does the API reject mixing `tool_reference` blocks with other content?
**Output**: Anthropic-specific serialization flow

### Agent D: Trace OpenAI `insert_additional_tools`
**Target**: `jcode-provider-openai/src/request.rs:87-141`
**Questions**:
1. How does it find deferred tools in the `tools` array (lines 92-96)?
2. How does it match `ContentBlock::ToolReference` to `function_call_output` items (lines 102-123)?
3. How does `build_tool` (line 59) construct the JSON with `strict` normalization?
4. Where exactly are `additional_tools` inserted (after `function_call_output`, line 131)?
**Output**: OpenAI Responses API insertion flow

### Agent E: Investigate `native_deferred_mcp`
**Target**: `jcode-app-core/src/agent/turn_execution.rs:541-551`
**Questions**:
1. How does it check if the provider supports `defer_loading`?
2. What is the difference between eager, auto, and deferred modes?
3. How does `announce_late_mcp_tools` (line 711) skip announcement when native deferred is active?
4. What is `LATE_MCP_ANNOUNCEMENT_MARKER` (line 1548)?
**Output**: Provider capability detection logic

## Validation Requirements
- Read every function in full, not just grep snippets
- Verify all constant values by reading their definitions
- Compare Anthropic and OpenAI implementations side-by-side
- Cross-reference with test files in `jcode-provider-anthropic/src/deferred_tools_tests.rs`

## Deliverables
1. A modular code snippet file for each major function (< 100 lines each)
2. A comparison table showing Anthropic vs. OpenAI deferred loading differences
3. A list of all constants and their exact values
4. A verification checklist confirming no assumptions were made
