# Guided Google Setup (Bring-Your-Own OAuth App)

Status: plan. Depends on the generalized Google login (`--google-services`)
and a reliable browser tool.

## Decision

Every user authorizes jcode's Google integrations (Gmail, Calendar, later
Drive/Docs) through **their own Google Cloud OAuth app** in the `direct`
backend. jcode does not ship a shared Google OAuth client and does not route
Google data through a third party by default.

jcode removes the setup pain by **driving the Google Cloud Console setup with
the browser tool**, so the user only signs in and approves screens.

### Why not the alternatives

| Option | Problem |
|---|---|
| Shared jcode-owned verified app | Gmail scopes (`gmail.readonly`, `gmail.modify`, `gmail.compose`) are *restricted*: Google verification plus a yearly CASA assessment (roughly $540+/yr, weeks of review). Calendar alone is only *sensitive* (free review), so this stays an option for Calendar later. |
| Composio (`JCODE_GMAIL_BACKEND=composio`) | Every request and its payload goes through Composio's proxy, and Composio holds every user's Google refresh token. Consent screen shows "Composio". Shared quota, external dependency. Kept as opt-in only. See `GMAIL_COMPOSIO_BACKEND.md`. |
| `gog` CLI | Extra install, still bring-your-own app, and its file keyring needs `GOG_KEYRING_PASSWORD` in non-interactive agent shells. |

### Why bring-your-own works

- One user per app, so the 100-test-user cap never applies.
- No verification or CASA: the user is authorizing their own app.
- Traffic is machine to Google only. Tokens live in `~/.jcode/google_oauth.json` (0600).
- Adding services (Calendar, Drive, ...) needs no review, just enabling the API and re-consenting.

## Flow

Triggered when a Google tool (`gmail`, `calendar`) is called with no
credentials, or when the user asks to set up Google. The agent runs it with
the browser tool, in the user's normal browser profile where they are already
signed in to Google.

1. **Pick the account and services.** Confirm which Google account and which
   services (`gmail`, `calendar`). Default to both so the user only goes
   through the console once.
2. **Create a project.** `https://console.cloud.google.com/projectcreate`,
   name `jcode`. Wait for creation and select the project.
3. **Enable APIs** for every selected service, up front:
   - Gmail: `https://console.cloud.google.com/apis/library/gmail.googleapis.com`
   - Calendar: `https://console.cloud.google.com/apis/library/calendar-json.googleapis.com`
4. **Configure the consent screen** (Google Auth Platform / OAuth consent screen):
   - User type: External (Internal is fine for Workspace accounts that allow it).
   - App name `jcode`, support and developer email = the user's email.
   - Do not add scopes here. jcode requests them at login.
5. **Publish the app** (Audience page, "Publish app" / "In production").
   This is required. In "Testing" mode Google expires refresh tokens after
   **7 days**, which forces a weekly re-login. Personal-use published apps do
   not need verification.
6. **Create the OAuth client.** Credentials, Create credentials, OAuth client
   ID, application type **Desktop app**, name `jcode`. jcode's loopback
   redirect (`http://127.0.0.1:<port>`) works with Desktop clients without
   registering redirect URIs.
7. **Save credentials.** Read the client ID and secret from the creation
   dialog (or the downloaded JSON) and write `~/.jcode/google_credentials.json`
   with owner-only permissions. The secret goes straight from the page to the
   file. Never echo it into the transcript.
8. **Run the login.** `jcode login google --google-services gmail,calendar`
   (add `--google-access-tier readonly` if the user wants read and draft only).
   The user approves the consent screen.
9. **Verify.** Make one read call per granted service (list recent mail, list
   upcoming events) and report which services are active.

## What the user must do themselves

- Sign in to Google if needed. The agent never types passwords or 2FA codes.
- Click through the **"Google hasn't verified this app"** screen at login:
  Advanced, then "Go to jcode (unsafe)". Warn about this before step 8 and
  explain it is expected: it is their own app.
- Approve the consent screen, keeping every requested permission ticked.
  Unticked permissions mean that service is not granted (login reports
  "Not granted").
- Accept Google Cloud terms of service on first console use.

## Failure handling

| Symptom | Cause | Action |
|---|---|---|
| `access_denied` / "app is blocked" for a Workspace account | Admin blocks unverified third-party apps | Tell the user to ask their admin to allow the client ID, or use a personal account. Do not retry. |
| `Gmail API has not been used in project` / `accessNotConfigured` | API not enabled | Re-open the step 3 link for that service, enable, wait about a minute, retry. |
| `No refresh token received` | Prior grant exists without offline consent | Revoke at `https://myaccount.google.com/permissions`, log in again. |
| Login works, then fails a week later with `invalid_grant` | App left in Testing mode | Publish the app (step 5), log in again. |
| Console UI moved or a selector fails | Google Cloud UI changes | Fall back to the manual instructions in `jcode login google` option [3]. |

## Adding a service later

Re-run `jcode login google --google-services <existing>,<new>`. The login
keeps already-granted services and requests the union of scopes
(`include_granted_scopes=true`). The agent should first enable the new
service's API in the same project (step 3) so the first call does not fail.

## Implementation notes

- The guided flow is agent-driven (browser tool plus shell), not a new
  interactive CLI wizard. `jcode login google` keeps its manual paths as the
  fallback.
- Tools should return a structured "not configured" message that names the
  missing service and suggests guided setup, so the agent can offer it
  instead of telling the user to run a CLI command.
- Prerequisite: browser-tool reliability. User reports show intermittent
  failures driving Chrome even with the Browser Agent Bridge installed. The
  console flow is long and multi-page, so it should only be enabled by
  default once browser handoff is dependable.
- Desktop: surface the same flow from the Desktop settings Accounts page,
  through the SDK, rather than a Desktop-only implementation.
