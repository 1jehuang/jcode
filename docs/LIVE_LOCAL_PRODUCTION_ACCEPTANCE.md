# Own-account live computer message acceptance

This uses the already-running local daemon and `https://api.jcode.sh/v1`, not a synthetic host. It does not build, install, restart, deploy, migrate, change mobile settings, or touch billing. Run from this repository with Node 20 or newer. Only the dedicated debug-created session receives messages.

## Activation gate

`prepare` may run before backend activation. It creates an `IsolatedTest` memory-scope session using `~/.local/bin/jcode debug create_session:<private-directory>/workspace` and performs one minimal local inference so the normal daemon uploader persists and publishes the session. Its provider/model is inherited from the daemon. The warm reply must exactly match its unique token. If inheritance selects an unavailable model, inspect `debug --session ID state` and adjust **only this dedicated session** with `set_provider:...` / `set_model:...`. Do not alter the daemon defaults or any existing user session.

If a warm attempt failed, after adjusting only that dedicated provider/model run `node scripts/accept_live_local_message.mjs warm "$D"` to recover the `created` checkpoint. This does not enqueue a production command.

`check` is read-only against production. It requires the real daemon registration, matching working directory, fresh live presence, and the uploaded warm reply. The root coordinator must explicitly confirm backend activation before `run`. Do not interpret source changes, passing mock tests, a TestFlight build, or a fresh desktop binary as activation.

```sh
D="$HOME/.jcode/scratch/live-local-production-20261114"
node scripts/accept_live_local_message.mjs prepare "$D"
```

Create `$D/auth.json` privately with mode `0600`, containing `{"api_base":"https://api.jcode.sh/v1","api_key":"OWN_ACCOUNT_CREDENTIAL"}`. Never paste or print the credential. For the existing account's owner-only Jcode env file, this Node snippet copies only the needed fields without printing them or executing shell config:

```sh
node --input-type=module - "$D" <<'NODE'
import { readFileSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
const source = readFileSync(join(homedir(), '.config/jcode/jcode-subscription.env'), 'utf8')
const values = {}
for (const line of source.split('\n')) {
  const m = line.match(/^\s*(?:export\s+)?(JCODE_API_KEY|JCODE_API_BASE)\s*=\s*(.*?)\s*$/)
  if (m) values[m[1]] = m[2].replace(/^(['"])(.*)\1$/, '$2')
}
if (!values.JCODE_API_KEY) throw new Error('Account credential absent')
writeFileSync(join(process.argv[2], 'auth.json'), JSON.stringify({
  api_base: values.JCODE_API_BASE || 'https://api.jcode.sh/v1', api_key: values.JCODE_API_KEY
}), { mode: 0o600, flag: 'wx' })
NODE
node scripts/accept_live_local_message.mjs check "$D"
# ONLY after explicit activation confirmation from root:
JCODE_PRODUCTION_LOCAL_ACTIVATED=yes node scripts/accept_live_local_message.mjs run "$D"
node scripts/accept_live_local_message.mjs cleanup "$D"
```

## Evidence and interpretation

The single enqueue uses precisely the mobile composer's `POST /v1/mobile/commands` body: `kind: "message"`, original `session_id`, trimmed `text`. The prompt asks for exactly a unique token and no tools. The script persists `enqueue_attempted` **before** sending and never retries a POST, including an ambiguous network failure. Running again cannot enqueue a duplicate. A failed run retains its dedicated session and private state for diagnosis.

It checks `target: "local"`, the original command/session identity, terminal command `done`, and no different `result_session_id`. `done` acknowledges delivery only, so the script independently waits for exact assistant output in both actual local debug history and paginated synced transcript. It rejects tool calls/results and requires the source and working directory to remain local and unchanged. It records all acceptance HTTP paths in owner-only `evidence.json`. The allowlist has no transfer, cloud creation, cloud attach, host wake, or billing endpoint. This proves local routing and same-session inference from client-visible evidence. It does not independently audit backend-internal host wake callbacks or billing ledgers, and does not claim a physical iPhone UI interaction.

`cleanup` only destroys the dedicated session recorded in its private state, after successful evidence capture and a matching actual daemon working-directory check. It keeps local transcript/evidence files and the synced transcript for review rather than deleting account history. Auth config remains private and can be removed separately after evidence retention requirements are satisfied. It never bulk-deletes sessions.

## Troubleshooting checkpoints

- `acceptance.json` has the created ID and lifecycle phase, never a credential.
- `created` means warm inference did not complete or the CLI failed. Inspect only that ID's `state`, `history`, and `last_response`. Do not repeat preparation in the same directory.
- `prepared` plus a failed `check` means the real warm response, uploader, or fresh presence is not ready. Allow ordinary sync reconciliation and retry `check`, not `run`.
- `enqueue_attempted` means uncertain delivery or a later assertion failure. Inspect command status with the private credential and inspect the dedicated session. Never reset the phase to retry.
- `passed` means evidence has been written and cleanup is safe. `cleaned` means that dedicated runtime registration was destroyed.

Protocol references: `~/jcode-mobile/SYNCED_CONTINUATION.md`, desktop `docs/CLOUD_LATENCY.md`, `crates/jcode-base/src/mobile_sync.rs`, `crates/jcode-base/src/mobile_runner.rs`, and backend `workers/api/src/mobile_sync.js` plus `docs/MOBILE_LIVE_LOCAL_MESSAGING.md`. No Rust build is required, particularly on battery.
