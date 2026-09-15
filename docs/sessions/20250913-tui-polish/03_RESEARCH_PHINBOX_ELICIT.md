# Research: Phinbox Elicitate MCP Tool for Jcode TUI Integration

**Date:** 2026-09-14  
**Status:** Research only - no code changes  
**Session:** TUI Polish (20250913-tui-polish)

---

## 1. Phinbox Elicitate MCP Tool

### Tool Definitions

The jcode agent has access to two MCP tool endpoints, both rendering native OS popups:

#### `mcp__phinbox__elicitate_mcp` / `mcp__phinbox_mcp__elicitate_mcp`

**Purpose:** Renders a native OS popup and blocks until the human operator responds (or the prompt times out). Use whenever an autonomous agent needs a single, structured decision from a human.

**Parameters (from tool schema):**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `title` | string | **Yes** | One-line title |
| `field` | FieldSpec | **Yes** | Input field configuration (see below) |
| `intent` | string | **Yes** | Required short label shown in the UI |
| `question` | string | No | Multi-line body explaining context |
| `notes` | NotesSpec | No | Optional notes/free-text box |
| `buttons` | ButtonSpec | No | Custom button labels (confirm/cancel) |
| `request_id` | string | No | Optional request ID for correlation |
| `timeout_secs` | uint32 | No | Timeout in seconds (default 600) |
| `urgency` | Urgency | No | Hint affecting icon and sound |

**FieldSpec (discriminated union by `kind`):**

| Kind | Fields | Description |
|------|--------|-------------|
| `text` | label, default?, placeholder?, max_length?, pattern?, secret? | Single-line text input |
| `long_text` | label, default?, max_length? | Long-form multi-line text |
| `integer` | label, default?, min?, max? | Integer in [min, max] |
| `choice` | label, options[], default_index? | Choice from fixed list (radio buttons) |
| `boolean` | label, default? | Boolean yes/no (2 buttons) |
| `date_time` | label, default?, picker_kind? | Date/time picker (date, time, or datetime) |

**ChoiceOption:** `{ label: string, value: string, description?: string }`

**NotesSpec:** `{ label: string, default?: string, max_length?: uint32, required?: boolean }`

**ButtonSpec:** `{ confirm: string, cancel: string, default_is_cancel?: boolean }`

**Urgency enum:** `info` | `warning` | `error` | `secret`

**Return type:** `ElicitResponse` (structured JSON from the OS popup)

### Origin

- **Not found on GitHub or npm** - likely a private/custom MCP server
- The tool renders a **native OS dialog** (macOS NSAlert/NSPanel or equivalent) rather than a TUI-rendered widget
- The name "phinbox" suggests it may be a messaging/inbox-related tool that also provides elicitation
- Two variants exist (`phinbox` and `phinbox_mcp`) suggesting either aliased config or two different server registrations

---

## 2. Current Integration Points in Jcode

### Phinbox References in Codebase

**Zero references found** in jcode source code:
- No mentions of `phinbox` or `elicitate` in any `.rs`, `.toml`, `.json`, `.yaml`, or `.md` file (except this research doc)
- The tool is purely an external MCP server, registered via the standard MCP configuration
- Tools appear as `mcp__phinbox__elicitate_mcp` via jcode's MCP proxy system (`dispatch_name()` in `crates/jcode-base/src/mcp/tool.rs`)

### Existing User-Input Patterns in Jcode

Jcode already has several patterns for tool-initiated user interaction:

#### Pattern 1: `StdinInputRequest` (bash command stdin)

**Location:** `crates/jcode-tool-core/src/lib.rs`

```rust
pub struct StdinInputRequest {
    pub request_id: String,
    pub prompt: String,
    pub is_password: bool,
    pub response_tx: tokio::sync::oneshot::Sender<String>,
}
```

**Flow:** Tool sends request -> forwarded via `stdin_request_tx` channel -> TUI renders prompt -> response sent back via oneshot channel.

**Used by:** The `bash` tool when a running command requests stdin input (password prompts, etc.)

**Wiring:** `client_lifecycle.rs:754-759` sets up forwarding from agent -> TUI.

#### Pattern 2: `RequestPermissionTool` (ambient sessions)

**Location:** `crates/jcode-app-core/src/tool/ambient.rs:383`

**Purpose:** Ambient sessions request approval before code changes. Sends structured permission requests with action, description, rationale, urgency, and context.

**Flow:** Tool call -> permission request dispatched via notifications system -> approval/reject -> continue or abort.

