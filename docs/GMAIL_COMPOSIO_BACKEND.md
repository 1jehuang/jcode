# Gmail Tool: Composio Managed Backend

The native `gmail` tool can source credentials and transport from one of two
backends. The tool interface, confirmation gating, access-tier logic, and
token-lean output formatting are identical across backends; only the
auth/transport layer changes.

## Backends

| Backend | Auth | Pros | Cons |
|---|---|---|---|
| `direct` (default) | Local Google OAuth tokens (`jcode login google`) | No third party in the loop | Unverified-app warning; 7-day refresh-token expiry in Google "Testing" mode |
| `composio` | Composio-managed OAuth (Google-verified app) | No unverified-app warning, no 7-day expiry, no per-user Google Cloud project | Composio brokers Gmail token custody; external dependency/cost |

Both backends call the *same* Gmail REST endpoints
(`https://gmail.googleapis.com/gmail/v1/users/me/...`). The Composio backend
routes those calls through Composio's
[`proxy-execute`](https://docs.composio.dev/reference/api-reference/tools/postToolsExecuteProxy)
endpoint, which attaches the managed Gmail credentials. Because the upstream
response shape is unchanged, all existing typed parsing and output formatting
is reused.

## Selecting the backend

The backend is resolved from environment at `GmailClient::new()`:

- `JCODE_GMAIL_BACKEND=direct` (or unset) -> direct Google backend.
- `JCODE_GMAIL_BACKEND=composio` -> Composio backend (requires `COMPOSIO_API_KEY`).

If `composio` is requested but `COMPOSIO_API_KEY` is missing, jcode warns and
falls back to `direct`.

### Composio environment variables

| Variable | Required | Description |
|---|---|---|
| `COMPOSIO_API_KEY` | Yes | Project API key from <https://platform.composio.dev> |
| `COMPOSIO_BASE_URL` | No | Override API base (default `https://backend.composio.dev/api/v3.1`) |
| `COMPOSIO_GMAIL_AUTH_CONFIG_ID` | For `connect` | Gmail auth config id (`ac_...`) from the Composio dashboard. Defines the OAuth blueprint/scopes used by the connect flow. |
| `COMPOSIO_GMAIL_CONNECTED_ACCOUNT_ID` | No | Pin a specific connected account (`ca_...`). Normally set automatically after `connect`. |
| `COMPOSIO_GMAIL_USER_ID` / `COMPOSIO_USER_ID` | No | End-user id for multi-user connected accounts (defaults to `default`) |

## Connecting a Gmail account (in-agent OAuth)

Once `COMPOSIO_API_KEY` and `COMPOSIO_GMAIL_AUTH_CONFIG_ID` are set, the user
(or the agent) runs the gmail tool with `action: "connect"`:

1. jcode calls Composio's `POST /connected_accounts/link` (hosted "Connect
   Link" flow) to start an OAuth session.
2. The returned `redirect_url` is opened in the system browser (printed to
   stderr as a fallback, e.g. over SSH).
3. The user approves Gmail access on Google's consent screen. Because Composio
   owns a Google-verified app, there is no "unverified app" warning.
4. jcode polls `GET /connected_accounts/{id}` until the connection is `ACTIVE`,
   then persists it to `~/.jcode/composio_gmail.json`.

Future sessions load the persisted `connected_account_id`, so the connect step
is a one-time action per account. Tool calls before a connection exists return
a hint telling the agent to run `action: "connect"` first.

> Note: Composio is retiring `initiate()` for managed OAuth in favor of the
> Connect Link `link()` flow used here, so this path is the supported one going
> forward.

## One-time Composio setup

1. Sign in at <https://platform.composio.dev> and copy your project API key.
2. Connect a Gmail account (Composio's hosted OAuth, no unverified-app warning).
   Note the resulting `connected_account_id` if you want to pin it.
3. Export the variables:
   ```bash
   export JCODE_GMAIL_BACKEND=composio
   export COMPOSIO_API_KEY="ck_..."
   # optional:
   export COMPOSIO_GMAIL_CONNECTED_ACCOUNT_ID="ca_..."
   export COMPOSIO_GMAIL_USER_ID="me"
   ```
4. Ensure the `gmail` tool is enabled in `config.toml`:
   ```toml
   [tools]
   enabled = ["*"]
   ```

## Access tiers

- `direct`: honors the access tier chosen at `jcode login google`
  (Read & Draft Only logins cannot send/trash, enforced at the OAuth scope level).
- `composio`: connections request full Gmail scopes, so send/trash are
  available. The tool still requires explicit `confirmed: true` for send,
  send_draft, and trash.

## Trust note

With the Composio backend, Composio holds your Gmail OAuth grant and sees API
traffic. This is the core tradeoff versus the direct backend. Disclose this to
users before enabling it as a default.
