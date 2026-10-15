# Appendices: OAuth and bridge verification detail

Supporting material for `PORTAL-FINDINGS.md`. Read that file first for the
authenticated Portal observations; this holds the deeper OAuth, CIMD and
bridge-verification records.

## Appendix A. OAuth discovery detail

### 1.9b RFC 9728 protected-resource metadata — verified present

Portal publishes genuine MCP authorization-discovery documents (real
`application/json`, unlike the CIMD false positive above):

`GET /.well-known/oauth-protected-resource` ->
```json
{"resource":"https://phenotype.spotifyportal.com/api/mcp-actions/",
 "authorization_servers":["https://phenotype.spotifyportal.com/api/auth"],
 "scopes_supported":["openid","offline_access"]}
```
`GET /.well-known/oauth-protected-resource/v1` -> the same document with
`"resource":"https://phenotype.spotifyportal.com/api/mcp-actions/v1"`, matching
the MCP endpoint path exactly.

The path-scoped variant
`/api/mcp-actions/.well-known/oauth-protected-resource` returns **401**, so
metadata is served only from the origin root, not per-resource. Worth knowing:
a client that derives the metadata URL from the resource URL rather than trying
the origin root will get a 401 and may misdiagnose the endpoint as
unconfigured.

Consequence for the bridge plan: the full discovery chain a compliant MCP client
needs is present and consistent —
`protected-resource metadata -> authorization_servers -> /.well-known/oauth-authorization-server`
— and both `resource` values line up with
`https://phenotype.spotifyportal.com/api/mcp-actions/v1`. RFC 8707 resource
binding against that exact `resource` string is therefore well-defined, which
is what `mcp-remote --resource` should be pointed at. The remaining gap is still
client identity (B6), not discovery.

## Appendix B. CIMD allow-list enforcement — proven with controls

### 1.10b CIMD allow-list enforcement — PROVEN, with controls (B6 confirmed)

This is the decisive experiment for XJ-03 / blocker B6. Read-only `GET` requests
only; **no credentials were sent, no consent was granted, and nothing was
authorized.** Only `client_id`, `response_type`, `redirect_uri`, `scope` and a
dummy PKCE challenge were supplied.

| `client_id` (CIMD URL) | Host resolvable | Result |
|---|---|---|
| `https://xirp.spotify.com/oauth/client-metadata.json` (**allow-listed**) | yes | **302** -> `https://phenotype.spotifyportal.com/oauth2/authorize/2e80fd66-...` |
| `https://kooshapari.com/oauth/client-metadata.json` | yes (200) | **400** `InputError` |
| `https://jcode.sh/oauth/client-metadata.json` | yes | **400** `InputError` |
| `https://example.invalid/oauth/client-metadata.json` | no (DNS fail) | **400** `InputError` |
| *(no `client_id` at all)* | n/a | **400** |

Exact rejection body:
```json
{"error":{"name":"InputError",
  "message":"Invalid client_id 'https://kooshapari.com/oauth/client-metadata.json'"},
 "request":{"method":"GET","url":"/v1/authorize?client_id=..."},
 "response":{"statusCode":400}}
```

Why the controls matter: `kooshapari.com` and `jcode.sh` both resolve over
HTTPS, so their 400s cannot be blamed on DNS or on the shape of the URL. Only the
allow-listed Xirp URL is accepted. The gate is therefore **client
allow-listing specifically**, exactly as B6 predicted, and not protocol
capability, not PKCE support, and not network reachability.

The consent page itself is reachable and returns `200 text/html`, so the flow
proceeds normally once a permitted client is presented.

What this means operationally:
- **B6 is confirmed real and is the only remaining blocker for authenticated
  Portal MCP.** Everything else in the chain is verified working.
- A CIMD client does **not** need to pre-register: the authorization server will
  accept any URL that matches an allow-listed pattern. So the fix is narrow —
  obtain authorization to add exactly one pattern for our own domain.
- Our CIMD document must be served over **HTTPS from a stable, publicly
  reachable URL**, and must declare `redirect_uris` including a loopback
  callback, `grant_types` including `authorization_code` and `refresh_token`,
  `token_endpoint_auth_method: "none"`, and scope `openid offline_access` — the
  same shape as the verified Xirp document.

Do not "solve" this by reusing Xirp's CIMD URL. That authenticates as Xirp and
would attribute Jcode's actions to it in Portal's audit trail.

## Appendix C. PR 619 non-catalog validation

Checked at the PR's real base `b69b4f4bf` rather than my commit parent:

