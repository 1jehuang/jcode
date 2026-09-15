# Specification: Elicitation Overlay for Jcode TUI

**Date:** 2026-09-14
**Status:** Draft - awaiting TUI layout audit
**Source:** 01_RESEARCH_ELICIT.md, 03_RESEARCH_PHINBOX_ELICIT.md

---

## 1. Problem

Agents need to ask the user structured questions mid-task (multi-choice, yes/no, text input). Currently jcode relies on phinbox's external OS popup (`mcp__phinbox__elicitate_mcp`) which breaks the TUI experience and adds an external dependency.

## 2. Proposed Solution

A native TUI elicitation overlay that renders inline, following the same pattern as the existing session picker, login picker, and changelog overlays.

## 3. UX Design

### Trigger Flow

```
Agent generates tool_use: elicit({title, field, question, ...})
  -> ToolRegistry dispatches to ElicitationTool
  -> ElicitationTool sends ElicitRequest to TUI via channel
  -> TUI renders overlay (modal, blocks input)
  -> User interacts with overlay
  -> Response sent back via oneshot channel
  -> Tool returns ElicitResponse to agent
```

### Overlay Layout

```
┌─────────────────────────────────────────────────────────┐
│  ┌─ Agent Request ───────────────────────────────────┐  │
│  │  "Which file should I refactor?"                  │  │
│  │                                                   │  │
│  │  ○ src/main.rs        Main application entry     │  │
│  │  ○ src/server.rs      HTTP server setup          │  │
│  │  ● src/tools.rs       Tool registry (current)    │  │
│  │  ○ src/config.rs      Configuration management   │  │
│  │                                                   │  │
│  │  Notes: [optional free-text area_______________]  │  │
│  │                                                   │  │
│  │  [Confirm]  [Cancel]                              │  │
│  └───────────────────────────────────────────────────┘  │
│                                                         │
│  (chat dimmed behind overlay)                           │
└─────────────────────────────────────────────────────────┘
```

### Field Types

| Kind | TUI Widget | Keyboard |
|------|-----------|----------|
| `text` | Single-line input field | Type to enter, Tab to confirm |
| `long_text` | Multi-line text area | Enter for newline, Ctrl+Enter to submit |
| `integer` | Number input with +/- | Arrow keys to increment |
| `choice` | Radio list (arrow navigable) | Up/Down to select, Enter to confirm |
| `boolean` | Two-button prompt | Y/N keys, Tab to switch |
| `date_time` | Date/time picker | Arrow keys for fields |

### Keyboard Controls

| Key | Action |
|-----|--------|
| Arrow Up/Down | Navigate options (choice type) |
| Enter | Confirm selection |
| Tab | Switch between fields and buttons |
| Escape | Cancel |
| Y/N | Quick yes/no for boolean type |
| Ctrl+Enter | Submit multi-line text |

### Urgency Styling

| Urgency | Border Color | Icon |
|---------|-------------|------|
| `info` | Blue | ● |
| `warning` | Yellow | ◆ |
| `error` | Red | ✗ |
| `secret` | Gray (masked input) | ○ |

## 4. Data Model

```rust
/// Request from agent to display elicitation overlay
#[derive(Debug, Clone)]
pub struct ElicitRequest {
    pub title: String,
    pub field: FieldSpec,
    pub intent: String,
    pub question: Option<String>,
    pub notes: Option<NotesSpec>,
    pub buttons: Option<ButtonSpec>,
    pub request_id: Option<String>,
    pub timeout_secs: u32,
    pub urgency: Urgency,
}

/// Discriminated union for input field types
#[derive(Debug, Clone)]
pub enum FieldSpec {
    Text { label: String, default: Option<String>, placeholder: Option<String>, max_length: Option<u32>, secret: bool },
    LongText { label: String, default: Option<String>, max_length: Option<u32> },
    Integer { label: String, default: Option<i64>, min: Option<i64>, max: Option<i64> },
    Choice { label: String, options: Vec<ChoiceOption>, default_index: Option<usize> },
    Boolean { label: String, default: Option<bool> },
    DateTime { label: String, default: Option<String>, picker_kind: DateTimeKind },
}

#[derive(Debug, Clone)]
pub struct ChoiceOption {
    pub label: String,
    pub value: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Urgency { Info, Warning, Error, Secret }

/// Response from user interaction
#[derive(Debug, Clone)]
pub struct ElicitResponse {
    pub action: ElicitAction,
    pub value: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ElicitAction {
    Confirm,
    Cancel,
}
```

