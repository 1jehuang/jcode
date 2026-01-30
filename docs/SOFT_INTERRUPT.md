# Soft Interrupt: Seamless Message Injection

## Overview

Soft interrupt allows users to inject messages into an ongoing AI conversation without cancelling the current generation. Instead of the disruptive cancel-and-restart flow, messages are queued and naturally incorporated at safe points where the model provider connection is idle.

## Current Behavior (Hard Interrupt)

```
User types message during AI processing
         │
         ▼
    ToolDone event
         │
         ▼
    remote.cancel()  ← Cancels current generation
         │
         ▼
    Wait for Done event
         │
         ▼
    Send user message as new request
         │
         ▼
    AI restarts fresh
```

**Problems:**
- Loses any partial work the AI was doing
- Delay while cancellation completes
- Full context re-send on new API call
- Jarring user experience

## New Behavior (Soft Interrupt)

```
User types message during AI processing
         │
         ▼
    Message stored in soft_interrupt queue
         │
         ▼
    AI continues processing...
         │
         ▼
    Safe injection point reached
         │
         ▼
    Message appended to conversation history
         │
         ▼
    AI naturally sees it on next loop iteration
```

**Benefits:**
- No cancellation, no lost work
- No delay
- AI naturally incorporates user input
- Smooth user experience

## Safe Injection Points

The key constraint is: **we can only inject when not actively streaming from the model provider**. The agent loop has several natural pause points:

### Agent Loop Structure (src/agent.rs)

```rust
loop {
    // 1. Build messages and call provider.stream()
    // === PROVIDER OWNS THE CONNECTION HERE ===
    // Stream events: TextDelta, ToolStart, ToolInput, ToolUseEnd

    // 2. Stream ends

    // 3. Add assistant message to history
    // (MUST happen before injection to preserve cache and conversation order)

    // 4. Check if tool calls exist
    if tool_calls.is_empty() {
        // ═══════════════════════════════════════════════
        // ✅ INJECTION POINT B: No tools, turn complete
        // ═══════════════════════════════════════════════
        break;
    }

    // 5. Execute tools and add tool_results
    for tc in tool_calls {
        // Execute single tool...
        // Add result to history...

        // ═══════════════════════════════════════════════
        // ✅ INJECTION POINT C: Between tool executions
        // (only for urgent aborts - must add skipped tool_results first)
        // ═══════════════════════════════════════════════
    }

    // ═══════════════════════════════════════════════
    // ✅ INJECTION POINT D: All tools done, before next API call
    // ═══════════════════════════════════════════════

    // Loop continues → next provider.stream() call
}
```

### Critical API Constraint

**The Anthropic API requires that every `tool_use` block must be immediately followed by
its corresponding `tool_result` block.** No user text messages can be injected between
a `tool_use` and its `tool_result`.

This means we CANNOT inject messages:
- After the assistant message with tool_use blocks
- Before all tool_results have been added

### Injection Point Details

| Point | Location | Timing | Use Case |
|-------|----------|--------|----------|
| **B** | Turn complete | No tools requested | Safe: no tool_use blocks to pair |
| **C** | Inside tool loop | Urgent abort only | Must add stub tool_results first |
| **D** | After all tools | Before next API call | **Default**: safest point for injection |

**Important**: We do NOT inject between tools for non-urgent interrupts. Doing so would
place user text between tool_results, which could violate API constraints. All non-urgent
injection is deferred to Point D.

### Point B: Turn Complete (No Tools)

```
Timeline:
  Provider: TextDelta... [stream ends, no tool calls]
  Agent: ──► INJECT HERE ◄──
  Agent: Would exit loop, but instead continues with user message

AI sees: "I finished my response, user has follow-up"
```

**Best for:** Quick follow-ups when AI is just responding with text.

### Point C: Between Tools

```
Timeline:
  Agent: Execute tool 1 → result 1
  Agent: ──► INJECT HERE ◄──
  Agent: Execute tool 2 → result 2 (or skip if user said "stop")
  Agent: Next API call

AI sees: "Tool 1 result, user interjection, tool 2 result (or skip message)"
```

**Best for:**
- Urgent abort: "wait, don't do the other tools"
- Mid-execution guidance: "for the next file, also check X"

### Point D: After All Tools

```
Timeline:
  Agent: Execute all tools → all results collected
  Agent: ──► INJECT HERE ◄──
  Agent: Next API call includes: [all tool results] + [user message]

AI sees: "All my tools completed, and user added context"
```

**Best for:** Default behavior. Cleanest, most predictable.

## Implementation

### Protocol Changes

Add new request type for soft interrupt:

```rust
// src/protocol.rs
#[serde(rename = "soft_interrupt")]
SoftInterrupt {
    id: u64,
    content: String,
    /// If true, can abort remaining tools at point C
    urgent: bool,
}
```

### Agent Changes

Add soft interrupt queue and check at each injection point:

```rust
// src/agent.rs
pub struct Agent {
    // ... existing fields
    soft_interrupt_queue: Vec<SoftInterruptMessage>,
}

struct SoftInterruptMessage {
    content: String,
    urgent: bool,
}

impl Agent {
    /// Check and inject any pending soft interrupt messages
    fn inject_soft_interrupts(&mut self) -> Option<String> {
        if self.soft_interrupt_queue.is_empty() {
            return None;
        }

        let messages: Vec<String> = self.soft_interrupt_queue
            .drain(..)
            .map(|m| m.content)
            .collect();

        let combined = messages.join("\n\n");

        // Add as user message to conversation
        self.add_message(Role::User, vec![ContentBlock::Text {
            text: combined.clone(),
            cache_control: None,
        }]);
        self.session.save().ok();

        Some(combined)
    }

    /// Check for urgent interrupt that should abort remaining tools
    fn has_urgent_interrupt(&self) -> bool {
        self.soft_interrupt_queue.iter().any(|m| m.urgent)
    }
}
```

### Injection Point Implementation

```rust
// In run_turn_streaming / run_turn_streaming_mpsc

loop {
    // ... stream from provider ...
    // ... add assistant message to history ...

    // NOTE: We CANNOT inject here if there are tool calls!
    // The API requires tool_use → tool_result with no intervening messages.

    if tool_calls.is_empty() {
        // Point B: No tools, turn complete - safe to inject
        if let Some(msg) = self.inject_soft_interrupts() {
            let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                content: msg,
                point: "B".to_string(),
            });
            // Don't break - continue loop to process the injected message
            continue;
        }
        break;
    }

    // ... tool execution loop ...
    for (i, tc) in tool_calls.iter().enumerate() {
        // Check for urgent abort before each tool (except first)
        if i > 0 && self.has_urgent_interrupt() {
            // Point C: Urgent abort - MUST add skipped tool_results first
            for skipped in &tool_calls[i..] {
                self.add_message(Role::User, vec![ContentBlock::ToolResult {
                    tool_use_id: skipped.id.clone(),
                    content: "[Skipped: user interrupted]".to_string(),
                    is_error: Some(true),
                }]);
            }
            // Now safe to inject user message
            if let Some(msg) = self.inject_soft_interrupts() {
                let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                    content: msg,
                    point: "C".to_string(),
                });
            }
            break;
        }

        // ... execute tool and add tool_result ...
    }

    // Point D: After all tools done, safe to inject
    if let Some(msg) = self.inject_soft_interrupts() {
        let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
            content: msg,
            point: "D".to_string(),
        });
    }
}
```

### TUI Changes

Update interleave handling to use soft interrupt:

```rust
// src/tui/app.rs

// Instead of:
//   remote.cancel() → wait → send message

// Do:
//   remote.soft_interrupt(message, urgent)

// The message will be injected at the next safe point
// No cancellation, no waiting
```

### Server Event for Feedback

```rust
// src/protocol.rs
ServerEvent::SoftInterruptInjected {
    content: String,
    point: String,  // "A", "B", "C", or "D"
}
```

This allows the TUI to show feedback like "Message injected after tool X".

## User Experience

### Default Mode (queue_mode = false)

```
User presses Enter during processing:
  → Message queued for soft interrupt
  → Status shows: "⏳ Will inject at next safe point"
  → AI continues working...
  → [ToolDone] → Message injected
  → Status shows: "✓ Message injected"
  → AI naturally incorporates it
```

### Urgent Mode (Shift+Enter or special flag)

```
User presses Shift+Enter during processing:
  → Message queued as urgent soft interrupt
  → Status shows: "⚡ Will inject ASAP (may skip tools)"
  → AI continues current tool...
  → [ToolDone] → Remaining tools skipped, message injected
  → AI sees: tool 1 result + "user interrupted, skipped tools 2-3" + user message
```

## Comparison

| Aspect | Hard Interrupt (current) | Soft Interrupt (new) |
|--------|-------------------------|---------------------|
| Cancels generation | Yes | No |
| Loses partial work | Yes | No |
| Delay | Yes (wait for cancel) | No |
| API calls | Wastes partial call | Efficient |
| User experience | Jarring | Smooth |
| Complexity | Simple | Moderate |

## Edge Cases

1. **Multiple soft interrupts**: Combine into single message with `\n\n` separator
2. **Soft interrupt during text-only response**: Inject at Point B, continue loop
3. **Provider handles tools internally** (Claude CLI): Still works, injection happens in our loop
4. **Urgent interrupt with no tools**: Treated as normal (nothing to skip)
5. **Stream error**: Clear soft interrupt queue, report error normally

## Testing

1. Send message while AI is streaming text (no tools) → should inject at Point B
2. Send message while AI is executing tools → should inject at Point D (after all tools)
3. Send urgent message while multiple tools queued → should skip remaining tools at Point C
4. Send multiple messages rapidly → should combine into one injection
5. Verify no API errors about tool_use/tool_result pairing
