# Continue a conversation from your phone

Mobile transcript sync is opt-in: `jcode mobile enable` requires a signed-in
Jcode account with mobile-sync entitlement. `jcode mobile status` reports the
local setting. Background sync uploads visible conversation text, summarized
tool calls, and truncated tool results. It is not a file or credential backup.

## Keep running on your computer

When mobile sync is enabled and the computer's jcode daemon holds an open
session, the phone can send messages to that same session without transferring
it to Jcode Cloud. The daemon reports live sessions every five seconds. The API
marks a session offline when its presence report is more than 45 seconds old.
A saved synced transcript alone is not evidence that a computer is online.

A message starts a normal user turn when the session is idle. During an active
turn it enters the session's user soft-interrupt queue. The existing computer's
provider, working directory, files, credentials, and tools remain in use. The
session's attached desktop windows also receive the resulting activity. Results
reach the phone through ordinary mobile transcript sync, not a direct phone-to-
computer network connection. Neither a cloud VM nor a new session is created.

`jcode mobile messages off` refuses new phone messages while leaving transcript
sync enabled. `jcode mobile messages on` allows them again, and `jcode mobile
status` shows the setting. Disabling sync also stops the listener. Previously
accepted messages are not cancelled by turning off the listener.

If the computer disappears before claiming a message, the queued message fails
after two minutes rather than executing unexpectedly when it returns. A claimed
message is not replayed after a lost response. Command completion means that the
daemon accepted the message, not that the model turn has finished.

The API migration `2026-11-14-mobile-live-local.sql`, compatible desktop daemon,
and compatible mobile app are all required. Until all three are installed,
existing sessions remain available through the separate cloud continuation flow.

## Move a copy to Jcode Cloud

In the mobile app, open a synced local conversation and choose **Continue in
cloud**, then send the next instruction. This queues an explicit `transfer`
command and wakes the account's cloud VM. Merely viewing a transcript does not
transfer it or start execution.

The cloud runner downloads every page of the account-scoped synced transcript,
creates a new cloud session, and records the original session ID as its parent.
The original local conversation is left intact, so local and cloud uploads do
not overwrite one another. The mobile command's `result_session_id` identifies
the new cloud conversation for subsequent messages.

Before processing the next instruction, the agent receives a relocation notice
explaining that it is now on a cloud VM. The notice includes the source host and
working directory as metadata, the new workspace, and these limitations:

- Local files, uncommitted changes, credentials, installed tools, and running
  processes are not transferred.
- The synced transcript may have truncated tool results and excludes images,
  hidden reasoning, and provider-native resume state.
- Earlier paths and tool actions belong to the original machine. The agent
  must inspect the VM and locate the project before continuing work.

Historical tool entries are imported as plain text, not executable tool calls.
Historical system entries are not promoted into the VM's system prompt. A new
cloud session uses the cloud runner's current provider configuration rather
than attempting to reuse provider-specific login or session state.

Transfer resumes from the latest successfully synced conversation, not any
unsynced work still on an offline laptop. Enabling sync backfills sessions
modified in the last 14 days, with at most 200 candidate sessions per pass.
This feature requires compatible API, mobile-app, and cloud-runner versions.

## Implementation validation, November 14, 2026

- Full backend suite: 625 passed, one skipped, zero failures after the local
  acknowledgement timeout fix.
- Hosted `justrust check -p jcode-app-core -p jcode-base --tests` passed.
- Hosted `justrust test -p jcode-base mobile_runner::local --lib` passed all six
  loopback-HTTP runner tests.
- Hosted daemon tests passed all six cases, including idle turns, busy-session
  queueing, transient lock recovery, rejected delivery, and headless presence.
- Hosted `justrust test -p jcode-base mobile_ --lib` passed 63 tests with five
  ignored tests. Full CLI checking and opt-out persistence tests also passed.
- The backend HTTP/SQLite plus Rust runner acceptance test passed using a
  synthetic session host. Its executable was compiled on hosted and executed
  locally for Node and sibling-checkout access. No local battery build was used.
- Mobile hosted workspace checking, 15 core sync tests, and all nine live-chat
  GPUI tests passed. Public TestFlight build 1794621085 contains the tested
  application source, commit `6f91b68`.

These automated results are not physical-device iOS acceptance. Production
backend `77039949-c1ed-4d7e-bbcf-ad6b1f80200a` was activated at 02:37 UTC,
November 14. Real mobile composer-API command
`88598cde-2df1-4dba-a6e3-b9cde7595e8d` reached the same normal local session,
completed as local, and its exact reply appeared in daemon history and the
ordinary synced transcript by 02:39:44 UTC, without cloud transfer. The updated
desktop was gracefully activated with running sessions preserved. See
`LIVE_LOCAL_PRODUCTION_ACCEPTANCE.md` and the backend's
`MOBILE_LIVE_LOCAL_ROLLOUT_2026_11_14.md` for receipts and cleanup details.
