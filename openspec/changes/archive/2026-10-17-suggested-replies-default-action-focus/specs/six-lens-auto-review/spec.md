# Spec: 6-Lens Subagent Auto-Self-Review (`six-lens-auto-review`)

## ADDED Requirements

### Requirement: 6-Lens Subagent Self-Review Orchestration
The system SHALL support a 6-lens read-only subagent review workflow (`/review 6`, `/autoreview 6`, and `six_lens_auto_review = true`) covering: (1) Correctness & Edge Cases, (2) Architecture & Module Boundaries, (3) Security & Safety Invariants, (4) TUI/UX & Keybindings, (5) Performance/Async & KV-Cache, and (6) Tests & Go/No-Go Closure.

#### Scenario: Feeding 6-Lens review findings into Suggested Replies Default Action
- **WHEN** the 6-lens review completes and reports actionable findings (`CONDITIONAL GO` or `NO-GO`)
- **THEN** the Suggested Replies state surfaces a high-confidence `Default Action` to address the reported 6-lens review findings.