- `bench.yml` parses as YAML (jobs: `bench`, `compare-baseline`).
- The repaired comparison block passes `bash -n`. The `+ fi` added in the PR
  correctly closes the `if [ -f baseline-results/criterion.txt ] && [ -f
  current-results/criterion.txt ]; then` opened above the Python heredoc. This is
  a **valid repair, not a bug** — I initially suspected a missing `if` and was
  wrong.
- `--bench criterion_bench` matches the declared `[[bench]]` target in
  `Cargo.toml` (`name = "criterion_bench"`, `harness = false`), and
  `benches/criterion_bench.rs` exists.
- The placeholder limitation is still real: `benches/criterion_bench.rs` is 6
  lines and benchmarks `1 + 1`. Not performance evidence, and now stated as such
  in the PR body.
- Descriptors are YAML with trailing commas, so they are **not** strict JSON
  documents. Their *parsed values* are JSON-serializable, which is what the
  README's "JSON-compatible values" wording actually claims. Both readings are
  recorded so the next session does not "fix" a non-issue.

## Appendix D. Bridge selection detail

### 1.10 Bridge selection for XJ-03 — DECIDED, not yet deployed

Read-only research (no installs, no config writes). Full report preserved at
`evidence/mcp-bridge-research.md`.

**Verdict: `punkpeye/mcp-remote` (npm `mcp-remote`) is the only viable bridge.**

| Candidate | Streamable HTTP | OAuth PKCE | DCR | **CIMD** | Verdict |
|---|---|---|---|---|---|
| `mcp-remote` v0.15.1 (2026-10-14) | yes | yes | yes | **yes** (`--client-metadata-url`) | **SELECTED** |
| `sparfenyuk/mcp-proxy` v0.13.0 | yes | **no PKCE, no refresh** | no | no | static-bearer only |
| `mikluko/mcp-proxy` v0.1.0 (2024-11) | yes | yes | yes | no | unmaintained, ~2 commits |
| official MCP org | — | — | — | — | **no official stdio-to-remote bridge exists** |

CIMD support is the discriminator: only `mcp-remote` implements SEP-991. That is
exactly what Portal needs, since CIMD is the mechanism its allow-list is built
around. Caveat recorded honestly: CIMD support was verified from the live README,
not by reading the TypeScript source.

**Rejected sample config.** The research proposed pointing the bridge at Xirp's
own CIMD URL:
```
"--client-metadata-url", "https://xirp.spotify.com/oauth/client-metadata.json"
```
This would authenticate **as Xirp** — `client_name: "Xirp"`. That is
impersonation of a supported agent, which is explicitly out of bounds, and it
would make Portal's audit trail attribute Jcode's actions to Xirp. **Rejected.**

The correct config needs **our own** CIMD document, which requires authorization
to add a pattern to Portal's allowed-client list (B6). Sketch only, not applied:
```jsonc
{
  "mcpServers": {
    "phenotype-spotify": {
      "command": "npx",
      "args": ["-y", "mcp-remote@<pinned-version>",
               "https://phenotype.spotifyportal.com/api/mcp-actions/v1",
               "--transport", "http-first",
               "--client-metadata-url", "<OUR-HTTPS-CIMD-URL>",
               "--resource", "https://phenotype.spotifyportal.com/api/mcp-actions/v1"]
    }
  }
}
```
Version must be pinned, not `@latest`, for reproducibility.

**Hard parts flagged for whoever builds it:** token refresh must be serialized to
avoid refresh-token rotation races (7-day refresh lifetime, 30-day max session);
token cache needs `chmod 0600`; `token_endpoint_auth_method` must be read from
server metadata rather than assumed `none`.

## Acceptance criteria for the 16 suggestions, prepared for the operator

Accepting is the operator's call, so this is a decision aid rather than an
action. The two items that block acceptance are already established above.
This adds the remaining criteria, derived from the verified rules, so the
decision can be made without re-deriving them.

### Criterion 1: do not accept anything that asserts private-repo content

Verified basis: the 11 private repositories have **no usable ingested content**
(their sync rows resolve to blank), and `PhenoRegistry`'s is frozen. So no
synthesis could have been written from them. A suggestion that describes private
repository internals is therefore asserting something the evidence does not
support, and must be rejected rather than accepted with a caveat.

Two suggestions are named in ways that risk this, and should be checked against
their content before acceptance:
- **AI Routing, Inference, and Evaluation** — describes provider gateways and
  "local MLX inference". `PhenoMLX` is **public** and synced, so `PhenoMLX`-based
  claims are supportable. But `HarnessDesk`, `PhenoLab` and `PhenoBot` are
  private, and if the page leans on those it is asserting unavailable evidence.
