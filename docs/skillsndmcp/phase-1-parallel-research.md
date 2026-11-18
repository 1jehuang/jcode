# Phase 1: Parallel Research & Investigation

## Overview
This phase contains independent research tasks that can be executed in parallel by multiple sub-agents. Each task investigates a specific aspect of the `/skills` and `/mcp` runtime behavior without depending on other tasks.

## File Tree Dependencies
```
Phase 1 (Root)
├── 01-skill-injection-mechanism.md
├── 02-mcp-tool-registration.md
├── 03-tui-command-routing.md
├── 04-provider-deferred-loading.md
├── 05-memory-system-integration.md
└── 06-wire-protocol-analysis.md
```

## Modular Code Requirements
- Each research agent must produce structured findings in a standardized JSON format
- All agents must cite exact file paths and line numbers from source code
- Agents must use sub-sub-agents for deep dives into specific functions
- No single research output should exceed 500 lines; split into modular findings files

## Sub-Agent Structure
Each phase file will spawn:
1. **Parent Agent**: Coordinates the research scope and validates findings
2. **Sub-Sub-Agents**: Perform deep code reading and function tracing
   - One sub-sub-agent per major function or code path
   - Each returns structured evidence with file:line references

## Tasks

### Task 1.1: Skill Injection Mechanism Investigation
**File**: `01-skill-injection-mechanism.md`
**Goal**: Map all code paths for skill injection into the system prompt
**Key Questions**:
- How does `resolve_invocation` parse skill commands?
- What is the exact flow from TUI input to `active_skill` state?
- How does `build_system_prompt_split` inject skill prompts?
- What are the exact boundaries between static and dynamic prompt parts?
**Sub-Sub-Agents Needed**:
- Agent A: Trace `resolve_invocation` in `jcode-base/src/skill.rs`
- Agent B: Trace `submit_input_with_routing` in `jcode-tui/src/tui/app/input.rs`
- Agent C: Trace `build_system_prompt_split` in `jcode-base/src/prompt.rs`
- Agent D: Investigate `announce_late_skills` in `jcode-app-core/src/agent/turn_execution.rs`

### Task 1.2: MCP Tool Registration Investigation
**File**: `02-mcp-tool-registration.md`
**Goal**: Map all code paths for MCP tool registration and deferred loading
**Key Questions**:
- How does `mcp_search` return `tool_references` metadata?
- How does `tool_output_to_content_blocks` create `ContentBlock::ToolReference`?
- How do Anthropic and OpenAI providers process these references?
- What is the exact KV cache preservation mechanism?
**Sub-Sub-Agents Needed**:
- Agent A: Trace `mcp_search` in `jcode-app-core/src/tool/mcp.rs`
- Agent B: Trace `apply_tool_references` in `jcode-provider-anthropic/src/history_repair.rs`
- Agent C: Trace `insert_additional_tools` in `jcode-provider-openai/src/request.rs`
- Agent D: Investigate `native_deferred_mcp` in `jcode-app-core/src/agent/turn_execution.rs`

### Task 1.3: TUI Command Routing Investigation
**File**: `03-tui-command-routing.md`
**Goal**: Map how `/skills` and `/mcp` are parsed and dispatched in the TUI
**Key Questions**:
- How does `slash_command_parser.rs` identify command boundaries?
- How does `handle_info_command` process `/skills` and `/mcp`?
- What is the difference between TUI display commands and agent tool calls?
- How does `refresh_skills_snapshot` work?
**Sub-Sub-Agents Needed**:
- Agent A: Trace `slash_command_parser.rs` boundary detection logic
- Agent B: Trace `handle_info_command` in `jcode-tui/src/tui/app/state_ui.rs`
- Agent C: Investigate `refresh_skills_snapshot` in `jcode-tui/src/tui/app/state_ui_runtime.rs`
- Agent D: Map the registered command table in `jcode-tui/src/tui/app/state_ui_input_helpers.rs`

### Task 1.4: Provider Deferred Loading Investigation
**File**: `04-provider-deferred-loading.md`
**Goal**: Map how different providers handle deferred tool definitions
**Key Questions**:
- How does the Anthropic provider serialize `ToolReference` blocks?
- How does the OpenAI Responses API insert `additional_tools`?
- What are the exact schema transformations applied?
- How does `sanitize_tool_id` ensure API compatibility?
**Sub-Sub-Agents Needed**:
- Agent A: Trace Anthropic `format_messages_with_native` in `jcode-provider-anthropic/src/lib.rs`
- Agent B: Trace OpenAI `build_tool` and `insert_additional_tools` in `jcode-provider-openai/src/request.rs`
- Agent C: Investigate `sanitize_tool_id` in `jcode-message-types/src/lib.rs`
- Agent D: Compare deferred loading across all supported providers

### Task 1.5: Memory System Integration Investigation
**File**: `05-memory-system-integration.md`
**Goal**: Verify the embedding-based skill injection claims and trace memory recall
**Key Questions**:
- Is `synthetic_skill_entries` truly gated behind `#[cfg(test)]`?
- What is the production memory recall pipeline?
- How does `with_skills` flag behave in production vs test?
- What is the exact flow of `get_relevant_parallel`?
**Sub-Sub-Agents Needed**:
- Agent A: Verify `#[cfg(test)]` gating in `jcode-base/src/memory.rs`
- Agent B: Trace `get_relevant_parallel` and `get_relevant_parallel_enabled`
- Agent C: Investigate `memory_jev::collect_scoped` in `jcode-base/src/memory_jev.rs`
- Agent D: Map `register_synthetic_entry_provider` usage in `cli/startup.rs`

### Task 1.6: Wire Protocol Analysis
**File**: `06-wire-protocol-analysis.md`
**Goal**: Map how `active_skill` flows through the wire protocol
**Key Questions**:
- How is `active_skill` serialized in the `Request` enum?
- How does `client_lifecycle.rs` receive and validate it?
- How does `set_remote_active_skill` work?
- What validation occurs at each layer?
**Sub-Sub-Agents Needed**:
- Agent A: Trace `Request` enum in `jcode-protocol/src/wire.rs`
- Agent B: Trace `handle_client` in `jcode-app-core/src/server/client_lifecycle.rs`
- Agent C: Investigate `set_remote_active_skill` in `jcode-app-core/src/agent.rs`
- Agent D: Map the full TUI → Wire → Agent flow

## Output Format
Each task must produce:
1. A structured findings document with exact file:line references
2. A modular code snippet library (each snippet < 100 lines)
3. A dependency map showing which files interact
4. A list of verified facts vs. assumptions needing validation

## Success Criteria
- Every major function in the skill/MCP pipeline is traced end-to-end
- All constant values (e.g., `LATE_SKILL_ANNOUNCEMENT_MARKER`) are documented
- All `#[cfg(test)]` gated code is identified and marked as non-production
- All provider-specific differences are mapped
- No assumptions remain unverified
