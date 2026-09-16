# Issue 1: Screen Capture Security

## Problem

When agents use `macos_computer_use` (screenshot/OCR), they capture the user's actual desktop — including personal data (SSNs, banking, medical), private messages, photos, browsing history. This is a severe privacy/security violation.

**Current state:**
- `macos_computer_use` screenshots the full desktop or specific windows
- No isolation between agent view and user's personal content
- Other agents on the same machine can also screenshot the same desktop
- No audit trail of what agents have "seen"

## Threat Model

| Threat | Severity | Likelihood |
|--------|----------|-----------|
| Agent captures SSN/financial data in screenshot | CRITICAL | HIGH |
| Agent captures private messages/DMs | HIGH | HIGH |
| Agent captures passwords/auth tokens on screen | CRITICAL | MEDIUM |
| Agent screenshots leaked via logs/MCP responses | HIGH | LOW |
| Multiple agents screenshot same sensitive content | HIGH | HIGH |

## Existing Solutions

1. **Docker/Podman containers** — Run agents in containers with no display access
2. **VNC/Xvfb virtual displays** — Agents get their own virtual desktop
3. **Apple Container (macOS 26)** — Native sandboxed Linux containers on Apple Silicon
4. **Ephemeral user sessions** — macOS guest accounts for agent work
5. **Browserbase/Sandbox SDK** — Cloud-hosted browser sandboxes
6. **Screen recording permission revocation** — macOS Settings > Privacy > Screen Recording

## Proposed Fixes

### Immediate (P0)
- [ ] Add `--no-screenshot` flag to jcode that disables `macos_computer_use` screenshot action
- [ ] Add environment variable `JCODE_SCREENSHOT_DISABLED=1` to suppress screen capture tools
- [ ] Log all screenshot captures to an audit file for review
- [ ] Add warning in AGENTS.md: "Never screenshot the live desktop"

### Short-term (P1)
- [ ] Implement screen capture sandboxing: agents can only see designated browser windows
- [ ] Add `screenshot_scope` config: `browser_only` | `app_window` | `full_desktop` (default: `browser_only`)
- [ ] Redact/blur regions marked as sensitive before returning to agent
- [ ] Add MCP tool permission model: `screen_capture` requires explicit user approval per session

### Long-term (P2)
- [ ] Virtual display (Xvfb) for agent screen operations on macOS
- [ ] Container-based agent execution with no host display access
- [ ] Screenshot content scanning (detect SSN patterns, credit cards, etc.) with auto-redact
- [ ] Per-agent isolated browser contexts (Browserbase, Playwright)

## Implementation Notes

The `macos_computer_use` tool in jcode has actions: `screenshot`, `ocr`, `ui`, `click`, `type`, etc. The screenshot action currently captures whatever the user's screen shows. We need:

1. A permission gate before screenshot actions
2. Scope restrictions (which windows/regions agents can see)
3. Audit logging of all screen captures
4. Integration with the sandbox/isolation layer
