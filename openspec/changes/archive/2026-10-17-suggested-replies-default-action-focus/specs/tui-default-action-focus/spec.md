# Spec: TUI Default Action Focus & Explorer Mode (`tui-default-action-focus`)

## ADDED Requirements

### Requirement: Inline TUI Suggested Replies Panel & Keybindings
The TUI SHALL render the Suggested Replies panel in `chunks[5]` (`ui_inline.rs`) when the session is idle and the composer input is empty, displaying a prominent `Default Action` control above a vertical stack of rounded secondary suggestion capsules (`1..=5`, one row per item).

#### Scenario: Vertical layout and Tab, Arrow, Number, Enter, Ctrl+Enter, and mouse activation
- **WHEN** Suggested Replies are visible on an empty composer input
- **THEN** secondary suggestions (`1..=5`) are stacked vertically one per row below `Default Action`, pressing `[Tab]` or `↑`/`↓` cycles focus across `Default Action` and secondary suggestions `1..=5`, pressing `[Enter]` submits the currently focused item, pressing `[Ctrl+Enter]` (or `0` when `Default Action` is present) immediately submits `Default Action`, pressing keys `1`–`5` immediately selects and submits the corresponding secondary suggestion, and clicking any capsule submits it in both local and remote TUI modes.

### Requirement: Cancelable 500ms Full-Auto Explorer Mode
When `enable_full_auto_explorer` is enabled and a `Default Action` is present (`confidence >= confidence_threshold`) with no queued follow-ups or active review splits, the TUI SHALL start a 500ms visual countdown timer (`REDRAW_IDLE`) and automatically submit the `Default Action` when the timer expires.

#### Scenario: Immediate cancellation on user interaction
- **WHEN** the 500ms Explorer countdown is active and the user presses `[Esc]`, types into the composer, pastes text, or triggers voice input
- **THEN** the Explorer countdown is immediately canceled and no automatic submission occurs.