- **Runtime and Agent Infrastructure** — names Herdr, cmux, ShareCLI,
  HeliosLite, Jcode, HeliosCLI, Herdr plugins and Ratatui. All of those are
  public and synced, so this one is on firmer ground than it first appears.

### Criterion 2: do not accept anything that claims coverage the numbers do not support

Verified basis: 42 attached, 36 ingested = **85.7%**, and the 6-row gap is the
private repositories rather than a transient sync. Any page claiming complete
or full 42-repository coverage is contradicted by the Sources view, on the same
footing as the disproved PhenoDesign claim.

This makes **Portfolio Evidence and Coverage** the one page where the correct
numbers are most likely to be got wrong, since it is the page that owns them.
Its own description promises it "records attached versus ingested source counts,
no-content resources, archive/read-only caveats, and acceptance-boundary
rules" — which is exactly right in intent. Check the rendered counts against 42
/ 36 / 12-private before accepting.

### Criterion 3: identity correctness, already established

Four cited repository names do not exist (`DinoForge`, `WorldSphereMod3D`,
`Civic Survival`, bare `Helios`). Any suggestion citing them needs regeneration,
not acceptance.

### What I could not check
The suggestion *bodies* were not readable through this bridge: `browser eval` is
unavailable, and the per-suggestion `View content` control is exposed only for
the currently-selected row, so navigating to a specific suggestion by index did
not work. Criterion 1 and Criterion 2 above are therefore stated as checks to
perform on the rendered content, not as findings about it. What **is** a finding
is the identity audit in Criterion 3, which was done against live GitHub rather
than against the UI.

## MAJOR: KooshaPari is a Portal Admin, and the CIMD allow-list is visible

Found while acting on the pending items. This changes what is possible.

**`kooshapari@kooshapari.com` is a member of "Portal Admins" (2 members).** Visible
in the profile menu under Groups. That means the admin surfaces are reachable,
not just the workspace.

### The CIMD configuration is an editable admin setting — B6 is actionable here

`App Settings -> General -> MCP` exposes, as live form fields:

| Setting | Observed |
|---|---|
| Enable client ID metadata documents | toggle present (CIMD **is** enabled) |
| Enable refresh tokens | toggle present |
| Token lifetime (days) | field, "Default: 7 days. Maximum: 14" |
| Maximum session lifetime (days) | field, "Default: 30 days. Maximum: 90" |
| **Allowed client ID patterns** | **exactly 4 "Client ID pattern" inputs** |
| Allowed redirect URI patterns | "Add item" (list empty, so Backstage defaults apply) |
| Announce MCP to users | toggle |

The **4 pattern inputs** match the 4 patterns in the checkpoint exactly, which
independently corroborates that record:
`https://claude.ai/*`, `https://chatgpt.com/oauth/codex/*/client.json`,
`https://backstage.spotify.com/chirp.json`,
`https://xirp.spotify.com/oauth/client-metadata.json`.

So **B6 is not a vendor ticket at all** — it is a field in a settings form the
operator can already reach, and the "Add item" control appends a fifth. The
field help states the semantics precisely: "Client ID URL patterns that are
allowed to resolve as Client ID Metadata Documents. Use * as a wildcard, for
example https://example.com/*. When no list is configured, Backstage allows
Claude, VS Code, and the built-in CLI. A custom list replaces these defaults,
although the Portal CLI client ID remains allowed automatically."

Two consequences that matter:

1. **Adding a pattern must not narrow the existing four.** The help text says a
   custom list *replaces* the defaults. If the four inputs were cleared and
   repopulated with fewer entries, Claude Code and the ChatGPT/Codex client
   would stop being able to authenticate. Any edit must append, never replace.
2. **The redirect-URI question may already be answered.** "Allowed redirect URI
   patterns" is empty, and its help says: "When no list is configured, Backstage
   allows HTTP loopback redirects on localhost, 127.0.0.1, and [::1], using
   **any port and path**." That is the answer to the `redirect_uri` mismatch
   raised as question (e) in the vendor draft. `mcp-remote` uses
   `http://127.0.0.1:<derived-port>/oauth/callback`, which is loopback, and the
   default allows **any port and path**. So the strict-comparison worry is
   resolved: the derived port does not need to be pre-registered.

   This means question (e) can likely be dropped from the vendor ask rather than
   sent. Recorded as a strong reading of the documented default, not as a
   verified successful authorization.

### Also observed
- `Catalog Settings -> Providers`: **"GitHub Organizational Data — Not
  configured"**, with an `Ingest` button. Corroborates the checkpoint. The
  account is a personal user account, so this provider is not applicable anyway.
