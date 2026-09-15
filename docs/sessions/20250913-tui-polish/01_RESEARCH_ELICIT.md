# Research: Elicitation / User Input Tools in AI Coding Agents

**Date**: 2026-09-14
**Purpose**: Understand how major AI coding agents handle structured user input requests (elicitation) mid-conversation, for TUI polish of Jcode's `elicitate_mcp` tool.

---

## Table of Contents

1. [MCP Elicitation Protocol (The Spec)](#1-mcp-elicitation-protocol-the-spec)
2. [Claude Code](#2-claude-code)
3. [Cursor](#3-cursor)
4. [Codex CLI (OpenAI)](#4-codex-cli-openai)
5. [Jcode's Current Implementation](#5-jcodes-current-implementation)
6. [Common Patterns & UI Mechanisms](#6-common-patterns--ui-mechanisms)
7. [Recommendations for Jcode](#7-recommendations-for-jcode)

---

## 1. MCP Elicitation Protocol (The Spec)

### Status: Fully specced and shipping in MCP 2025-11-25+

Elicitation is a **first-class MCP protocol feature** that allows servers to request structured input from users during tool execution. It is NOT a hack or workaround -- it is part of the official MCP specification.

**Spec URL**: https://modelcontextprotocol.io/docs/concepts/elicitation
**SDK Reference**: https://github.com/modelcontextprotocol/typescript-sdk (v2, targeting 2026-07-28 spec)

### Core Concept

The MCP elicitation protocol enables servers to pause mid-tool-execution, present a structured form or URL to the user, and receive the response before continuing. This is fundamentally different from "ask the LLM to ask the user" -- it is a **protocol-level mechanism** where the server communicates directly with the client's UI.

### Two Modes

#### Form Mode (Primary)
- Server sends an `elicitation/create` request with a JSON Schema (`requestedSchema`)
- Client renders a form UI based on the schema
- User fills out the form
- Client returns `ElicitResult` with `action` (accept/cancel/decline) and `content`

#### URL Mode (New in 2025-11-25)
- Server sends a URL for the user to navigate to
- Used for sensitive interactions (auth flows, payments) that must NOT pass through the MCP client
- Client displays the URL with domain info and gets user consent before navigating

### Supported Schema Types (Form Mode)

The schema is intentionally restricted to flat objects with primitive properties:

- **String**: text input, with optional `format` (email, uri, date, date-time), minLength, maxLength
- **Number/Integer**: numeric input with min/max
- **Boolean**: yes/no toggle
- **Enum (single-select)**: `enum` array or `oneOf` with const values
- **Enum (multi-select)**: `type: "array"` with `items.enum`
- All types support `default` values

**Intentionally NOT supported**: nested objects, arrays of objects, advanced JSON Schema features. This is a deliberate UX design choice to keep client implementations simple.

### Protocol Messages

#### Client Capability Declaration

Clients must advertise elicitation support via `_meta.io.modelcontextprotocol/clientCapabilities`:

```json
{
  "_meta": {
    "io.modelcontextprotocol/clientCapabilities": {
      "elicitation": {
        "form": {},
        "url": {}
      }
    }
  }
}
```

#### Server Elicitation Request (inside InputRequiredResult)

```json
{
  "method": "elicitation/create",
  "params": {
    "mode": "form",
    "message": "Please provide your GitHub username",
    "requestedSchema": {
      "type": "object",
      "properties": {
        "name": { "type": "string" }
      },
      "required": ["name"]
    }
  }
}
```

#### Client Response (inside inputResponses on retried request)

```json
{
  "action": "accept",
  "content": {
    "name": "octocat"
  }
}
```

#### Actions

- `accept` -- user submitted the form
- `cancel` -- user explicitly dismissed without answering
- `decline` -- user explicitly refused to provide the information

### Trust & Safety Rules

- Servers **MUST NOT** use form mode for passwords, API keys, access tokens, payment credentials
- Servers **MUST** use URL mode for such sensitive information
- Clients **MUST** make it clear which server is requesting information
- Clients **MUST** provide clear decline and cancel options
- Clients **MUST** allow users to review and modify responses before sending

### App-Rendered Elicitations (SEP-3118, In Progress)

**PR**: https://github.com/modelcontextprotocol/modelcontextprotocol/pull/3118

This is an advanced proposal that allows MCP Apps (embedded UI components) to render their own custom elicitation forms rather than relying on the client's generic form renderer. Key points:

- Preserves the core form elicitation as interoperable fallback
- Adds `ui.resourceUri` routing for capable hosts
- Negotiated via `io.modelcontextprotocol/clientCapabilities`
- Reference implementations in C# SDK, TypeScript SDK, and Inspector
- **Status**: Open PR, not yet merged

---

## 2. Claude Code

### AskUserQuestion Tool (Native)

Claude Code has a **native built-in tool** called `AskUserQuestion` that serves as its elicitation mechanism. This is NOT an MCP tool -- it is a first-class tool in Claude Code's tool set.

**Source**: https://docs.anthropic.com/en/docs/claude-code/tools

**Key characteristics**:
- **Permission required**: No (always available)
- **Behavior**: Asks multiple-choice questions to gather requirements or clarify ambiguity
- **Questions stay open** until the user answers them
- **Non-blocking for subagents**: Subagents can use it but the UX differs

From the docs:
> `AskUserQuestion` - Asks multiple-choice questions to gather requirements or clarify ambiguity. Questions stay open until you answer them by default.

### How Claude Code Handles MCP Elicitation

Claude Code's relationship with MCP elicitation is layered:

1. **Claude Code as MCP Client**: Claude Code can connect to MCP servers and handle their elicitation requests. The `AskUserQuestion` tool is the mechanism by which Claude Code surfaces elicitation to users.

2. **MCP Servers connected to Claude Code**: When an MCP server sends an `elicitation/create` request, Claude Code's client implementation handles it by presenting the form/questions to the user through its TUI.

3. **Claude Desktop vs Claude Code CLI**: Both support MCP elicitation, but the UI differs:
   - **Claude Desktop**: Renders elicitation as native OS dialogs/popups
   - **Claude Code CLI**: Renders elicitation inline in the terminal (TUI-style prompts)

### Other Claude Code User Interaction Tools

Claude Code has several tools that involve user interaction:

| Tool | Purpose | User Interaction Type |
|------|---------|----------------------|
| `AskUserQuestion` | Multiple-choice questions | Inline TUI prompt |
| `EnterPlanMode` / `ExitPlanMode` | Plan presentation and approval | Plan review + approval |
| `EndConversation` | End session | Confirmation |
| `PushNotification` | Desktop notifications | System notification |

### Claude Code MCP Configuration

Claude Code supports three transport types for MCP servers:
- **HTTP** (recommended for remote)
- **stdio** (local processes)
- **WebSocket** (persistent bidirectional)

The elicitation flow works across all transports because it is protocol-level, not transport-level.

---

## 3. Cursor

### No Native Elicitation Tool

Cursor does **not** have a dedicated elicitation tool comparable to Claude Code's `AskUserQuestion`. Instead, Cursor uses several interaction patterns:

### Plan Mode

Cursor's "Plan Mode" is the closest equivalent to structured elicitation:

- **Plan Mode**: Agent analyzes codebase and presents a plan before executing
- **User reviews the plan** and can approve, modify, or reject
- **No structured form** -- the plan is presented as markdown text
- **Approval is binary** (approve/reject) or via free-text comments

From Cursor docs:
> "Plan and build features -- Scope changes, use Plan Mode, and ship bigger work with confidence"

### Interactive Prompts in Cursor

Cursor's agent interaction is primarily through:
1. **Chat-based**: User types requests in the chat panel
2. **Plan review**: Agent presents plans, user approves/modifies
3. **Diff review**: Agent shows proposed changes, user accepts/rejects per-file
4. **MCP elicitation**: Cursor supports MCP servers but its handling of `elicitation/create` requests is limited compared to Claude Desktop

### Cursor MCP Support

Cursor supports MCP servers but the elicitation handling is less developed than Claude Desktop/Claude Code. The primary interaction model is:
- MCP servers provide tools
- Agent calls tools
- If a tool needs user input, the agent typically asks the user in chat (not via protocol-level elicitation)

### Key Difference from Claude Code

Cursor's approach is **agent-mediated** -- the LLM asks the user questions in natural language. Claude Code's approach is **protocol-mediated** -- the MCP server sends a structured request that the client renders directly. This is a fundamental architectural difference.

---

## 4. Codex CLI (OpenAI)

### No Native Elicitation Tool

OpenAI's Codex CLI (`openai/codex`) does **not** have a native elicitation tool. The interaction model is fundamentally different.

### Approval Modes

Codex CLI uses a **permission-based** model rather than elicitation:

- **Suggest mode**: Agent can only suggest changes, user must approve everything
- **Auto-edit mode**: Agent can edit files, but must get approval for shell commands
- **Full-auto mode**: Agent runs autonomously with minimal approval

From the Codex CLI README:
> "Lightweight coding agent that runs in your terminal"

### User Input Handling

Codex CLI handles user input through:
1. **Natural language chat**: User types requests in the terminal
2. **Implicit approval**: Pre-configured permission levels determine what needs approval
3. **No structured elicitation**: When Codex needs information, it asks in natural language within the conversation

### Codex Web (Cloud Agent)

The cloud-based Codex (chatgpt.com/codex) has a different model:
- Runs asynchronously
- User provides task description upfront
- Agent works in a sandbox
- Results delivered back
- No mid-task elicitation

### Codex in IDEs (VS Code, Cursor, Windsurf)

When used as an IDE extension, Codex inherits the host IDE's interaction model. In Cursor, this means Cursor's plan mode. In VS Code, this means the chat panel.

### OpenAI Agents SDK

OpenAI's Agents SDK has a "Guardrails / Approvals" concept:
- Tools can be configured with approval requirements
- Human-in-the-loop approvals are part of the orchestration
- This is the closest OpenAI gets to protocol-level elicitation

**Key Insight**: OpenAI's approach is to build permission/approval into the agent framework rather than into the MCP protocol. They assume the agent can ask questions naturally.

---

## 5. Jcode's Current Implementation

### The `elicitate_mcp` Tool

Jcode currently has the `mcp__phinbox__elicitate_mcp` (and `mcp__phinbox_mcp__elicitate_mcp`) tool, which is provided by the **Phinbox MCP server** (not by Jcode itself).

**Tool Name**: `elicitate_mcp`
**Description**: Render a native OS popup and block until the human operator responds

### Current Schema

The tool accepts a `PromptSpec` with:

```rust
// Core fields
title: String,           // One-line title
question: String,        // Multi-line body explaining context
field: FieldSpec,        // The input field configuration

// Optional
notes: Option<NotesSpec>,
buttons: Option<ButtonSpec>,
request_id: Option<String>,
timeout_secs: u32,       // Default: 600 (10 minutes)
urgency: Urgency,        // info | warning | error | secret
```

### FieldSpec Variants

The current implementation supports 6 field types:

| Kind | Description | UI |
|------|-------------|-----|
| `text` | Single-line text input | Text field |
| `long_text` | Multi-line text | Text area |
| `integer` | Integer in [min, max] | Number input |
| `choice` | Radio buttons / select | Choice list |
| `boolean` | Yes/no | Two buttons |
| `date_time` | Date/time picker | Picker widget |

### Urgency Levels

- `info` -- no sound, blue icon
- `warning` -- system sound, yellow icon
- `error` -- alert sound, red icon
- `secret` -- mask field, no logging

### How It Works (Architecture)

1. Jcode agent calls `elicitate_mcp` tool via MCP protocol
2. Phinbox MCP server receives the request
3. Phinbox renders a **native OS popup** (NSAlert on macOS)
4. User interacts with the popup
5. Phinbox returns `ElicitResponse` to Jcode
6. Jcode continues with the response

### Key Design Decision: Blocking

The current implementation is **synchronous/blocking** -- the tool call blocks until the user responds or the timeout expires. This is appropriate for the TUI use case because:
- The agent needs the user's input before it can continue
- The MCP protocol expects a response to the tool call
- The 10-minute timeout prevents indefinite hangs

---

## 6. Common Patterns & UI Mechanisms

### Pattern 1: OS-Level Dialogs (Native Popups)

**Used by**: Phinbox MCP, Claude Desktop

- Renders as a native OS dialog (NSAlert on macOS, Win32 MessageBox on Windows)
- **Pros**: Works across all applications, system-native look and feel, can play sounds
- **Cons**: Blocks the terminal/app, may not be visible if user is in another app
- **Best for**: Urgent requests, secrets, quick confirmations

### Pattern 2: Inline TUI Prompts

**Used by**: Claude Code CLI, Jcode (terminal mode)

- Renders as inline text in the terminal
- **Pros**: Stays in the terminal context, non-disruptive
- **Cons**: Only works in the terminal, harder to render complex forms
- **Best for**: Simple questions, multiple-choice, confirmations

### Pattern 3: Agent-Mediated Questions

**Used by**: Cursor, Codex CLI, most agents without native elicitation

- The LLM asks questions in natural language within the conversation
- User responds in the chat
- **Pros**: No special UI needed, works everywhere
- **Cons**: Ambiguous (is the agent asking or is the user directing?), no structured validation
- **Best for**: Exploratory questions, open-ended input

### Pattern 4: Plan/Review Loops

**Used by**: Claude Code (Plan mode), Cursor (Plan mode), most code review tools

- Agent presents a plan, user reviews and approves/modifies
- **Pros**: Structured review of complex decisions
- **Cons**: Overkill for simple questions, slows down autonomous work
- **Best for**: Architecture decisions, multi-step implementation plans

### Pattern 5: Permission Gates

**Used by**: Codex CLI (permission modes), Claude Code (permission rules)

- Pre-configured rules determine what needs approval
- Agent asks only when it hits a permission boundary
- **Pros**: Reduces interruption for routine tasks
- **Cons**: Can miss novel situations, requires upfront configuration
- **Best for**: File writes, shell commands, destructive operations

### Pattern 6: URL-Based Elicitation (MCP 2025-11-25+)

**Used by**: MCP servers requiring OAuth, payments, sensitive auth

- Server provides a URL, client opens it in a browser
- User completes the interaction in the browser
- Client receives the result
- **Pros**: Can use full web UIs, handles OAuth flows naturally
- **Cons**: Requires browser, more complex implementation
- **Best for**: OAuth, payments, complex forms, sensitive data entry

---

## 7. Recommendations for Jcode

### What Jcode Already Does Well

1. **Native OS popups** via Phinbox -- works cross-platform, looks native
2. **Structured input** -- FieldSpec covers common use cases (text, choice, boolean, date)
3. **Urgency levels** -- appropriate for different contexts
4. **Timeout handling** -- prevents indefinite blocking
5. **Secret handling** -- mask field for sensitive input

### Areas for Improvement

1. **Inline TUI fallback**: When running in a terminal without Phinbox, provide a TUI-based fallback using ratatui crossterm. This would make `elicitate_mcp` work even without the MCP server.

2. **Richer field types**: Consider adding:
   - File path selector (with browse button)
   - Password/secret field (masked, no logging) -- already partially covered by `secret` urgency
   - Multi-line confirmation (show what will happen, get approve/cancel)

3. **Non-blocking option**: For long-running operations that need periodic user input, consider a non-blocking variant that puts the question in a queue and the user can respond when ready.

4. **Multiple questions**: The MCP spec supports `requestedSchema` with multiple properties, but the current UI is a single popup. Consider whether multi-step wizards are needed.

5. **Context display**: Before showing the form, display what the agent was doing so the user understands why they're being asked.

### Architectural Consideration: Tool vs Protocol

Jcode currently has `elicitate_mcp` as an **MCP tool** (Phinbox server). Two approaches:

**Approach A: Keep as MCP tool (current)**
- Pro: Modular, works with any MCP client
- Pro: Phinbox handles the UI rendering
- Con: Requires Phinbox to be running
- Con: Tool name is long (`mcp__phinbox__elicitate_mcp`)

**Approach B: Build native elicitation into Jcode**
- Pro: Works without external MCP server
- Pro: Can use Jcode's own TUI for rendering
- Pro: Shorter tool name (`elicitate` or `ask_user`)
- Con: More code to maintain
- Con: Duplicates Phinbox's functionality

**Recommendation**: Start with Approach A (current) and add a lightweight native fallback for when Phinbox is unavailable.

---

## Sources

- MCP Elicitation Spec: https://modelcontextprotocol.io/docs/concepts/elicitation
- MCP TypeScript SDK: https://github.com/modelcontextprotocol/typescript-sdk
- MCP Schema (2025-03-26): https://github.com/modelcontextprotocol/modelcontextprotocol/blob/main/schema/2025-03-26/schema.json
- SEP-3118 App-Rendered Elicitations: https://github.com/modelcontextprotocol/modelcontextprotocol/pull/3118
- Claude Code Tools Reference: https://docs.anthropic.com/en/docs/claude-code/tools
- Claude Code MCP: https://docs.anthropic.com/en/docs/claude-code/mcp
- Claude Code Subagents: https://docs.anthropic.com/en/docs/claude-code/sub-agents
- OpenAI Codex CLI: https://github.com/openai/codex
- Codex CLI Docs: https://developers.openai.com/codex/cli
- Cursor Docs: https://docs.cursor.com/agent/plans-and-requests
- Cursor Prompting: https://docs.cursor.com/agent/prompting
- Cursor Tools: https://docs.cursor.com/agent/tools
