# Diagnosing missing mobile synced sessions

Treat transcript upload, live computer presence, and cloud SSH attachment as
three separate paths. A successful cloud connection does not prove that the
laptop uploaded anything. A saved transcript does not prove a live daemon.

## Start with read-only production evidence

The subscription API checkout contains `workers/api/MOBILE_DIAGNOSTICS.md`.
Its account-scoped mobile diagnostics retain structured events for 14 days.
Use the read-only operator endpoint or scoped D1 SELECTs, not transcript dumps.
Resolve the account using the email shown in the phone's account selector.
Never select bearer keys, message text, working directories, or previews for
this diagnosis.

Check, scoped to the account:

- `mobile_sessions`: source, message_count, updated_at, live_at. Zero rows means
  there is no currently stored synced inventory under that account, not proof
  that the computer has never attempted uploading. Check account mismatch,
  disabled sync, authentication, upload errors, and prior deletion separately.
- `mobile_diagnostics`: release build, OS, recent fixed event names. Distinguish
  cloud `session_connect_error` / `connection_error_harness` events from synced
  list, lookup, transcript, and command requests. Older mobile builds only
  printed synced HTTP outcomes to local stderr, so absence of uploaded sync
  events from those builds is not evidence that requests were never made.
- `mobile_commands`: target, status, timestamps. No command rows is not evidence
  of a failed runner: browsing alone never queues a command.

The phone's `Tap here to retry opening session` banner comes from external
session lookup in `src/app/synced_sessions.rs` in the mobile repository. A
`no such session` error can therefore reflect missing account-scoped synced
metadata, rather than a broken SSH connection or an outdated local daemon.

## Computer-side evidence

Ask for these outputs, not credentials or full raw logs:

```sh
jcode --version
jcode mobile status
jcode mobile sync
```

If sync is off, sign in to the same Jcode account and explicitly opt in with
`jcode mobile enable`. Do not enable or backfill transcripts without consent.
`mobile status` reports local persisted configuration, not proof of the version
or configuration held by an already running daemon. Likewise a new CLI binary
alone is not proof the long-lived daemon was updated.

Daemon logs live under `$JCODE_HOME/logs/`, normally `~/.jcode/logs/`.
Locally inspect `mobile_sync` stage records around a reproduction. Share only
those sanitized records and the sync summary, not whole logs which can contain
unrelated private information. Useful outcomes include `skipped_disabled`,
`skipped_empty_or_debug`, `missing_account`, `load_error`, and `partial_failure`.
HTTP upload records distinguish `unauthorized`, `forbidden`, `route_missing`,
`payload_too_large`, `rate_limited`, `server_error`, `timeout`, `connect_error`,
and other `transport_error` / `http_error` outcomes. Older builds may report
only generic `error` / `http_error` labels.

Basic account transcript upload landed November 2, 2026. Safe daemon stage
logging landed November 7. Live messaging into the same computer session
landed November 13 and additionally requires the API migration and compatible
mobile app described in `MOBILE_CLOUD_CONTINUATION.md`. Do not diagnose an empty
transcript list solely by the newer live-messaging requirements.

## Observability limits

Mobile uploaded diagnostics do not remotely expose the laptop's opt-in flag,
account selection, daemon version, or daemon logs. CLI telemetry may have no
email-to-installation linkage, so do not guess a user's installation by timing
or platform. When these facts are missing, obtain the three computer outputs
and sanitized stage records above. Additional instrumentation requires shipping
the changed client and reproducing again, and cannot reconstruct past failures.