## 5. Architecture Integration

### Channel Pattern (matches existing StdinInputRequest)

```rust
// In ElicitationTool::execute():
let (tx, rx) = oneshot::channel();
self.tui_tx.send(TuiEvent::ElicitRequest(ElicitRequest { ... }, tx))?;
let response = rx.await??;
// Convert to ToolOutput
```

### TUI State

```rust
// In TuiState:
pub elicit_overlay: Option<ElicitOverlayState>,

pub struct ElicitOverlayState {
    pub request: ElicitRequest,
    pub cursor: usize,        // Currently focused field
    pub selected: usize,      // For choice type
    pub text_buffer: String,  // For text/long_text type
    pub response_tx: oneshot::Sender<ElicitResponse>,
}
```

### Overlay Rendering

```rust
// In ui_overlays.rs - add to overlay rendering chain
if let Some(elicit) = &state.elicit_overlay {
    draw_elicit_overlay(f, area, elicit, theme);
}
```

## 6. File Changes Required

| File | Change | Est. Lines |
|------|--------|-----------|
| `crates/jcode-tui/src/tui/ui_overlays.rs` | Add `draw_elicit_overlay()` | +120 |
| `crates/jcode-tui/src/tui/app/tui_state.rs` | Add `ElicitOverlayState` | +40 |
| `crates/jcode-tui/src/tui/app/tui_event.rs` | Add `TuiEvent::ElicitRequest` | +10 |
| `crates/jcode-tui/src/tui/app/key_dispatch.rs` | Add elicit key handling | +60 |
| `crates/jcode-tui/src/tui/app/mod.rs` | Handle ElicitRequest event | +30 |
| `crates/jcode-app-core/src/tool/elicitation.rs` | New ElicitationTool | +150 |
| `crates/jcode-app-core/src/tool/mod.rs` | Register elicitation tool | +5 |
| **Total** | | **~415 lines** |

## 7. Phases

### Phase 1: Data Model + Channel (10m)
- Define `ElicitRequest`, `FieldSpec`, `ElicitResponse` in a new types module
- Add `TuiEvent::ElicitRequest` variant
- Add `ElicitOverlayState` to `TuiState`
- Wire channel: tool -> TUI -> oneshot response

### Phase 2: Overlay Rendering (10m)
- Implement `draw_elicit_overlay()` in `ui_overlays.rs`
- Support `choice` type (radio list) first - most common
- Support `boolean` type (two buttons)
- Urgency-based border coloring

### Phase 3: Key Handling (10m)
- Arrow up/down for choice navigation
- Enter to confirm, Escape to cancel
- Y/N shortcuts for boolean
- Tab between fields and buttons

### Phase 4: Tool Registration (10m)
- Create `ElicitationTool` implementing the tool trait
- Register in tool registry
- Handle `tool_use` -> `ElicitRequest` conversion
- Return `ElicitResponse` as `ToolOutput`

### Phase 5: Integration Test (10m)
- Manual test with a real agent elicitation
- Verify all field types work
- Verify timeout behavior
- Verify cancellation flow

## 8. Open Questions

- [ ] Should the overlay be dismissable by clicking outside? (Probably not - agent expects a response)
- [ ] Should there be a global keybinding to cancel any pending elicitation? (Ctrl+C already exists)
- [ ] How to handle agent timeout? (Let the oneshot drop, agent gets timeout error)
- [ ] Should the overlay support file picker fields? (Future enhancement, not MVP)