#### Pattern 3: MCP Tool Proxy

**Location:** `crates/jcode-base/src/mcp/tool.rs`

```rust
pub struct McpTool {
    server_name: String,
    tool_def: McpToolDef,
    manager: Arc<RwLock<McpManager>>,
}
```

**Flow:** External MCP tool call -> `McpManager::call_tool()` -> JSON-RPC over stdio -> response converted to `ToolOutput`.

**Key detail:** The MCP proxy currently returns only `ContentBlock` types (Text, Image, Resource). There is **no handling for `InputRequiredResult`** in the protocol layer.

### MCP Protocol Implementation

**Location:** `crates/jcode-base/src/mcp/protocol.rs`

**Current capabilities:**
- JSON-RPC 2.0 request/response/notification
- `InitializeParams` / `InitializeResult` with `ServerCapabilities`
- `McpToolDef`, `ToolCallParams`, `ToolCallResult`, `ContentBlock`
- `McpServerConfig` with stdio transport support

**Missing (compared to MCP spec 2026-07-28):**
- `InputRequiredResult` / `resultType: "input_required"` flow
- `elicitation/create` protocol message
- `inputResponses` in retry requests
- Client capability declaration for elicitation (`io.modelcontextprotocol/clientCapabilities`)
- `requestState` for multi-round-trip requests

---

## 3. MCP Elicitation Protocol (Spec 2026-07-28)

### Overview

Elicitation is a **client feature** in MCP - servers request additional information from users through the client during interactions. Two modes:

1. **Form mode**: Structured data collection with JSON Schema validation (in-band, data visible to client)
2. **URL mode**: Direct users to external URLs for sensitive interactions (out-of-band, data NOT exposed to client)

### Protocol Flow

```
Server                          Client (jcode)                   User
  |                                |                               |
  |--- tools/call request -------->|                               |
  |                                |--- render UI ---------------->|
  |<-- InputRequiredResult --------|                               |
  |    (inputRequests,             |<-- user responds -------------|
  |     requestState)              |                               |
  |                                |--- tools/call (retry) ------->|
  |                                |    with inputResponses         |
  |                                |    + requestState              |
  |<-- ToolCallResult -------------|                               |
```

### Form Mode Schema (restricted JSON Schema subset)

Only flat objects with primitive properties:
- `string` (with formats: email, uri, date, date-time)
- `number` / `integer`
- `boolean`
- `enum` (single-select)
- `array` with enum items (multi-select)

**No nested objects, no arrays of objects** - intentionally limited for client UX.

### Client Capability Declaration

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

### Security Constraints

- Servers MUST NOT use form mode for passwords, API keys, access tokens, or payment credentials
- Servers MUST use URL mode for sensitive information
- Clients MUST provide clear decline/cancel options
- Clients MUST let users review/modify responses before sending

### Tools/Calls InputRequired Flow

This is the key mechanism - it allows a tool call to pause mid-execution and request user input:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "resultType": "input_required",
    "inputRequests": {
      "github_login": {
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
    },
    "requestState": "eyJsb2NhdGlvbiI6Ik5ldyBZb3JrIn0..."
  }
}
```

Client retries with:

```json
{
  "method": "tools/call",
  "params": {
    "name": "get_weather",
    "arguments": { "location": "New York" },
    "inputResponses": {
      "github_login": {
        "action": "accept",
        "content": { "name": "octocat" }
      }
    },
    "requestState": "eyJsb2NhdGlvbiI6Ik5ldyBZb3JrIn0..."
  }
}
```

### createMessage vs Elicitation

These are two distinct MCP mechanisms:

| Aspect | `createMessage` | Elicitation |
|--------|-----------------|-------------|
| Direction | Server -> Client | Server -> Client |
| Purpose | Server requests LLM to generate a message | Server requests user input |
| Data source | LLM generates content | User provides content |
| Privacy | Content visible to client | Form data visible, URL data not |
| Use case | Content generation, summarization | Login, configuration, confirmation |

Elicitation is the correct mechanism for structured user input. `createMessage` is for LLM-generated content.

---

## 4. Native Plugin Opportunity

### Current State

Jcode's MCP infrastructure treats all MCP tools as external proxies. The `phinbox` elicitation tool is:
- Spawned as a separate process
- Communicates over stdio JSON-RPC
- Tool call -> proxy -> external process -> OS popup -> response -> proxy -> tool result

This adds latency and complexity for what could be a native capability.

### Architecture Analysis

#### How Jcode MCP Tools Work

```
Tool call
  -> Registry dispatches to McpTool.execute()
  -> McpManager.call_tool(server, tool, args)
  -> McpClient sends JSON-RPC over stdio
  -> External process handles
  -> Response converted to ToolOutput
