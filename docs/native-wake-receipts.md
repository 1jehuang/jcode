# Native wake receipts

The native socket accepts `notify_session_receipt` with `id`, `receipt_id`,
`session_id`, and `message`. The daemon must have
`JCODE_NATIVE_WAKE_SESSION_ALLOWLIST` set to a comma-separated list of exact
session IDs. The target must already exist in the daemon's profile.

The response is `wake_receipt` with the request `id`, `receipt_id`,
`session_id`, and `status`. `queued` means the request is durably accepted in
`native-wake-receipts.json`; it does **not** mean the session ran. `awakened`
means the exact message was saved in the target transcript at the start of a
receipt-specific live turn and the transcript snapshot or journal was synced
before the receipt advanced. A system-displayed wake input forces the first
snapshot even if no earlier visible conversation exists. A busy, unattached,
or unavailable target remains `queued` at submit time. The caller may retry the
same request or poll `get_wake_receipt`; a poll attempts to start the queued
turn once the target is attached and idle, so polling can advance a receipt
that submit could not start. Only retrying or polling can make progress: no
stored receipt advances on its own.
Retries with the same receipt ID and identical payload return the existing
state; changed payloads fail. Neither the legacy `notify_session` response nor
a generic `done` event is a wake receipt.

The ledger is bounded: it holds at most 256 pending receipts. When it is full,
a new submission is refused with an explicit error instead of evicting an
already-accepted queued wake, and settling an existing wake frees a slot.
Awakened receipts past the ceiling move to a bounded tombstone set (1024
entries) that keeps the receipt ID and target but drops the delivered message,
so a retried ID still resolves to `awakened` and cannot start a second turn.
`get` reads both sets.

This is an at-least-once turn-entry protocol. A process crash after the target
transcript save but before the receipt state save can leave a queued receipt
that replays on retry. A concurrent retry rechecks the receipt after reserving
the idle agent, so a stale queued response cannot start another turn after an
awakened receipt. An interrupted store replacement with a missing primary
recovers the prior `.bak` generation. A corrupt primary also falls back to
`.bak`; if both generations are unusable, the request fails rather than
starting from an empty ledger. The Windows replacement gap remains until a
platform atomic replace is adopted; recovery may lose the most recent store
update if the crash falls between moving the primary and installing the new
file. Filesystem or device failures after a successful sync cannot be ruled
out by the application. It does not claim exactly-once execution, provider
completion, or that the model responded. A bridge must acknowledge only an
exact `awakened` status for its own receipt ID and session ID. A `queued`
status must stay pending.

Validation requires an isolated `JCODE_HOME`, socket, and synthetic session.
No existing user session or shared daemon is needed.
