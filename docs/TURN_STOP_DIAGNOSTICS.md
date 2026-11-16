# Investigating unexpected AI stops

Logs are stored under `~/.jcode/logs/` (or the configured Jcode home).

## Start at the turn boundary

`TURN_START` and `TURN_STOP` describe real runtime turns. Match their session,
runtime-generation, and turn IDs. `TURN_STOP` is emitted once per turn, not once
per provider response or tool-use round. Control-command acknowledgements are not
AI turns and do not produce these markers.

A stop includes its request ID when available, an allowlisted `provider_stop`
classification, `reason`, `abnormal`, and `needs_investigation`:

| reason | needs_investigation | Meaning |
| --- | --- | --- |
| `normal` | `false` | Provider reported normal completion |
| `interrupted` | `false` | Explicit interruption, normally user/control initiated |
| `failure` | `true` | Turn returned an error |
| `crash` | `true` | A caught turn panic |
| `limit` | `true` | Output or execution limit stopped the turn |
| `guardrail` | `true` | Provider policy stop; not necessarily a Jcode bug |
| `unknown` | `true` | Missing or unrecognized completion evidence |

A successful `Done` without provider completion evidence is **unknown**, not
assumed normal. An intermediate provider `MessageEnd` is not a completed AI turn.
Retries clear stale completion evidence before determining the final outcome.

With the default text event format, find investigation candidates:

```bash
rg 'event=TURN_STOP .*needs_investigation=true' ~/.jcode/logs/
```

Keep the normal records available. `normal` means the provider ended normally,
not proof that the user's task was finished. If an apparently unfinished task has
only normal stops, inspect that session's surrounding events and conversation.

A hard process kill, OS crash, or power loss cannot write a final log. A
`TURN_START` with no matching `TURN_STOP` is another investigation signal, though
an active turn or rotated/missing log files can explain it too.

## Follow the recovery chain

Search a session or request ID around a candidate stop, then inspect
`RECOVERY_*` events. They cover:

- Send submission and write/preparation failures.
- Retry scheduling, network waits, elapsed deadlines, and bounded retry exhaustion.
- Due retries held by history loading, a running turn, an unconfirmed model
  switch, or an outstanding interrupt acknowledgement. Repeated blocked-state
  logs are rate-limited, not emitted on every TUI tick.
- Failed fallback route confirmation and failed fallback resubmission.
- Terminal auto-resume pauses and credential circuit-breaker stops.
- Draft/image restoration and text-only queue guards.
- Interleave/soft-interrupt reconciliation and queue recovery.
- Reload context recovery and duplicate startup/continuation suppression.

`RECOVERY_TURN_FOLLOWUP_DECISION` records whether the client scheduled another
turn, whether auto-resume is enabled, and whether a final response was observed.
Use it when a provider reports a normal stop but the work appears unfinished.
These are diagnostic signals, not a claim that the user's intent was satisfied.

`RECOVERY_TURN_SETTLED` is a client observation of a matching `Done`, not a second
canonical outcome. In particular, a hidden continuation acknowledged without an
accepted scoped turn emits `RECOVERY_CONTINUATION_NOT_ACCEPTED`. Investigate it
alongside the server's `Suppressing delivered reload recovery replay` message.
A delivery acknowledgement is not proof that AI execution happened.

```bash
rg 'TURN_(START|STOP)|RECOVERY_|Suppressing delivered reload recovery replay' ~/.jcode/logs/
```

Recovery snapshots include IDs, retry attempts/deadlines, queue sizes, pending
images counts, processing state, and model-switch state. `SENT`/`SUBMITTED` means
the transport write succeeded, not that the server accepted or completed a turn.
Metadata intentionally excludes prompt text, hidden reminders, image bytes,
credentials, and raw provider error bodies. New provider stop strings are
classified as `unknown` rather than copied into the diagnostic record.

The structured logging facility can also emit `EVENT_JSON` records. In that
mode, filter the `event` and `needs_investigation` fields instead of the default
`key=value` syntax above.