- The global `/catalog` reads **`No results.`** as admin. The empty catalog is
  therefore global, not workspace-scoped, which independently supports the
  "catalog source is unfetchable" root cause rather than a workspace wiring
  problem.
- App Settings `Name` is `kooshapari.com`, `Title` is `Spotify Portal`.
- 11 of 12 MCP plugin action sources are enabled; "Data Experience" and
  "Semantic" are "Currently configured, but not available in this Portal
  instance".

### What I did NOT do
I did not modify any setting. Adding a fifth client ID pattern is a change to
authentication policy that permits a new client to authenticate against this
Portal, which is exactly the kind of permission change the task constraints place
outside automatic action. The finding is recorded instead so the edit can be made
deliberately, with the append-never-replace warning attached.

The correct next edit, when authorized, is to add exactly one pattern for our own
domain — `https://kooshapari.com/oauth/client-metadata.json` — using the CIMD
template already committed at
`docs/sessions/2026-10-15-portal-xirp-resume/portal-cimd.client-metadata.template.json`,
with the redirect port derived by `scripts/derive_callback_port.py` for that exact
URL (18586).

## Appendix E. CIMD hosting analysis

## CIMD hosting: where the document can actually be served (2026-10-15)

Tested candidate hosts before recommending the append. Two of them look right and
are not, which is worth writing down so nobody repeats the check.

### Both obvious domain choices fail

| Host | Probe | Result |
|---|---|---|
| `https://kooshapari.com/oauth/client-metadata.json` | anonymous GET | **404** |
| `https://jcode.sh/oauth/client-metadata.json` | anonymous GET | **200 — but `text/html`, 98,237 bytes** |
| `https://jcode.sh/definitely-not-real-xyz` (control) | anonymous GET | **200, `text/html`, 98,425 bytes** |

`jcode.sh` is a Cloudflare-hosted SPA (`server: cloudflare`) that returns the
HTML shell with **HTTP 200 for every path**. So the 200 on the CIMD path is the
identical SPA-fallback trap already found on Portal's
`/.well-known/oauth-client-metadata.json`. **Content-type must be checked; a 200
proves nothing on an SPA host.** Neither `kooshapari.com` nor `jcode.sh` can
serve the document as it stands.

### raw.githubusercontent is the viable route, with one caveat

`KooshaPari/Jcode` is `private: false, visibility: public, fork: true`, and
anonymous raw fetches work — e.g. `README.md` and `Cargo.toml` both return 200.
So a CIMD document committed to that repo would be anonymously fetchable.

**Caveat, stated honestly:** raw serves JSON as `content-type: text/plain`.
Verified against a real file (`fork-prompt-session-rich-summary.json` -> 200,
`text/plain`). Whether Spotify's auth backend accepts a client-metadata document
served as `text/plain` is **not established**, because the only allow-listed
document that could serve as a comparison point, Xirp's, is served as
`application/json` from a normal web host. If the fetcher is strict, raw is the
wrong host and a small static host or Cloudflare Worker returning
`application/json` would be required.

Two of my own earlier 404s during this check were my error, not a property of
raw: I probed `package.json` and `biome.json`, neither of which exists at Jcode
master root. Corrected by listing the directory first.

### Net hosting recommendation
1. **Preferred:** a static host that serves `application/json` at a stable HTTPS
   path — a small Cloudflare Worker is the natural fit given the account already
   runs one for `jcode.sh`.
2. **Fallback:** a file committed to a public repo and served via
   `raw.githubusercontent.com`, accepting the `text/plain` risk and testing it
   against the real authorization endpoint before relying on it.

Either way, the allow-list pattern appended in App Settings must match the URL
actually used, and the redirect port must be re-derived by
`scripts/derive_callback_port.py` for that exact URL. The port is 18586 for
`https://kooshapari.com/oauth/client-metadata.json` and **35683** for
`https://jcode.sh/oauth/client-metadata.json`, so it changes with the choice.

### Bridge state during this work
The Firefox bridge flapped again and stopped responding, and `browser setup
firefox` could not complete — it needs a manual "Add" click in the extension
prompt. This is the same flap already documented in STATE.md section 13. It did
not block this work, because everything above was done with `curl`.

## CIMD hosting: the deployment is now scoped (2026-10-15)

Verified against the Cloudflare API and by probe, so the operator's step is a
single known action rather than a design decision.

### `kooshapari.com` is a Cloudflare zone in your own account

