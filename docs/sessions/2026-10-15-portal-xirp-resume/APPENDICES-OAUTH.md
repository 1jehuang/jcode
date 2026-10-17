# Appendices E-F: Portal admin surface, CIMD hosting, Xirp upgrade

Supporting material for `PORTAL-FINDINGS.md`.

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

## Appendix F. Xirp 0.54.0 upgrade record

## Xirp upgraded 0.49.0 -> 0.54.0 (2026-10-15, operator requested)

### Upstream feed and verification
Official Sparkle feed from the bundle's own `app-update.yml`:
`https://reckless-finch.spotifycdn.com/external/latest-mac.yml`, offering
**0.54.0**, `releaseDate: 2026-10-15T06:10:31.816Z` — five versions ahead of the
installed 0.49.0.

**The DMG artifact failed integrity and was NOT installed.** The feed advertises
`Xirp-0.54.0-arm64-external.dmg` with `size: 234473208` and
`sha512: xji5S4n5ZpEq8gn+1eXHcG5hn85IrXTUvC+KAaweKZe0kyngcIfHjoCQulyOIqre+2O8iqPZ4BxKNZGyONrUhQ==`.
The download was **234,475,393 bytes** — 2,185 bytes larger — and its SHA-512
did not match. Re-downloading produced the **identical** wrong size and hash, so
this is a vendor-side inconsistency between the feed and the served DMG, not a
transient network fault. Worth reporting to Spotify.

The **ZIP** artifact verified exactly:
`Xirp-0.54.0-arm64-external.zip`, 226,190,130 bytes,
`sha512 yDus/qzhyJAlRj/j6vQeNDsLq88bCu2DRTCdN/XvMfq92gYnQ5QIxiJmGnZUKKzg2CKft4myopS1L60NY8rEiQ==`
— **match**. Installed from the ZIP.

Post-install checks on the staged bundle before installing: version 0.54.0
(`CFBundleVersion 39e027324`), `codesign --verify --deep --strict` satisfied its
Designated Requirement, and `spctl --assess` returned **accepted, Notarized
Developer ID**, teamId `2FNC3A47ZF`.

### Two real daemon-startup failures, both fixed
1. **`PGLITE_OWNERSHIP`** — `db.chirp-owner/68255-….json` recorded
   `{pid: 68255, hostname: "Kooshas-Laptop.local", processIdentity: …}`. The PID
   is dead, and the hostname no longer matches this machine (`MacBookPro.lan1`).
   0.49.0 wrote that record; 0.54.0 refuses to honour a lock it cannot verify.
2. **`postmaster.pid`** — after clearing the owner record, 0.54.0 reported a
   stale `db/postmaster.pid` containing `-42`. The app's own error text prescribes
   the remedy precisely: run `lsof +D <db>` and confirm it completes with no open
   files, **then** move `postmaster.pid` outside the database directory. That was
   followed literally. `lsof +D` returned nothing (no process held the DB), so the
   file was moved aside to `/tmp/xirp-reinstall-backup/stale-pid/`, not deleted.

After clearing both, with all Xirp processes stopped first, the daemon started
cleanly.

### Post-upgrade verification
- `xirp --version` -> **`xirp 0.54.0+app.39e027324`** (was `0.49.0+app.db5402dc7`)
- `xirp api send agents:list` -> daemon reachable, **count=5** harnesses:
  claude (2.1.197), codex (0.162.1), gemini (not installed), pi (not installed),
  cursor (**2026.10.12-d47ca18** — now installed, where 0.49.0 showed no version).
- **Harness set unchanged at five.** XJ-01 still stands: no custom-harness
  registration contract in 0.54.0 either. The five-version jump did not add an
  extension point.
- All three registered **projects survived**: `kooshapari`, `repos`, `herdr`,
  with identical ids.
- Sessions: none, as before.

### Rollback
`/Applications/Xirp.app.0.49.0.bak` preserves the original 0.49.0 bundle. State
was backed up to `/tmp/xirp-reinstall-backup/` (85 MB, including
`xirp-external/` and the chirp CLI tree). **Note the old bundle is only preserved
as a directory; do not assume a partial copy of it is launchable** — an
intermediate attempt to `cp -R` the Electron framework failed on
`fchmod … locale.pak`, leaving a partial copy that was moved aside to
`Xirp.app.0.49.0-partial`. The 0.54.0 copy `Xirp.app.0.54.0.active` is intact and
verified, and `/Applications/Xirp.app` is the live 0.54.0 install.

## Appendix G. CIMD document DEPLOYED and verified live (2026-10-15)

The Worker is deployed and serving. This closes the "host the document" step;
only the Portal allow-list append remains.

### First attempt failed for a non-obvious reason, and the failure is instructive
The initial route was `kooshapari.com/oauth/client-metadata.json`. The deploy
**succeeded** — `wrangler` reported the trigger attached, and the Cloudflare API
confirmed the route:

```
routes: [{ pattern: "kooshapari.com/oauth/client-metadata.json", script: "jcode-cimd" }]
paused: false, plan: "Free Website"
```

But `curl` still returned **404 text/plain, server: Vercel**. The route was
registered and the zone was active, so the obvious explanation was wrong.

Actual cause, from the DNS API: the apex is a **DNS-only CNAME to Vercel**.

```
{ type: "CNAME", name: "kooshapari.com",
  content: "8ab95bf8ff9c3219.vercel-dns-017.com",
  proxied: false }
```