```

#### How Native Tools Work

```
Tool call
  -> Registry dispatches to native Tool impl
  -> Tool.execute(input, ctx)
  -> Direct function call (no IPC)
  -> ToolOutput returned
```

### Proposed: Native Elicitation Tool

Could be implemented as a native `Tool` in `crates/jcode-app-core/src/tool/`:

```rust
// crates/jcode-app-core/src/tool/elicitation.rs

pub struct ElicitationTool {
    // For macOS: use NSAlert/NSPanel via objc2
    // For Linux: use zenity or native dialog
    // For Windows: use native Win32 dialogs
}

#[async_trait]
impl Tool for ElicitationTool {
    fn name(&self) -> &str { "elicit" }
    
    fn description(&self) -> &str {
        "Render a native OS popup for structured user input..."
    }
    
    fn parameters_schema(&self) -> Value {
        // Map from MCP elicitation/create schema
        // to jcode's native tool schema format
    }
    
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        // 1. Parse FieldSpec from input
        // 2. Use platform-native dialog:
        //    - macOS: objc2 NSAlert/NSPanel
        //    - Linux: zenity/kdialog
        //    - Windows: Win32 DialogBox
        // 3. Return ElicitResponse
    }
}
```

### Architecture Options

#### Option A: Pure Native (Recommended)

**Pros:**
- Zero latency (no IPC)
- No external process dependency
- Full control over UX
- Works offline
- Integrates with jcode's existing permission/notification system

**Cons:**
- Platform-specific code (need conditional compilation)
- Doesn't match MCP spec (if other agents need to consume it)
- Separate implementation from the MCP version

**Implementation:**
- `crates/jcode-app-core/src/tool/elicitation.rs` (new tool)
- `crates/jcode-platform/` (new crate for OS-specific dialogs)
- macOS: `objc2` bindings to AppKit
- Linux: subprocess calls to `zenity`/`kdialog`
- Windows: Win32 API via `windows` crate

#### Option B: MCP-Native Hybrid

**Pros:**
- Same tool works as both native and MCP
- Other agents can use it via MCP
- Single implementation

**Cons:**
- More complex architecture
- Still requires external process for MCP mode
- The MCP spec elicitation flow adds complexity

**Implementation:**
- Native `ElicitationTool` that also implements MCP server handler
- Can serve elicitation both ways

#### Option C: Enhance MCP Client to Support Elicitation

**Pros:**
- Follows MCP spec closely
- Any MCP server can use elicitation
- jcode becomes a proper MCP elicitation client

**Cons:**
- Only works with MCP servers, not native tools
- Doesn't help with the phinbox dependency
- Most complex to implement (protocol changes)

**Implementation:**
- Add `InputRequiredResult` handling to `McpManager`
- Add elicitation capability declaration
- TUI renders elicitation forms inline
- Retry flow with `inputResponses`

### Recommendation

**Start with Option A (Pure Native)** for these reasons:

1. **Immediate value**: No external dependency for the most common case
2. **Follows existing patterns**: Same as `RequestPermissionTool` and `StdinInputRequest`
3. **TUI integration**: jcode's overlay system already supports session picker, login picker, account picker, changelog overlay - an elicitation overlay would fit naturally
4. **Platform story**: jcode already has `#[cfg(target_os = "macos")]` conditional compilation (see `computer/mod.rs`)
5. **Incremental**: Can add MCP elicitation support later as a separate enhancement

### What the TUI Architecture Would Look Like

```
User types prompt -> Agent generates tool_use: elicit({...})
  -> ToolRegistry dispatches to ElicitationTool
  -> ElicitationTool.execute():
      1. Parse field schema (text, choice, boolean, etc.)
      2. Send ElicitRequest to TUI via channel (similar to StdinInputRequest)
      3. TUI renders:
         - Option A: Inline overlay in chat (like session picker)
         - Option B: Native OS dialog (like phinbox does now)
         - Option C: Both, with fallback
      4. User interacts
      5. Response sent back via oneshot channel
      6. Tool returns ElicitResponse as ToolOutput
  -> Agent receives result and continues
```