```
/zones -> kooshapari.com (active, 6c9edab581e9c7b8fdb6a83adc6878ea)
         pheno.shop, pheno.studio, phenotype.space
whoami -> user:2a8cf34986db70be316acea959d9b9b9b9, kooshapari@gmail.com
```

That is the decisive fact: the domain named in my recommended pattern is one you
control in Cloudflare, so a Worker route on it is a first-party operation.

### Today `kooshapari.com` is served by Vercel, and the path 404s honestly

| Path | Status | Content-Type |
|---|---|---|
| `/` | 200 | text/html |
| `/index.html` | 200 | text/html |
| `/.well-known/oauth-client-metadata.json` | 404 | text/plain |
| `/oauth/client-metadata.json` | 404 | text/plain |

`server: Vercel`. Note this is the **good** failure mode: Vercel returns a real
`404 text/plain` for unknown paths rather than an HTML fallback with a 200. So
`kooshapari.com` is a trustworthy host for the pattern — the 404 today simply
means nothing is deployed at that path yet.

### No existing Worker can be reused

Enumerated all 12 Workers: `agent-digest`, `dispensary-collector`,
`job-collector`, `job-collector-verification-preview`, `phenotype-apex-landing`,
`phenotype-pages-proxy`, `relay-forwarder`, `shop-redirect`, `studio-redirect`,
`tracera-edge`, `tracera-edge-dev`, `tracera-edge-preview`. **Every one has
`routes: []`**, so none owns a custom domain, and none is a fit — they do email,
KV/D1/R2 collection, redirects, and a tracera API. A dedicated Worker is
required.

Also checked Pages: only `job-cockpit` (dash.kooshapari.com) and
`harnessdesk-preview` exist. `jcode.sh` is in **none** of this account's zones,
which is consistent with it being an external/Vercel-hosted domain and is why it
was not an option.

### Recommended deployment, exactly

A new Worker with a route scoped to **only** the CIMD path, so it cannot affect
any other path on the domain:

- Worker name: `jcode-cimd`
- Route: `kooshapari.com/oauth/client-metadata.json` (exact path, not a wildcard)
- Response: `content-type: application/json`, `cache-control` short, CORS not
  required (server-to-server fetch)
- Body: the committed template with `client_id` set to that same URL,
  `client_name` set to our own identity, and `redirect_uris` containing
  `http://127.0.0.1:18586/oauth/callback`

**Order matters.** Deploy and verify the URL returns real JSON *first*, then
append the allow-list pattern in App Settings. Adding the pattern before the
document is served would allow a client_id that cannot be resolved.

### The exact port, once more
`18586` for `https://kooshapari.com/oauth/client-metadata.json`, derived by
`scripts/derive_callback_port.py` and cross-checked against the shipped
`mcp-remote` JavaScript. Recompute if the URL changes.

### The Worker config is validated, not just written

Two independent checks, neither of which deploys anything:

**`wrangler deploy --dry-run`** against a real `wrangler.toml` (name `jcode-cimd`,
`main = index.js`, `compatibility_date = 2026-10-15`, and the single route
`kooshapari.com/oauth/client-metadata.json` on `zone_name = kooshapari.com`):

```
Total Upload: 1.52 KiB / gzip: 0.81 KiB
No bindings found.
--dry-run: exiting now.
```

So the bundle builds and the config parses. Note the dry-run does **not** contact
Cloudflare, so it cannot prove the route pattern is accepted.

**Cloudflare API, zone `6c9edab581e9c7b8fdb6a83adc6878ea`:**
- `zoneStatus: "active"`, `zoneName: "kooshapari.com"`
- `GET /zones/{id}/workers/routes` -> **`routeCount: 0`**

That zero is worth stating precisely, for two reasons. First, **no route
occupies our path**, so the deploy cannot collide with anything. Second, it
independently corroborates the `curl` probe: nothing is serving
`/oauth/client-metadata.json` on this zone today, which is why that path 404s
rather than falling back.

Together these mean the deploy step is a known-good configuration meeting an
empty target. What remains genuinely unproven is only whether Spotify's auth
backend accepts the resulting document — which is exactly what the allow-list
append and a test authorization would establish.

### One gap in the dry-run worth being honest about
`wrangler deploy --dry-run` validates the bundle and the TOML, but it does not
call the Cloudflare API, so it cannot catch a malformed route pattern. The API
call above confirms the zone is active but does not itself validate the pattern
string. If the deploy fails on the route, the fallback is to drop `routes` from
wrangler.toml, deploy the Worker on its default `*.workers.dev` URL, verify the
document there, and only then attach the custom domain. That fallback changes the
client_id URL, which changes the derived port, so it is a last resort rather than
an equal option.
