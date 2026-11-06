# Continue a local conversation on Jcode Cloud

Mobile transcript sync is opt-in: `jcode mobile enable` requires a signed-in
Jcode account with mobile-sync entitlement. `jcode mobile status` reports the
local setting. Background sync uploads visible conversation text, summarized
tool calls, and truncated tool results. It is not a file or credential backup.

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