The TUI already has:
- **Overlay system**: `crates/jcode-tui/src/tui/ui_overlays.rs` - draws changelog, help, model status, session picker overlays
- **Picker widgets**: `crates/jcode-tui-session-picker/` - full-featured picker component
- **Input channel**: `StdinInputRequest` pattern with oneshot response channel
- **State management**: `crates/jcode-tui/src/tui/app/tui_state.rs` - manages overlay visibility

---

## 5. Other Agents' Patterns

### Claude Code

**Interactive prompts approach:**
- Claude Code uses MCP elicitation natively as a client feature
- MCP servers can send `elicitation/create` requests and Claude Code renders them in the terminal
- No native OS dialogs - everything is terminal-based
- Claude Code also supports `createMessage` for server-initiated LLM calls
- The TUI is the primary interaction surface

**Key insight:** Claude Code keeps everything in the terminal. No native OS popups.

### Codex CLI (OpenAI)

**Interactive prompts approach:**
- Codex CLI is a local coding agent that runs in terminal
- User input is primarily through the chat interface
- No MCP elicitation support (Codex uses its own tool protocol)
- Permission prompts are inline in the terminal
- The `codex app` variant provides a desktop app experience

**Key insight:** Codex doesn't have a formal elicitation protocol. It relies on the conversational interface.

### Cursor

**Interactive prompts approach:**
- Cursor uses its own agent framework
- Permission prompts for file edits, terminal commands, etc. appear as inline UI elements
- No native OS dialogs for agent interaction
- Uses its own tool calling protocol, not MCP elicitation
- Background agents can request user input through the chat

**Key insight:** Cursor keeps everything in the IDE UI.

### OpenCode

**Interactive prompts approach:**
- OpenCode (the project jcode is forked from) uses TUI-native prompts
- Tool permission requests are inline
- No MCP elicitation support
- Uses terminal UI for all interaction

### Comparison Matrix

| Agent | MCP Elicitation | Native OS Dialog | TUI/Inline Prompt | Protocol |
|-------|----------------|------------------|-------------------|----------|
| Jcode (current) | Via external MCP (phinbox) | Via external MCP | StdinInput, RequestPermission | Custom JSON-RPC |
| Claude Code | Yes (client feature) | No | Terminal inline | MCP spec |
| Codex CLI | No | No | Terminal inline | Custom |
| Cursor | No | No | IDE UI | Custom |
| OpenCode | No | No | TUI inline | Custom |

---

## 6. Key Findings Summary

### Critical Facts

1. **Phinbox is an external MCP server** - zero references in jcode source code
2. **Jcode's MCP protocol is incomplete** - no `InputRequiredResult` or elicitation support
3. **Jcode already has user-input patterns** - `StdinInputRequest` and `RequestPermissionTool` prove the architecture works
4. **The MCP elicitation spec (2026-07-28)** defines both form mode and URL mode with structured schemas
5. **No other coding agent uses native OS dialogs** for elicitation - they all stay in-terminal
6. **The `Tool` trait is simple** - just `name()`, `description()`, `parameters_schema()`, `execute()`
7. **jcode's TUI overlay system** is mature and could host an elicitation UI

### Architectural Gaps to Fill

1. **No elicitation tool exists in jcode** - needs new `ElicitationTool` in `crates/jcode-app-core/src/tool/`
2. **No platform dialog abstraction** - needs `crates/jcode-platform/` or similar for OS-specific dialogs
3. **MCP protocol types are incomplete** - need `InputRequiredResult`, `inputResponses`, client capabilities
4. **TUI overlay for elicitation** - optional enhancement for inline form rendering

### Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| Platform dialog code is complex | Medium | Start with macOS, add Linux/Windows later |
| MCP spec changes | Low | Elicitation spec is stable since 2025-03-26 |
| UX inconsistency (native vs TUI) | Medium | Offer both modes with user preference |
| Breaking existing MCP tool flow | Low | Native tool is independent of MCP proxy |

---

## 7. Next Steps (Recommended)

If proceeding with native elicitation:

1. **Create `crates/jcode-platform/`** - OS dialog abstraction layer
2. **Create `crates/jcode-app-core/src/tool/elicitation.rs`** - native `ElicitationTool`
3. **Add TUI overlay** - `crates/jcode-tui/src/tui/ui/elicitation_overlay.rs`
4. **Wire into `ToolRegistry`** - register alongside other tools
5. **Optionally enhance MCP protocol** - add `InputRequiredResult` support for full MCP elicitation client

If staying with external MCP:

1. **Document phinbox setup** - make it easy for users to configure
2. **Consider MCP elicitation client support** - for broader ecosystem compatibility
3. **Add inline TUI fallback** - when phinbox is not available
