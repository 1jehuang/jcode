# Message Voice Guidelines

How jcode talks to the user in system/status messages (TUI notices, CLI
output, notifications). The goal: speak plainly to the user about what
happened and what we did for them, not narrate internal mechanics.

## Core principles

1. **Lead with what happened, in the user's terms.**
   The user cares about outcomes ("your agent didn't finish its work"),
   not mechanism names ("todo completion gate", "auto-poke armed",
   "queued continuation").

2. **Say what we did for them, past tense.**
   When the harness takes an action automatically, frame it as already
   handled: "We poked it for you." Not "Auto-poking..." (progressive,
   machine-centric) or "Poke scheduled" (config-speak).

3. **Escape hatch last, short.**
   Controls and opt-outs go at the end, after the human sentence:
   ". /poke off to stop." Never lead with the flag or setting name.

4. **No internal jargon in user-facing text.**
   Words to avoid: "gate", "armed", "dispatch", "continuation",
   "followup", "queued dispatch", "state", "flag". Internal names are
   fine in logs, comments, and code, just not in what the user reads.

5. **Plain sentences over labels.**
   Prefer "Your agent stopped with 5 todos unfinished." over
   "Auto-poke: 5 incomplete todos."

6. **Warnings explain the consequence, then what to do.**
   "We stopped poking because it wasn't making progress. Review the
   remaining todos." Not "Gate exhausted after N attempts."

## Formula

> [What happened.] [What we did about it.] [How to change the behavior.]

Each part is optional except the first, but this order holds.

## Examples

| Before | After |
|---|---|
| `👉 Auto-poking: 5 incomplete todos. /poke off to stop.` | `👉 5 incomplete todos. We poked it for you. /poke off to stop.` |
| `Auto-poking: todos complete; sending confidence summary follow-up.` | `Todos are done. Asking the agent for a final confidence check.` |
| `⚠️ Todo completion gate: validation still failing after repeated nudges. Auto-poke stopped; review the remaining todos manually.` | `⚠️ We poked the agent several times but it stopped making progress. Giving up; review the remaining todos yourself.` |

## Non-goals

- Log lines, tracing, and debug output keep precise internal names.
- Error messages meant for developers (panics, internal errors) are out
  of scope.
