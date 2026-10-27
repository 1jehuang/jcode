# Task 5.1: User-Facing Documentation

## File Tree
```
Task 5.1 (User-Facing Documentation)
├── Depends On: Phase 4 Testing & Validation (17-20)
├── Used By: End users, support team
├── Uses Modules:
│   ├── All validated features from Phase 4
│   └── Existing user documentation structure
├── Produces:
│   ├── USER_GUIDE_SKILLS.md
│   ├── USER_GUIDE_MCP.md
│   └── TROUBLESHOOTING.md
```

## Objective
Create comprehensive user-facing documentation for skill and MCP features, including usage guides, examples, and troubleshooting steps.

## Sub-Sub-Agent Assignments

### Agent A: Write User Guide for Skill Invocation and Creation
**Goal**: Document how users can use and create custom skills
**Questions**:
1. How do users invoke skills with `/skillname prompt`?
2. How do users create custom skills in `.agents/skills/`?
3. What is the SKILL.md format with frontmatter?
4. How do multi-word skill names work?
5. What are best practices for writing effective skills?
**Output**: USER_GUIDE_SKILLS.md with examples and best practices

### Agent B: Write User Guide for MCP Server Connection and Tool Usage
**Goal**: Document how users can connect MCP servers and use tools
**Questions**:
1. How do users configure MCP servers in `~/.jcode/mcp.json`?
2. How do users connect to local vs remote MCP servers?
3. How does `mcp search <query>` discover tools?
4. How do deferred-loaded tools become callable?
5. What are best practices for MCP server configuration?
**Output**: USER_GUIDE_MCP.md with configuration examples

### Agent C: Create Troubleshooting Guide with Common Error Scenarios
**Goal**: Document common issues and their solutions
**Questions**:
1. What happens if a skill name is not found?
2. What happens if an MCP server connection fails?
3. How do users debug skill parsing issues?
4. How do users verify MCP tool availability?
5. What are common error messages and their meanings?
**Output**: TROUBLESHOOTING.md with error scenarios and solutions

### Agent D: Validate Documentation Against Actual User Workflows
**Goal**: Ensure documentation matches real user experience
**Questions**:
1. Do the examples in the docs work as described?
2. Are all command syntaxes accurate?
3. Are error messages documented correctly?
4. Is the troubleshooting guide comprehensive?
5. Are there any gaps in the documentation?
**Output**: Documentation validation report with suggested improvements

## Validation Requirements
- Test all examples in the documentation to ensure they work
- Verify command syntax matches actual implementation
- Check that error messages are accurately documented
- Ensure troubleshooting guide covers common user issues

## Deliverables
1. USER_GUIDE_SKILLS.md with invocation and creation examples
2. USER_GUIDE_MCP.md with configuration and usage examples
3. TROUBLESHOOTING.md with common error scenarios and solutions
4. Documentation validation report with accuracy verification
5. User feedback collection plan for ongoing improvements
