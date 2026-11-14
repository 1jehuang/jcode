# Own-account live computer message acceptance

This uses the already-running local daemon and `https://api.jcode.sh/v1`, not a synthetic host. It does not build, install, restart, deploy, migrate, change mobile settings, or touch billing. Run from this repository with Node 20 or newer. Only the dedicated debug-created session receives messages.

## Headless debug-session prerequisite

Current debug-created sessions are marked `is_debug=true`, and ordinary mobile sync intentionally skips them. `prepare` creates the requested `IsolatedTest` setup session. After its warm inference succeeds, `wire` creates a **new**, dedicated nondebug session on the same actual daemon using supported normal `Subscribe`, with no `target_session_id`, the same private working directory, and `continue_on_disconnect:true`. The registry `~/.jcode/servers.json` must identify exactly one active daemon. The returned `SessionId` becomes the only production message destination. `wire` configures only that new ID to OpenAI `gpt-6.1-sol`, and `warm` verifies its real inference and persistence. No persisted flags or production source are modified. The old debug ID is retained only for scoped cleanup after successful evidence. Targeted `agent:info` proves registration because `debug sessions` lists connected clients, not detached/headless registrations. Normal daemon upload and heartbeat must independently publish the warm transcript and live presence before `check` passes.

## Activation gate

`prepare` may run before backend activation. It creates an `IsolatedTest` memory-scope session using `~/.local/bin/jcode debug create_session:<private-directory>/workspace` and performs one minimal local inference to verify that test session. The `wire` and `warm` steps then prepare the normal session used for real uploader acceptance. Its provider/model is inherited from the daemon. The warm reply must exactly match its unique token. If inheritance selects an unavailable model, inspect `debug --session ID state` and adjust **only this dedicated session** with `set_provider:...` / `set_model:...`. Do not alter the daemon defaults or any existing user session.

If a warm attempt failed, after adjusting only that dedicated provider/model run `node scripts/accept_live_local_message.mjs warm "$D"` to recover the `created` checkpoint. This does not enqueue a production command.

`check` is read-only against production. It requires the real daemon registration, matching working directory, fresh live presence, and the uploaded warm reply. The root coordinator must explicitly confirm backend activation before `run`. Do not interpret source changes, passing mock tests, a TestFlight build, or a fresh desktop binary as activation.

```sh
D="$HOME/.jcode/scratch/live-local-production-20261114"
node scripts/accept_live_local_message.mjs prepare "$D"
# If inherited setup provider is unavailable, adjust only that debug ID and use warm.
node scripts/accept_live_local_message.mjs wire "$D"
node scripts/accept_live_local_message.mjs warm "$D"
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

`cleanup` only destroys the dedicated normal session and its recorded setup debug session, after successful evidence capture and a matching actual daemon working-directory check. It keeps local transcript/evidence files and the synced transcript for review rather than deleting account history. Auth config remains private and can be removed separately after evidence retention requirements are satisfied. It never bulk-deletes sessions.

## Offline script safety validation

`node --test scripts/accept_live_local_message.test.mjs` validates parsing, a production run failing activation before daemon/API access, and source invariants for the single persisted enqueue. It makes no network requests, creates no daemon sessions, and is not a substitute for the production run.

## Troubleshooting checkpoints

- Failures print a content-free checkpoint label, including an HTTP status when available, never raw response bodies, credentials, or child stderr.
- `acceptance.json` has the created ID and lifecycle phase, never a credential.
- `created` means warm inference did not complete or the CLI failed. Inspect only that ID's `state`, `history`, and `last_response`. Do not repeat preparation in the same directory.
- `prepared` plus a failed `check` means the real warm response, uploader, or fresh presence is not ready. Allow ordinary sync reconciliation and retry `check`, not `run`.
- `wire_attempted` means normal Subscribe was attempted and its identity may need recovery from `debug swarm:members` by the exact private working directory. Do not repeat session creation blindly.
- `readiness.json` records source, working directory, warm sync, model/provider and whether production exposes live presence, without an account credential.
- `enqueue_attempted` means uncertain delivery or a later assertion failure. Inspect command status with the private credential and inspect the dedicated session. Never reset the phase to retry.
- `passed` means evidence has been written and cleanup is safe. `cleaned` means that dedicated runtime registration was destroyed.

Protocol references: `~/jcode-mobile/SYNCED_CONTINUATION.md`, desktop `docs/CLOUD_LATENCY.md`, `crates/jcode-base/src/mobile_sync.rs`, `crates/jcode-base/src/mobile_runner.rs`, and backend `workers/api/src/mobile_sync.js` plus `docs/MOBILE_LIVE_LOCAL_MESSAGING.md`. No Rust build is required, particularly on battery.
