# Phase 3: Implementation & Modularization

## Overview
This phase implements the architectural design from Phase 2, focusing on modular code extraction, file size reduction, and comprehensive documentation. Each module must be under 500 lines with clear file trees showing dependencies and users.

## File Tree Dependencies
```
Phase 3 (Implementation)
├── Depends On: Phase 2 Synthesis (09-12)
├── Produces:
│   ├── Modularized source files (< 500 lines each)
│   ├── Documentation headers in all files
│   ├── File tree comments showing dependencies
│   └── Integration tests for all modules
```

## Modular Code Requirements
- **File Size Limit**: No single file exceeds 500 lines; extract reusable snippets into separate modules
- **Documentation Header**: Every file must have a comment block at the top explaining:
  - Functionality of the file
  - File tree of modules it uses
  - File tree of modules that use it
- **Reusability**: Extract common logic (e.g., prompt building, tool reference handling) into shared utilities
- **Sub-Agent Usage**: Use sub-agents for parallel implementation of independent modules

## Sub-Agent Structure
Each implementation task will spawn:
1. **Parent Agent**: Coordinates module extraction and ensures file size limits
2. **Sub-Sub-Agents**: 
   - One per module to be extracted or refactored
   - Each implements a specific module with full documentation
   - Returns modularized code with dependency trees

## Tasks

### Task 3.1: Extract Skill Parsing Utilities
**Goal**: Extract skill invocation parsing logic into reusable modules under 500 lines each
**Modules to Create**:
- `skill_parser.rs`: Contains `resolve_invocation` and `parse_invocation` logic
- `skill_validator.rs`: Contains character validation and security checks
- `skill_prompt_builder.rs`: Contains `build_available_skills_section` and `clip_skill_description`
**Sub-Sub-Agents Needed**:
- Agent A: Extract and modularize `resolve_invocation` with multi-word support
- Agent B: Extract and modularize `parse_invocation` with security validations
- Agent C: Extract and modularize prompt building utilities
- Agent D: Add documentation headers with file trees to all new modules

### Task 3.2: Extract MCP Tool Reference Handlers
**Goal**: Extract MCP tool reference processing into reusable modules under 500 lines each
**Modules to Create**:
- `tool_reference_extractor.rs`: Contains `tool_output_references` and metadata extraction
- `tool_reference_serializer.rs`: Contains provider-specific serialization (Anthropic/OpenAI)
- `deferred_tool_loader.rs`: Contains `native_deferred_mcp` checks and loading logic
**Sub-Sub-Agents Needed**:
- Agent A: Extract and modularize tool reference extraction from `ToolOutput`
- Agent B: Extract and modularize Anthropic `apply_tool_references` logic
- Agent C: Extract and modularize OpenAI `insert_additional_tools` logic
- Agent D: Add documentation headers with file trees to all new modules

### Task 3.3: Extract TUI Input Routing Logic
**Goal**: Extract TUI input routing into reusable modules under 500 lines each
**Modules to Create**:
- `tui_input_router.rs`: Contains `submit_input_with_routing` logic
- `tui_skill_activator.rs`: Contains skill activation and `active_skill` state management
- `tui_snapshot_refresher.rs`: Contains `refresh_skills_snapshot` logic
**Sub-Sub-Agents Needed**:
- Agent A: Extract and modularize local TUI input routing
- Agent B: Extract and modularize remote TUI input routing
- Agent C: Extract and modularize snapshot refresh logic
- Agent D: Add documentation headers with file trees to all new modules

### Task 3.4: Add Documentation Headers
**Goal**: Add comprehensive documentation headers to all existing and new files
**Requirements**:
- Each header must include:
  - Functionality description
  - File tree of modules used by this file
  - File tree of modules that use this file
- Headers must be kept up-to-date as dependencies change
**Sub-Sub-Agents Needed**:
- Agent A: Add headers to all skill-related files
- Agent B: Add headers to all MCP-related files
- Agent C: Add headers to all TUI-related files
- Agent D: Verify header accuracy against actual dependencies

## Output Format
Each task must produce:
1. Modularized source files under 500 lines each
2. Documentation headers with file trees in all files
3. Updated dependency graphs showing new module relationships
4. Unit tests for all extracted modules

## Success Criteria
- No file exceeds 500 lines after modularization
- All files have accurate documentation headers with file trees
- All extracted modules are reusable and well-tested
- Dependency graphs are updated to reflect new module structure
