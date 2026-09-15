# 02 - Research: TUI Layout Audit for Status Bar

**Date:** 2026-09-14
**Source:** Direct audit of `crates/jcode-tui/src/tui/`

---

## 1. Existing Status System (DO NOT ADD NEW BAR)

The TUI already has a comprehensive status system. The task is to **extend** it, not add a new bar.

### A. Bottom Status Bar: `draw_status()` (ui_input.rs:754-1067, 314 lines)

**Location:** Below the chat area, above the input prompt.
**Current content:**
- Processing state: idle/sending/connecting/thinking/streaming/running_tool
- Rate limit countdown with spinner
- Build progress indicator
- Queued message count (`· +N queued`)
- Streaming tokens (↑N ↓N)
- Batch progress (Running batch: N/M done)

**Width behavior:** Fills full width, truncates narrow lines.

### B. Right Fact Stack: `right_fact_lines()` (ui_input.rs:2484-2571)

**Location:** Right side of chat area, stacked vertically.
**Current content (4 lines max):**
```
Line 1: Provider + Auth method        (e.g., "openrouter · API key")
Line 2: Model + Reasoning effort      (e.g., "mimo-v2.5 low")
Line 3: Working dir + Git branch      (e.g., "~/jcode  main")
Line 4: Context usage + bar           (e.g., "50k/200k ████████░░")
```

**Width behavior:** Hidden on narrow terminals (collision detection in `right_fact_placements`).

### C. Overscroll Status: `draw_overscroll_status()` (ui_input.rs:2024-2183)

**Location:** Above the input area, below chat.
**Content:** Provider display, git branch, context usage, auth label.

## 2. Layout Structure (from ui.rs)

```
┌─ Header (1 line) ──────────────────────────────────────────┐
│                                                             │
│  Chat / Viewport (variable height)        [Right Fact Stack]│
│  (scrollable message area)                 [Provider    ]   │
│                                           [Model       ]   │
│                                           [Dir + Branch]   │
│                                           [Context Bar ]   │
│                                                             │
├─ Overscroll Status (1 line, optional) ─────────────────────┤
├─ Status Bar (1 line) ─────────────────────────────────────┤
│ Processing state / rate limit / batch progress              │
├─ Input Area (1+ lines) ───────────────────────────────────┤
│ > _                                                         │
└─────────────────────────────────────────────────────────────┘
```

## 3. Key Data Access Patterns

### For right_fact_lines (right side panel)
```rust
// Provider
let provider = app.provider_name();
// Model
let model = app.provider_model();
// Working dir
let dir = app.working_dir();
// Context usage
let (used, limit) = app.total_session_tokens();
```

### For bottom status bar
```rust
// Processing state
let status = app.status();  // ProcessingStatus enum
// Streaming tokens
let (input, output) = app.streaming_tokens();
// Rate limit
let remaining = app.rate_limit_remaining();
```

### For swarm stats (NEW - need to add)
```rust
// From tui_state.rs / app.rs
let members = &app.remote_swarm_members;  // Vec<SwarmMemberStatus>
let active = members.iter().filter(|m| m.status == "running").count();
let done = members.iter().filter(|m| m.status == "completed").count();
let failed = members.iter().filter(|m| m.status == "failed").count();
```

## 4. Extension Points for Status Bar Feature

### Option A: Extend right_fact_lines (RECOMMENDED)
Add a 5th line to the right fact stack for swarm stats:
```
Line 1: Provider + Auth
Line 2: Model + Reasoning
Line 3: Dir + Branch
Line 4: Context bar
Line 5: ▸ 3 active ● 12 done ✗ 1 failed  (NEW)
```

**Pros:** Uses existing layout, auto-hides on narrow width, no layout chunk changes.
**Cons:** Right side only, not full-width.

### Option B: Extend draw_status (bottom bar)
Add swarm stats to the bottom status bar when idle:
```
▸ 3 active ● 12 done ✗ 1 failed  · ~/jcode
```

**Pros:** Full width, always visible.
**Cons:** Competes with processing state info.

### Option C: Turn-level recap (full width, user preference)
When a turn completes, show a full-width recap line:
```
── Turn 5 Complete ── 2.3s · 1.2k tokens · 3 tools · ▸ 3 active ● 12 done
```

**Pros:** Full width, temporal context, matches user's "turn level recap" requirement.
**Cons:** Needs new rendering function, transient (disappears on next turn).

### Recommendation: Combine A + C
- **Always visible:** Extend right_fact_lines with swarm stats (line 5)
- **On turn complete:** Show full-width recap in draw_status area
- **On narrow width:** Hide right fact stack (existing behavior), show only bottom bar

## 5. Width-Awareness

The existing `right_fact_placements` already handles collision detection and hiding on narrow terminals. The new swarm stats line would inherit this behavior automatically.

For the bottom bar, `truncate_line_for_narrow` already handles truncation.

## 6. Files to Modify

| File | Change | Lines |
|------|--------|-------|
| `ui_input.rs` `right_fact_lines()` | Add swarm stats line 5 | +30 |
| `ui_input.rs` `draw_status()` | Add turn recap when idle | +40 |
| `tui_state.rs` | Add `swarm_stats()` method to trait | +10 |
| `tui_state.rs` impl | Implement `swarm_stats()` | +20 |
| **Total** | | **~100 lines** |
