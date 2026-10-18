## MODIFIED Requirements

### Requirement: Inline TUI Suggested Replies Panel & Keybindings
The TUI SHALL render the Suggested Replies panel in `chunks[5]` (`ui_inline.rs`) when the session is idle and the composer input is empty or holds an unedited staged suggestion (`staged_input`), displaying a prominent `Default Action` control above a vertical stack of rounded secondary suggestion capsules (`1..=N`, where `N` is up to the configured `max_secondary_suggestions`, defaulting to `3`, one row per item) truncated by Unicode display column width (`truncate_to_display_width`). When `suggested_replies.show_confidence` is `true` (default), the TUI SHALL display the calibrated model confidence percentage (`(XX%)`) on every suggestion row (both `Default Action` and each secondary suggestion row `1..=N`); when `show_confidence` is `false`, confidence percentages SHALL be omitted.

#### Scenario: Vertical layout, configurable item count (default 3), per-row confidence badges, prompt input staging for editing, and Tab, Arrow, Number, Enter, Ctrl+Enter, and mouse activation
- **WHEN** Suggested Replies are visible on an empty or staged composer input
- **THEN** secondary suggestions (`1..=N`, defaulting to `3` or matching the configured `max_secondary_suggestions`) are stacked vertically one per row below `Default Action`, each row displays its model confidence percentage (`(XX%)`) when `show_confidence = true`, pressing `[Tab]` or `↑`/`↓` cycles focus across `Default Action` and all visible secondary suggestions `1..=N` while immediately populating `app.input` with the focused suggestion's text for user editing, pressing keys `1`–`9` (or `0` for `Default Action`) on an empty input or clicking an unfocused capsule focuses and populates `app.input` with that suggestion, editing `app.input` (`Backspace`, `Delete`, `←`/`→`, `Home`/`End`, or typing) detaches suggestion cycling and keeps the text as a normal draft, pressing `[Enter]` (or clicking an already-focused capsule) submits the prompt, and pressing `[Ctrl+Enter]` immediately submits the focused suggestion or `Default Action`.

## ADDED Requirements

### Requirement: Session-Version Suggestion Caching and Interactive Diagnostics (`/suggestions`)
The TUI SHALL cache the latest `SuggestedRepliesResponse` per `(session_id, display_messages_version)` to avoid redundant turn-end re-evaluation on unchanged transcripts (bypassing and invalidating the cache when 6-lens review findings are recorded or when `/suggestions test` / `/suggestions refresh` is invoked), and SHALL provide a `/suggestions` (`/suggested-replies`) slash command supporting `debug`/`status` and `test`/`refresh` subcommands.

#### Scenario: Inspecting and re-evaluating Suggested Replies via `/suggestions`
- **WHEN** the user runs `/suggestions debug` (or `/suggestions`) or `/suggestions test`
- **THEN** `/suggestions debug` displays the active `[suggested_replies]` configuration (including `max_secondary_suggestions`, `show_confidence`, and `custom_prompts` count), OpenGF and Jev/Laya route status, elapsed latency, and scored candidates vs threshold, while `/suggestions test` bypasses the version cache and triggers an immediate fresh cascade evaluation.
