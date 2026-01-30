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

    // ═══════════════════════════════════════════════
    // ✅ INJECTION POINT A: Stream ended, before tools
    // ═══════════════════════════════════════════════

    // 4. Check if tool calls exist
    if tool_calls.is_empty() {
        // ═══════════════════════════════════════════════
        // ✅ INJECTION POINT B: No tools, turn complete
        // ═══════════════════════════════════════════════
        break;
    }

    // 5. Execute tools
    for tc in tool_calls {
        // Execute single tool...
        // Add result to history...

        // ═══════════════════════════════════════════════
        // ✅ INJECTION POINT C: Between tool executions
        // ═══════════════════════════════════════════════
    }

    // ═══════════════════════════════════════════════
    // ✅ INJECTION POINT D: All tools done, before next API call
    // ═══════════════════════════════════════════════

    // Loop continues → next provider.stream() call
}
```

### Injection Point Details

| Point | Location | Timing | Use Case |
|-------|----------|--------|----------|
| **A** | After stream ends | Before any tool runs | Early injection, AI sees msg + pending tool calls |
| **B** | Turn complete | No tools requested | Inject before agent loop exits |
| **C** | Inside tool loop | Between tools | Urgent: "stop!" can skip remaining tools |
| **D** | After all tools | Before next API call | Cleanest: all results + user msg together |

### Point A: After Stream Ends

```
Timeline:
  Provider: TextDelta... ToolStart... ToolInput... ToolUseEnd... [stream ends]
  Agent: ──► INJECT HERE ◄──
  Agent: Execute tool 1, tool 2, tool 3...
  Agent: Next API call includes: [tool results] + [user message]

AI sees: "I requested these tools, got results, and user said X"
```

**Best for:** General interjections that don't need to affect tool execution.

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

    // Point A: After stream ends, before tools
    if let Some(msg) = self.inject_soft_interrupts() {
        let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
            content: msg,
            point: "A".to_string(),
        });
    }

    // ... add assistant message to history ...

    if tool_calls.is_empty() {
        // Point B: No tools, turn complete
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
            // Point C: Urgent abort, skip remaining tools
            if let Some(msg) = self.inject_soft_interrupts() {
                let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                    content: msg,
                    point: "C".to_string(),
                });
                // Add note about skipped tools
                self.add_message(Role::User, vec![ContentBlock::Text {
                    text: format!("[Skipped {} remaining tool(s) due to user interrupt]",
                                  tool_calls.len() - i),
                    cache_control: None,
                }]);
            }
            break;
        }

        // ... execute tool ...

        // Point C: Between tools (non-urgent)
        if i < tool_calls.len() - 1 {
            if let Some(msg) = self.inject_soft_interrupts() {
                let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                    content: msg,
                    point: "C".to_string(),
                });
            }
        }
    }

    // Point D: After all tools, before next API call
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

1. Send message while AI is streaming text → should inject at Point B or D
2. Send message while AI is executing tools → should inject at Point C or D
3. Send urgent message while multiple tools queued → should skip remaining tools
4. Send multiple messages rapidly → should combine into one injection
5. Verify no provider errors from mid-stream injection (there shouldn't be any)
