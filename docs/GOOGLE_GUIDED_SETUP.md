# Guided Google Setup (Bring-Your-Own OAuth App)

Status: implemented. The CLI wizard (`jcode login google`) and the
agent-driven browser flow below share the same helpers
(`crates/jcode-base/src/auth/google/setup.rs`).

## Decision

Every user authorizes jcode's Google integrations (Gmail, Calendar, later
Drive/Docs) through **their own Google Cloud OAuth app** in the `direct`
backend. jcode does not ship a shared Google OAuth client and does not route
Google data through a third party by default.

jcode removes the setup pain two ways:

1. **CLI wizard** (`jcode login google`, or `--setup` to redo it). It uses
   `gcloud` when installed and signed in to create the project and enable
   every API, opens each remaining console page with the project
   preselected, and imports the OAuth client JSON automatically when it lands
   in Downloads.
2. **Agent-driven setup.** When a Google tool reports it is not configured,
   the agent drives the Google Cloud Console in the user's browser with the
   browser tool, then hands the download to jcode with
   `--google-client-json auto`.

### Why not the alternatives

| Option | Problem |
|---|---|
| Shared jcode-owned verified app | Gmail scopes (`gmail.readonly`, `gmail.modify`, `gmail.compose`) are *restricted*: Google verification plus a yearly CASA assessment (weeks of review, recurring cost). Calendar alone is only *sensitive* (standard verification), so a shared app stays an option for Calendar later. |
| Google's Workspace MCP servers | Developer Preview, still bring-your-own OAuth app plus an extra MCP API per product, and Gmail is read/draft/label only (no send, trash, or draft editing). |
| Composio (`JCODE_GMAIL_BACKEND=composio`) | Every request and its payload goes through Composio's proxy, and Composio holds every user's Google refresh token. Kept as opt-in only. See `GMAIL_COMPOSIO_BACKEND.md`. |
| `gog` CLI | Extra install, still bring-your-own app, and its file keyring needs `GOG_KEYRING_PASSWORD` in non-interactive agent shells. Its `gog auth setup` inspired the wizard. |

### Why bring-your-own works

- One user per app, so the 100-test-user cap never applies.
- No verification or CASA: the user is authorizing their own app.
- Traffic is machine to Google only. Tokens live in `~/.jcode/google_oauth.json` (0600).
- Adding services needs no review, just enabling the API and re-consenting.

The one unavoidable screen is Google's **"Google hasn't verified this app"**
warning at login. Every self-made app shows it for personal Gmail accounts.
Workspace accounts can avoid it by choosing an Internal audience.

## Agent playbook

Use this when `gmail` or `calendar` says it is not configured, or the user
asks to set up Google. Drive the steps with the browser tool in the user's
normal browser profile, where they are already signed in to Google. Never
type passwords or 2FA codes.

1. **Pick services.** Default to all (`gmail,calendar`) so the user only goes
   through the console once.
2. **Project and APIs.** If `gcloud config get-value account` prints an
   account, run:
   ```
   gcloud projects create jcode-<6 digits> --name jcode --quiet
   gcloud services enable gmail.googleapis.com calendar-json.googleapis.com --project <id> --quiet
   ```
   Otherwise open `https://console.cloud.google.com/projectcreate`, create a
   project named `jcode`, then open one link that enables every API:
   `https://console.cloud.google.com/flows/enableapi?apiid=gmail.googleapis.com,calendar-json.googleapis.com&project=<id>`
3. **Consent screen.** `https://console.cloud.google.com/auth/branding?project=<id>`.
   If it says "Google Auth Platform not configured yet", click Get Started:
   app name `jcode`, support and contact email = the user's email, audience
   External (Internal for Workspace accounts that allow it), agree, Create.
   Do not add scopes. jcode requests them at login.
4. **Publish the app.** `https://console.cloud.google.com/auth/audience?project=<id>`,
   "Publish app", confirm. Required: in Testing mode Google expires the login
   after **7 days**. Publishing your own app needs no review.
5. **Create the OAuth client.** `https://console.cloud.google.com/auth/clients/create?project=<id>`,
   type **Desktop app**, name `jcode`, Create, then **Download JSON**. Desktop
   clients accept jcode's loopback redirect without registering URIs.
6. **Import and log in.**
   ```
   jcode login google --google-client-json auto --google-services gmail,calendar
   ```
   `auto` picks the newest valid `client_secret*.json` in Downloads. The secret
   goes straight from the file into `~/.jcode/google_credentials.json` and
   never enters the transcript. Before the user signs in, tell them about the
   unverified-app warning (Advanced, then Go to jcode) and to keep every box
   ticked.
7. **Verify.** One read call per granted service (list recent mail, list
   upcoming events), then report which services are active.

## Failure handling

| Symptom | Cause | Action |
|---|---|---|
| `access_denied` / "app is blocked" for a Workspace account | Admin blocks unverified third-party apps | Ask the admin to allow the client ID, or use a personal account. Do not retry. |
| `accessNotConfigured` / "has not been used in project" | API not enabled | Re-open the enable link from step 2, wait about a minute, retry. |
| `No refresh token received` | Prior grant exists without offline consent | Revoke at `https://myaccount.google.com/permissions`, log in again. |
| Works, then fails about a week later with `invalid_grant` | App left in Testing mode | Publish the app (step 4), log in again. jcode's refresh error says this too. |
| Import says "Web application client" | Wrong client type | Create a Desktop app client (step 5). |
| Console UI moved or a selector fails | Google Cloud UI changes | Fall back to `jcode login google --setup`, which opens each page. |

## Adding a service later

Re-run `jcode login google --google-services <existing>,<new>`. The login
keeps already-granted services and requests the union of scopes
(`include_granted_scopes=true`). Enable the new service's API first.

## Remaining work

- Desktop: surface the same flow from the Desktop settings Accounts page,
  through the SDK, rather than a Desktop-only implementation.
- Browser-tool reliability: the console flow is long and multi-page, so the
  agent path depends on dependable browser handoff.
