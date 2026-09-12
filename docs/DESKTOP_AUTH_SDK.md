# Native desktop login with the Rust SDK

`jcode-sdk` exports `AuthClient`, `AuthOptions`, `LoginProvider`, `LoginMethod`,
`AuthFlow`, `AuthPrompt`, `AuthInputKind`, and `AuthResult`.

## UI contract

- Intercept `/login` locally before `send_message`. A clickable Login action uses
  the same controller. Do not add auth input, URLs, or subprocess output to chat.
- Build the picker with `AuthClient::providers()`. `resolve_provider()` accepts
  shared catalog IDs, aliases, and display names, filtered to supported methods.
- `LoginMethod::ApiKey` uses `JcodeClient::set_api_key(provider.id, key)`. Keys
  belong in a masked, transient input. Clear that input after submission.
- For OAuth/device code, `begin(provider.id, account)` allocates an `AuthFlow`
  without starting I/O. Retain a clone for the Cancel button. On a worker thread,
  call `start()` and render its `AuthPrompt` only inside the login panel.
- Open `prompt.auth_url` in the system browser with a native Open Browser button.
  Show a dedicated callback/code input or the device code as directed by
  `AuthInputKind`. Use `submit_callback`, `submit_code`, or `complete_device` on a
  worker thread. This scriptable flow does not run a localhost callback listener.
- Call `cancel()` off the UI thread when closing/cancelling the panel, including
  while device polling or begin is running. It kills/reaps the owned child and
  cleans only that flow ID. Dropping the last clone also schedules bounded cleanup.
  Cancellation does not revoke credentials already issued by an exchange.
- `AuthResult.validation_warning` means credentials were saved, but validation
  failed. Do not retry a spent code. Offer model selection/recovery instead.

## Runtime and compatibility

`AuthOptions` selects a trusted local executable, `JCODE_HOME`, and the **daemon**
socket (not the harness API socket). Defaults use `JCODE_BIN` or `jcode` on PATH,
inherit the credential home, and use the normal daemon socket. The client is
local-only. A desktop attached over SSH must explicitly disable this local flow.

The existing scriptable CLI handles scoped pending state, PKCE/state validation,
and credential persistence. Callback/code input travels through stdin, never argv.
The SDK bounds subprocess output, suppresses stderr, and returns redacted errors.
`AuthPrompt` intentionally has no `Debug`/serialization implementation.

Normal CLI completion already notifies the daemon. The SDK sends a best-effort
legacy auth-change notification after saved-but-unvalidated completion, so this
path needs no daemon or harness upgrade. The additive
`JcodeClient::notify_auth_changed()` / TypeScript `notifyAuthChanged()` API requires
an updated harness bridge, advertised as `auth_changed_notification`. Its reply
acknowledges the notification, not completion of asynchronous model discovery.

OAuth supports Claude, OpenAI, Gemini, Antigravity, and Google. Copilot supports
device code. Google requires previously configured OAuth client credentials.
Jcode subscription supports API-key entry here, not interactive device login.
Other CLI-only providers are excluded instead of falling back to a terminal.

## Verification

Unit tests cover catalog resolution, secret-free stdin transport, bounded errors,
validation warnings, daemon notification, timeout/reaping, concurrent cancellation,
and drop cleanup. An opt-in `installed_cli_begin_cancel_isolated` test uses
`JCODE_AUTH_TEST_BINARY` with empty temporary homes for Claude/OpenAI begin/cancel.
It neither opens browsers nor completes OAuth or prints authorization URLs.
