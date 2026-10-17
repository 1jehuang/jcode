# Spec: TUI Default Action Focus & Explorer Mode (`tui-default-action-focus`)

## ADDED Requirements

### Requirement: Inline TUI Suggested Replies Panel & Keybindings
The TUI SHALL render the Suggested Replies panel in `chunks[5]` (`ui_inline.rs`) when the session is idle and the composer input is empty, displaying a prominent `Default Action` control above rounded secondary suggestion capsules (`1..=5`).

#### Scenario: Tab, Enter, Ctrl+Enter, and mouse activation
- **WHEN** Suggested Replies are visible on an empty composer input
- **THEN** pressing `[Tab]` focuses/activates the `Default Action`, pressing `[Ctrl+Enter]` (or `[Enter]` while focused) submits the `Default Action`, and clicking any secondary capsule badge (`1..=5`) inserts and submits that suggestion in both local and remote TUI modes.

### Requirement: Cancelable 500ms Full-Auto Explorer Mode
When `enable_full_auto_explorer` is enabled and a `Default Action` is present (`confidence >= confidence_threshold`) with no queued follow-ups or active review splits, the TUI SHALL start a 500ms visual countdown timer (`REDRAW_IDLE`) and automatically submit the `Default Action` when the timer expires.

#### Scenario: Immediate cancellation on user interaction
- **WHEN** the 500ms Explorer countdown is active and the user presses `[Esc]`, types into the composer, pastes text, or triggers voice input
- **THEN** the Explorer countdown is immediately canceled and no automatic submission occurs.