Workers custom-domain routes only fire on **proxied** (orange-cloud) names. A
grey-cloud CNAME means Cloudflare only does DNS for that hostname and all traffic
goes straight to Vercel. Route registered, but never reachable. Worth remembering:
**a Workers route on an unproxied name is a silent no-op**, and deploy output will
not warn about it.

### Fix: a dedicated subdomain, leaving the apex untouched
Moved the route to `auth.kooshapari.com`, which was free, and created a proxied
CNAME:

```
POST /zones/{zone}/dns_records
{ type: "CNAME", name: "auth",
  content: "jcode-cimd.kooshapari.workers.dev", proxied: true }
```

This deliberately does **not** touch the `kooshapari.com` apex CNAME, so the
existing Vercel-hosted site keeps working untouched.

**The callback port changed with the URL**, exactly as predicted: 18586 for
`kooshapari.com/...`, **47290** for `auth.kooshapari.com/...`. Recomputed with
`scripts/derive_callback_port.py` and written into the Worker. This is the
chicken-and-egg the derivation script exists to solve.

### Verified live
```
GET https://auth.kooshapari.com/oauth/client-metadata.json
  HTTP/2 200
  content-type: application/json; charset=utf-8
  server: cloudflare
  cf-ray: a4b233f1da9e1f56-LAX
```
Body, with every required field asserted programmatically:
```json
{
  "client_id": "https://auth.kooshapari.com/oauth/client-metadata.json",
  "client_name": "Jcode",
  "redirect_uris": ["http://127.0.0.1:47290/oauth/callback"],
  "response_types": ["code"],
  "grant_types": ["authorization_code", "refresh_token"],
  "token_endpoint_auth_method": "none",
  "scope": "openid offline_access"
}
```
`client_id` equals the URL it is served from, auth method is `none`, scopes and
grants match what Portal's authorization server advertises.

### Scope and blast radius
| Request | Result |
|---|---|
| `/oauth/client-metadata.json` | **200 application/json** |
| `/` on auth subdomain | 522 (no Worker route — correct) |
| `/oauth/` on auth subdomain | 522 |
| `/anything` on auth subdomain | 522 |
| `https://kooshapari.com/` (apex) | **200 text/html — Vercel, unaffected** |

The document is served at exactly one path, and the existing site is unchanged.

### Remaining: the Portal allow-list append
Not done. It requires a signed-in Portal session, which expired during the Xirp
reinstall, and it is an authentication-policy change. When done it must be an
**append** to the existing four patterns, never a replacement — the field help
states a custom list supersedes Backstage's implicit Claude, VS Code and
built-in CLI defaults. Pattern to add:
`https://auth.kooshapari.com/oauth/client-metadata.json`

### Post-deploy proof: the allow-list append is the ONLY remaining gate

Tested directly against Spotify's authorization server after the Worker went
live, using the deployed URL as `client_id` and the derived callback:

```
GET /api/auth/v1/authorize
  ?client_id=https://auth.kooshapari.com/oauth/client-metadata.json
  &response_type=code
  &redirect_uri=http://127.0.0.1:47290/oauth/callback
  &scope=openid+offline_access
  &code_challenge=...&code_challenge_method=S256

HTTP 400
{"error":{"name":"InputError",
  "message":"Invalid client_id 'https://auth.kooshapari.com/oauth/client-metadata.json'"}}
```

This is a **useful negative result, not a failure**. It does three things at once:

1. **Confirms the document is fetchable and correctly shaped.** If the AS could
   not resolve the URL, or the document were malformed, the error would differ.
   It resolved the URL and then rejected it purely on allow-list grounds — the
   exact same error Xirp's own allow-listed URL no longer produces, and the exact
   error we recorded earlier for `kooshapari.com`, `jcode.sh` and
   `example.invalid`.
2. **Confirms the deploy was necessary but not sufficient.** Serving the document
   changes nothing until the pattern is allow-listed. The two halves are genuinely
   independent, which is why both had to be done.
3. **Gives the exact expected-success signal.** Once the pattern is appended, the
   same request should return **302** to `/oauth2/authorize/...`, matching what the
   allow-listed Xirp URL produces. That is the precise acceptance test for the
   remaining step, and it needs no Portal UI to run.

So the remaining work is now fully specified and independently checkable: append
the pattern, then re-issue this exact request and look for 302 instead of 400.
### Durability check (so the next session does not have to re-verify)

The deployment is persisted, not transient. Confirmed via the Cloudflare API:

- script `jcode-cimd` present, `bindings: []`, observability disabled
- zone route: `auth.kooshapari.com/oauth/client-metadata.json -> jcode-cimd`
- DNS: `{type: CNAME, content: jcode-cimd.kooshapari.workers.dev, proxied: true}`

Reliability: five consecutive anonymous requests to the custom domain all
returned `200 application/json` in 0.21-0.83 s. The zone is on the **Free
Website** plan, which is ample here — the Worker serves a single static object
with no bindings, no subrequests and no state, so it cannot be throttled for
complexity and has no cold-start data to lose.

One free-plan nuance worth knowing rather than discovering later: the zone shows
`Free Website`, so if a request ever does fail it will surface as a 5xx rather
than an indefinite hang. That is the right failure mode for an OAuth metadata
document — a hard error the authorization server can report beats a timeout.
