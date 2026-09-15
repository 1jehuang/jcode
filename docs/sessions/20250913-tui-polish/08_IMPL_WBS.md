# Implementation WBS: Status Bar Extension + Elicit Overlay + DevOps

**Date:** 2026-09-14
**Status:** Ready for dispatch

---

## Epic 1: Extend Existing Status System (swarm stats + turn recap)

### 1.1 Add SwarmStats to TuiState (10m)
- File: `crates/jcode-tui/src/tui/app/tui_state.rs`
- Add `SwarmStats` struct: `active: u32, completed: u32, failed: u32`
- Add `TurnRecapState` struct: `turn_number: u32, elapsed: f32, tokens: u32, tools: u32, expires_at: Instant`
- Add `fn swarm_stats(&self) -> SwarmStats` to TuiState trait
- Implement using `self.remote_swarm_members` status field counts

### 1.2 Extend right_fact_lines with swarm stats (10m)
- File: `crates/jcode-tui/src/tui/ui_input.rs` → `right_fact_lines()` (line 2484)
- Add line 5: `▸ N active ● N done ✗ N failed` using `app.swarm_stats()`
- Color: green active, dim completed, red failed
- Only show when `active > 0 || failed > 0` (don't show when all idle)
- Existing `right_fact_placements` auto-hides on narrow width

### 1.3 Add turn recap to draw_status (10m)
- File: `crates/jcode-tui/src/tui/ui_input.rs` → `draw_status()` (line 754)
- When status transitions Streaming→Idle: capture turn metrics, set TurnRecapState
- Show recap for 5s: `── Turn N ── Xs · Nk tok · N tools · swarm stats`
- After expiry: fall back to normal idle state

### 1.4 Enhance idle bottom bar with swarm summary (10m)
- File: `crates/jcode-tui/src/tui/ui_input.rs` → `draw_status()`
- When idle AND agents active: `▸ N active ● N done ✗ N failed  · project_name`
- Falls back to existing idle behavior when no agents

---

## Epic 2: Elicitation Overlay

### 2.1 Create elicitation types module (10m)
- File: `crates/jcode-tui/src/tui/elicitation_types.rs` (NEW)
- Define `ElicitRequest`, `FieldSpec`, `ChoiceOption`, `ElicitResponse`, `ElicitAction`
- Define `ElicitOverlayState` with cursor, selected, text_buffer, response_tx

### 2.2 Add TuiEvent::ElicitRequest channel (10m)
- File: `crates/jcode-tui/src/tui/app/tui_event.rs`
- Add `TuiEvent::ElicitRequest(ElicitRequest, oneshot::Sender<ElicitResponse>)` variant
- Handle in main event loop: set `state.elicit_overlay`

### 2.3 Implement elicitation overlay rendering (10m)
- File: `crates/jcode-tui/src/tui/ui_overlays.rs`
- Add `draw_elicit_overlay()` - radio list for choice, buttons for boolean
- Border color by urgency (blue/yellow/red/gray)
- Notes area below main field

### 2.4 Add elicitation key handling (10m)
- File: `crates/jcode-tui/src/tui/app/key_dispatch.rs`
- Arrow Up/Down for choice navigation
- Enter to confirm, Escape to cancel
- Y/N shortcuts for boolean

### 2.5 Create ElicitationTool (10m)
- File: `crates/jcode-app-core/src/tool/elicitation.rs` (NEW)
- Implement tool trait, parse args → ElicitRequest → oneshot → ToolOutput

### 2.6 Register elicitation tool (10m)
- File: `crates/jcode-app-core/src/tool/mod.rs`
- Add to tool registry, wire dispatch

---

## Epic 3: DevOps / CI/CD (user requested)

### 3.1 Audit upstream sync status (10m)
- Check if jcode fork is behind upstream 1jehuang/jcode
- Identify commits to cherry-pick or merge
- Document sync delta

### 3.2 Set up GitHub Actions release CI (10m)
- File: `.github/workflows/release.yml` (NEW)
- Trigger on tag push (v*)
- Build matrix: Linux (x86_64, aarch64), macOS (aarch64, x86_64), Windows (x86_64)
- Upload artifacts to GitHub Releases
- Generate install scripts (curl, irm for Windows, bun)

### 3.3 Set up GitHub Actions dev CI (10m)
- File: `.github/workflows/ci.yml` (NEW or existing)
- Trigger on PR + push to master
- Run: cargo check, cargo test, cargo clippy, cargo fmt --check
- Cache dependencies

### 3.4 Create install scripts (10m)
- macOS/Linux: `curl -fsSL https://jcode.sh/install.sh | bash`
- Windows: `irm https://jcode.sh/install.ps1 | iex`
- Bun: `bun install jcode`

---

## Dependency Graph

```
Epic 1 (status bar):  1.1 → 1.2, 1.1 → 1.3, 1.1 → 1.4  (parallel after 1.1)
Epic 2 (elicitation): 2.1 → 2.2 → 2.3, 2.2 → 2.4, 2.5 → 2.6
Epic 3 (devops):      3.1 → 3.2, 3.1 → 3.3, 3.2 → 3.4
```

## Parallel Execution

| Time | Worker A (Status Bar) | Worker B (Elicitation) | Worker C (DevOps) |
|------|----------------------|----------------------|-------------------|
| 0-10m | 1.1 SwarmStats | 2.1 Types module | 3.1 Upstream audit |
| 10-20m | 1.2 right_fact + 1.3 recap | 2.2 Channel + event | 3.2 Release CI |
| 20-30m | 1.4 Idle enhancement | 2.3 Overlay render | 3.3 Dev CI |
| 30-40m | polish + test | 2.4 Key handling | 3.4 Install scripts |
| 40-50m | — | 2.5 ElicitationTool | — |
| 50-60m | — | 2.6 Registration | — |

**Total parallel wall time: ~60 minutes**
