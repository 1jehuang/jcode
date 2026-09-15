# Specification: Compact Status Bar for Jcode TUI (REVISED)

**Date:** 2026-09-14
**Status:** Final - revised per user feedback
**Key change:** EXTEND existing status system, do NOT add new bar
**Source:** User feedback (2026-09-14), 02_RESEARCH_TUI_LAYOUT.md, 04_RESEARCH_SCOPE_TRACKING.md

---

## 1. Problem

The operator manages 16-128 agents across multiple repos. The existing TUI has status info scattered across:
- **Bottom bar** (`draw_status`): Processing state, rate limits, batch progress
- **Right fact stack** (`right_fact_lines`): Provider, model, dir+branch, context usage

**Missing:** Swarm/dispatch stats (active/complete/failed agents), turn-level recap, workspace scope.

## 2. User Requirements (Revised)

- **Extend existing bottom status bar and right fact stack** - no new bar
- Add dispatch stats: `▸ N active ● N complete ✗ N failed`
- Add turn-level recap: full-width summary after each turn completes
- Items like "1/4 active", model info, alt+n controls, todos should integrate into existing right_fact_stack
- **Hide on narrow width** / most views (existing behavior handles this)
- Status bars should hold **turn-level recap style full width prints**

## 3. Design: Three Integration Points

### A. Right Fact Stack Extension (always visible when agents active)

Add line 5 to `right_fact_lines()`:

```
Line 1: openrouter · API key              (existing)
Line 2: mimo-v2.5 low                     (existing)
Line 3: ~/jcode  main                     (existing)
Line 4: 50k/200k ████████░░               (existing)
Line 5: ▸ 3 active ● 12 done ✗ 1 failed   (NEW - swarm stats)
```

Color coding: green for active, dim for completed, red for failed.
Auto-hides on narrow width (existing collision detection).

### B. Bottom Bar Turn Recap (transient, after turn completes)

When a turn completes (ProcessingStatus goes from Streaming → Idle), show a full-width recap:

```
── Turn 5 ── 2.3s · 1.2k tok · 3 tools · ▸ 3 active ● 12 done ✗ 1 failed
```

This replaces the normal idle state temporarily (5 seconds or until next input).
Format: `── Turn N ── {elapsed} · {tokens} · {tool_count} tools · {swarm_stats}`

### C. Bottom Bar Idle State (always visible)

When idle with active agents, show swarm summary in the bottom bar:

```
▸ 3 active ● 12 done ✗ 1 failed  · ~/jcode (main)
```

When idle with no agents, show existing idle behavior (session tip, token count, etc.).

## 4. Data Sources (confirmed from 04_RESEARCH)

| Data | Access | Source |
|------|--------|--------|
| Project name | `self.session.working_dir` → `Path::file_name()` | session.rs:105 |
| Swarm members | `self.remote_swarm_members` | app.rs:1251 |
| Member status | `.status` field ("running"/"completed"/"failed") | lib.rs:441 |
| Streaming tokens | `app.streaming_tokens()` | TuiState trait |
| Processing state | `app.status()` | TuiState trait |
| Turn count | `app.display_user_message_count()` | TuiState trait |

## 5. Implementation Plan

### Phase 1: Add swarm_stats() to TuiState (10m)
- Add `fn swarm_stats(&self) -> SwarmStats` to TuiState trait
- Implement in app/tui_state.rs using `remote_swarm_members`
- Return `SwarmStats { active, completed, failed }`

### Phase 2: Extend right_fact_lines (10m)
- Add line 5 to `right_fact_lines()` in ui_input.rs
- Show only when `active > 0 || failed > 0`
- Color: green active, dim completed, red failed
- Auto-hide: existing `right_fact_placements` handles narrow width

### Phase 3: Add turn recap to draw_status (10m)
- In `draw_status()`, when status transitions to Idle:
  - Capture turn metrics (elapsed, tokens, tools used)
  - Show recap line for 5 seconds
  - Format: `── Turn N ── Xs · Nk tok · N tools · swarm stats`
- Store turn_recap state in TuiState with expiry timestamp

### Phase 4: Enhance idle bottom bar (10m)
- When idle and agents active, show swarm summary
- Format: `▸ N active ● N done ✗ N failed  · project_name`
- Falls back to existing idle behavior when no agents

### Phase 5: Width-awareness + polish (10m)
- Verify right_fact_stack hides correctly on narrow terminals
- Test bottom bar truncation with long repo names
- Test turn recap expiry and transition

## 6. File Changes

| File | Change | Est. Lines |
|------|--------|-----------|
| `ui_input.rs` `right_fact_lines()` | Add swarm stats line 5 | +25 |
| `ui_input.rs` `draw_status()` | Add turn recap + idle swarm summary | +50 |
| `tui_state.rs` | Add `swarm_stats()`, `turn_recap_state` | +30 |
| `app/tui_state.rs` | Implement swarm_stats, track turn transitions | +40 |
| **Total** | | **~145 lines** |

## 7. Comparison: Before vs After

### Before (right side)
```
openrouter · API key
mimo-v2.5 low
~/jcode  main
50k/200k ████░░░░
```

### After (right side, agents active)
```
openrouter · API key
mimo-v2.5 low
~/jcode  main
50k/200k ████░░░░
▸ 3 active ● 12 done ✗ 1 failed
```

### Before (bottom, idle)
```
Idle (session: 12k in, 8k out)
```

### After (bottom, idle with agents)
```
▸ 3 active ● 12 done ✗ 1 failed  · jcode
```

### After (bottom, turn recap)
```
── Turn 5 ── 2.3s · 1.2k tok · 3 tools · ▸ 3 active ● 12 done ✗ 1 failed
```
