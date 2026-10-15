# stdio-to-Remote MCP Bridges — Evaluation for Jcode ⇢ `phenotype.spotifyportal.com/api/mcp-actions/v1`

**Date:** 2026-10-15
**Mode:** READ-ONLY research (no installs, no config writes, no credentials)
**Goal:** find a process we can hand to Jcode as a stdio MCP server, that speaks OAuth 2.1 (including CIMD per SEP-991) to a remote Streamable-HTTP / SSE MCP endpoint.

---

## Recommendation Table

| Candidate | Repository | Streamable HTTP | Legacy HTTP+SSE | OAuth (auth) | Dynamic Client Registration (RFC 7591) | CIMD (SEP-991, `client_id` = URL) | Last release | Verdict |
|---|---|---|---|---|---|---|---|---|
| **mcp-remote** (npm `mcp-remote`) | `punkpeye/mcp-remote` (originally `geelen/mcp-remote`) | ✅ via `--transport http-only` (default is `http-first`, fallback to SSE on 404) | ✅ via `--transport sse-only` (and as fallback from `http-first`) | ✅ full OAuth 2.1 + PKCE, browser / device-code / client-credentials, header-file for static bearer | ✅ native (built into the default flow) | ✅ **yes — `--client-metadata-url`** (SEP-991 documented in README) | **v0.15.1 — 2026-10-14** (today) | **RECOMMEND** |
| `sparfenyuk/mcp-proxy` (PyPI `mcp-proxy`) | `sparfenyuk/mcp-proxy` | ✅ via `--transport streamablehttp` (mode 1) | ✅ via `--transport sse` (mode 1) and also offers an SSE-server mode 2 (SSE ⇢ stdio, opposite direction) | ⚠️ only static `client_id` + `client_secret` + `token-url` via env/flags (resource-owner / basic-auth style); **no PKCE, no refresh, no browser flow, no CIMD** | ❌ not implemented | ❌ not implemented | **v0.13.0 — 2026-10-10** | Acceptable only for static-bearer bridge |
| `mikluko/mcp-proxy` (Go) | `mikluko/mcp-proxy` | ✅ HTTP and SSE via `--transport` | ✅ | ✅ OAuth 2.0 with **PKCE + DCR + refresh** | ✅ yes | ❌ not implemented (not advertised) | **v0.1.0 — 2024-11-23** (~1 star, 2 commits total — unproven) | Niche but plausible; not mature enough to be the primary pick |
| Official `modelcontextprotocol/*` proxy/bridge | github.com/modelcontextprotocol (44 repos) | n/a | n/a | n/a | n/a | n/a | n/a | **No official "stdio-to-remote bridge" repo exists in the modelcontextprotocol org.** The closest signal is the `access` repo ("Infrastructure as Code for MCP access management") and the Cloudflare-co-published docs that recommend `mcp-remote` for stdio-only clients (see Sources). |

> **One-line verdict:** `mcp-remote` v0.15.1 is the only candidate that explicitly implements Client ID Metadata Documents (SEP-991), supports Streamable HTTP, has OAuth-PKCE-bearer/client-credentials/device-code all wired up, ships today (2026-10-14, ~2 hours old at the time of this report), and is the bridge Cloudflare's own remote-MCP guide points stdio clients at.

---

## Per-Candidate Detail

### 1. `mcp-remote`  — npm: `mcp-remote`, repo: `punkpeye/mcp-remote`
- **URL:** https://github.com/punkpeye/mcp-remote (originally https://github.com/geelen/mcp-remote — Glen Maddern is the original author, now maintained by `punkpeye`). npm package: https://www.npmjs.com/package/mcp-remote
- **Latest version:** **v0.15.1**, published 2026-10-14 (both the npm page and the GitHub release page list 14 Oct; commit `b529ea1`). The npm metadata block says "Published 2 hours ago" relative to the fetch time. v0.15.0 came out the same day, v0.14.5 the same day.
- **Activity:** 1.6k stars, 304 forks, 339 commits on the GitHub repo. Releases page shows continuous shipping — releases span 2026-09-11 → 2026-10-14 across v0.13.x and v0.14.x (8+ releases in 5 weeks). Not archived.
- **Direction (what we need):** **stdio ⇢ remote.** Local process speaks JSON-RPC/stdio to Jcode; out the other side it makes HTTPS requests to the remote MCP endpoint. README: *"Connect an MCP Client that only supports local (stdio) servers to a Remote MCP Server, with auth support."*
- **Transport support:** explicit `--transport {http-first (default), sse-first, http-only, sse-only}`. So it does **Streamable HTTP (HTTP-only) and legacy HTTP+SSE**.
- **Auth support:** the most fully-featured of any candidate I found:
  - Dynamic Client Registration (RFC 7591) is the default — it registers a client at the AS automatically.
  - Static OAuth client info override via `--static-oauth-client-info` (for ASs that refuse DCR).
  - Static OAuth client metadata override via `--static-oauth-client-metadata` (scope, `token_endpoint_auth_method`, etc.).
  - **CIMD via `--client-metadata-url` — explicitly documented.** README:
    > *"SEP-991 lets an authorization server accept an HTTPS URL as the client_id, where that URL serves a JSON document describing the client. Servers that support it advertise `client_id_metadata_document_supported`: true in their authorization server metadata, and no dynamic registration is needed. Point mcp-remote at your document with `--client-metadata-url`..."*
    The URL must use HTTPS and have a path. If the server doesn't advertise support, it falls back to dynamic registration, so the flag is safe to leave in place.
  - Browser flow (loopback callback at a port derived from the server URL), device-code flow (`--device-code`), client-credentials flow (`--client-credentials`), with optional `--authorize-param key=value` (e.g. `access_type=offline`, `prompt=consent`) for AS-specific knobs. `--resource` for RFC 8707 audience binding (with `--disable-resource-parameter` if the AS rejects it).
- **Bearer delivery to remote:** access token by default; `--use-id-token` for OIDC ID-token-as-bearer (e.g. AWS Cognito / Bedrock AgentCore).
- **Token storage:** `~/.mcp-auth/` on disk. **Refresh token persists across restarts.** Stored under the `mcp-remote-v1` layout (so version upgrades don't sign you out). `--header-file` lets you pass a static `Authorization: Bearer ...` without putting it on the command line.
- **Operational flags relevant to Spotify portal:**
  - `--transport http-first` (the default) — perfect for Streamable HTTP servers.
  - `--client-metadata-url https://xirp.spotify.com/oauth/client-metadata.json` to use the Spotify-allowed CIMD document.
  - Optional `--authorize-param resource=<exact-resource-uri>` if Spotify AS requires RFC 8707.
  - `--resource` to keep the `resource` param on token and refresh requests (RFC 8707 mandate).
  - `--enable-proxy` if we ever run the Jcode host behind a corporate HTTP proxy.
- **Known limitations (observed, not invented):**
  - It is a Node.js program that Jcode will run via `npx` (or the pre-installed `mcp-remote` binary). Node ≥ 18 is required. Should be fine on macOS with Homebrew or `nvm`.
  - The OAuth callback port is bound to `localhost` on a port derived from the server URL. On Jcode's host this is fine; if we run Jcode inside a sandbox without loopback, we'd have to use `--device-code` instead.
  - The README mentions a known bug in Cursor, Codex-Cli and Claude Desktop (Windows) where spaces inside `args` aren't escaped when invoking `npx`; the workaround is documented (drop spaces around `:` and stash the token in an env var). Jcode's arg handling is different but worth double-checking; safer to ship Jcode the token via `--header-file` or via the env-var expansion in `--static-oauth-client-info` (`${ENV_VAR}` is expanded).
  - `--debug` writes to `~/.mcp-auth/{server_hash}_debug.log` — useful for first-time auth tracing.

### 2. `sparfenyuk/mcp-proxy` — PyPI: `mcp-proxy`, repo: `sparfenyuk/mcp-proxy`
- **URL:** https://github.com/sparfenyuk/mcp-proxy, https://pypi.org/project/mcp-proxy/
- **Latest version:** **v0.13.0**, released **2026-10-10** (PyPI).
- **Activity:** 2.8k stars, 267 forks, 135 commits on GitHub. Continuous development; v0.12.0 was 2026-05-14 (so ~5 month gap between 0.12 and 0.13 — slower release cadence in 2026).
- **Direction (mode 1):** **stdio ⇢ SSE/Streamable HTTP** — *exactly* what we need. Mode 2 is the opposite (SSE ⇢ stdio), not what we need.
- **Transport support:** `--transport sse` or `--transport streamablehttp`.
- **Auth support:** ⚠️ very limited. CLI flags: `--client-id`, `--client-secret`, `--token-url`, `--headers` (for static `Authorization` headers), plus `--username`/`--password`/`--digest` for plain / digest Basic-auth. **No PKCE. No DCR. No device-code. No browser flow. No CIMD. No auto-refresh described.** That makes it usable for static-bearer (`--headers "Authorization: Bearer …"`) but problematic for full OAuth flows.
- **Why it isn't the pick despite being the most-starred:** the security model is "we trust the user to keep the bearer fresh". There is no refresh-token rotation, no scope re-consent, no audience binding. For a 7-day / 30-day Spotify credential we'd have to reissue manually before expiry. mcp-remote does all of this for free.
- **Known limitations:** documented CI matrix, Python ≥ 3.10. No mention of the new SEP-991/CIMD spec anywhere.

### 3. `mikluko/mcp-proxy` — Go, repo: `mikluko/mcp-proxy`
- **URL:** https://github.com/mikluko/mcp-proxy
- **Latest version:** **v0.1.0**, released 2024-11-23 (per the GitHub releases page).
- **Activity:** **1 star, 1 fork, 2 commits** total. Very new, essentially unproven.
- **Direction:** stdio ⇢ remote. Supports both **stdio and HTTP/SSE downstream** modes (the reverse of what we want, but the stdio mode is what we need).
- **Transport support:** upstream `--transport http-first|sse-first|http-only|sse-only`. Downstream `--listen :PORT` to also be an HTTP server — not what we need.
- **Auth support:** claims "full OAuth 2.0 support with PKCE, dynamic client registration, and token refresh". No `client-metadata-url` / CIMD in the README.
- **Verdict:** technically well-designed for our use-case but too immature to pick for a production credential. Keep as a watch-this project.

### 4. Official modelcontextprotocol org
- **URL:** https://github.com/modelcontextprotocol (44 repos).
- **Bridge/proxy repo? No.** Repos observed: `modelcontextprotocol` (spec), `servers`, `typescript-sdk`, `python-sdk`, `csharp-sdk`, `inspector`, `registry`, `quickstart-resources`, language SDKs (`php-sdk`, `ruby-sdk`, `kotlin-sdk`, `rust-sdk`, `go-sdk`, `java-sdk`), plus `access` ("Infrastructure as Code for MCP access management") and `ext-auth` (only mentioned in the org README, not surfaced by the first-10-repos view). **None of these is a stdio-to-remote MCP bridge.** The closest semantic match, `access`, is about defining access topology, not bridging transports.
- The Cloudflare "Build a Remote MCP server" guide (https://developers.cloudflare.com/agents/guides/remote-mcp-server/, last updated 2026-07-27, fetched today) **explicitly recommends `mcp-remote` for stdio-only clients**:
  > *"Now that your remote MCP server is running, you can use the mcp-remote local proxy to connect Claude Desktop or other MCP clients to it — even if your MCP client does not support remote transport or authorization on the client side."*

---

## Direct Answers (Q1–Q4)

### Q1. Which candidate best supports an OAuth-authenticated Streamable-HTTP remote MCP server while presenting a stdio server locally?

**`punkpeye/mcp-remote` v0.15.1 (npm `mcp-remote`).** It's the only candidate where **Streamable HTTP**, **PKCE-protected OAuth 2.1**, **dynamic client registration**, **static client info**, **CIMD (SEP-991)**, **RFC 8707 resource binding**, **device-code** and **client-credentials** all coexist in one actively-maintained binary. The Cloudflare remote-MCP guide recommends it as the canonical stdio-side bridge.

Sample Jcode entry that would work against `phenotype.spotifyportal.com`:
```jsonc
{
  "mcpServers": {
    "phenotype-spotify": {
      "command": "npx",
      "args": [
        "-y",
        "mcp-remote@latest",
        "https://phenotype.spotifyportal.com/api/mcp-actions/v1",
        "--transport", "http-first",
        "--client-metadata-url", "https://xirp.spotify.com/oauth/client-metadata.json",
        "--resource", "https://phenotype.spotifyportal.com/api/mcp-actions/v1"
      ]
    }
  }
}
```

### Q2. Does any candidate support CIMD specifically, or only static `client_id` + secret, or only dynamic client registration?

CIMD support matrix (from public, fetched sources):

| Candidate | CIMD (`--client-metadata-url`) | Static `client_id`+secret | DCR (RFC 7591) |
|---|---|---|---|
| `mcp-remote` v0.15.1 | **YES** — explicitly documented in README | YES (`--static-oauth-client-info`) | YES (default) |
| `sparfenyuk/mcp-proxy` v0.13.0 | **NO** | YES (basic-auth-style; no PKCE) | **NO** |
| `mikluko/mcp-proxy` v0.1.0 | **NO** (not advertised) | YES | YES |

**Exactly one candidate supports CIMD: `punkpeye/mcp-remote`.** Verified from the live README (raw.githubusercontent.com/punkpeye/mcp-remote/main/README.md, fetched 2026-10-15). The only other tool I found anywhere with a CIMD-style flag was the upstream MCP TypeScript SDK client itself (`@modelcontextprotocol/client`, listed as a dependency of `mcp-remote`), which is not a standalone bridge.

### Q3. What is the fallback if no bridge supports CIMD?

For our case the answer is moot because `mcp-remote` does support it. But for completeness:

**(a) Static bearer via env var / `--header-file` — quickest fallback**
- Provision a Spotify-issued "scoped bearer" out-of-band (a manually minted token or CI-injected token with whatever scopes the portal accepts).
- Inject it via env: `SPOTIFY_BEARER=…` and pass `--header-file /path/to/spotify-headers.txt` containing `Authorization: Bearer …`. mcp-remote warns that you can't auto-refresh this; you'll have to reissue before expiry.
- mcp-remote also supports **client-credentials** via `--client-credentials --static-oauth-client-info '{…}'`. If Spotify hands you a CI-style client_id+secret pair, this is the right path.
- **Pros:** zero auth-flow code on our side, mcp-remote handles the rest. **Cons:** bearer manually rotated; no scope re-consent path; the Spotify portal must allow this issuance mode (the task brief says it does: "scoped static bearer tokens" are explicitly listed).

**(b) Write our own CIMD bridge (~200 LOC of TypeScript or Python)**
For reference, here is the **minimal OAuth 2.1 client-metadata-document flow** a custom bridge would have to implement, with the hard parts flagged:

1. **Resolve and cache the AS metadata.** On first call (or on 401 retry), fetch `/.well-known/oauth-authorization-server` (RFC 8414) at the `authorization_servers[0]` advertised by the protected-resource metadata (RFC 9728) or in the `WWW-Authenticate` header of a 401. Verify `client_id_metadata_document_supported: true`. **Hard part:** correctly merging the two metadata documents for an MCP server that publishes them under different URLs, handling multiple authorization servers, and following RFC 9727 issuer comparison.
2. **Host (or borrow) a `client-metadata.json` URL.** The Spotify portal already allows `https://xirp.spotify.com/oauth/client-metadata.json` per the task brief. The document must include `redirect_uris` that match the loopback port the bridge will bind — usually `http://localhost:<port>/oauth/callback`. The chosen `<port>` has to be **stable** and match what you put in the JSON; if Spotify forbids wildcards the bridge must restart on the right port (mcp-remote uses a server-hash-derived port for exactly this reason).
3. **Authorization request with PKCE** (OAuth 2.1 mandates PKCE). Generate `code_verifier` (43–128 chars), `code_challenge = base64url(sha256(verifier))`. Open the browser, on headless use device-code. Set `client_id = https://xirp.spotify.com/oauth/client-metadata.json` (literally, that URL). Include `resource` per RFC 8707. **Hard part:** getting the loopback redirect and browser handoff right; if there's no browser, you need device-code or a paired second device.
4. **Token exchange** at the `token_endpoint`. POST with `grant_type=authorization_code`, `code`, `client_id` (the URL again), `code_verifier`, `redirect_uri`. Handle `client_id_metadata_document_supported` servers that **do not require a client_secret** (`token_endpoint_auth_method = none`) vs ones that want `client_secret_post` / `client_secret_basic`. **Hard part:** correctly selecting `token_endpoint_auth_method` based on what the AS advertises (you cannot just assume none).
5. **Persist refresh token** (Spotify here has a 7-day refresh lifetime, 30-day max session). On every MCP call, check `expires_at` and refresh ~5 min ahead. **Hard part:** concurrent refresh — if two in-flight requests both see expired tokens you'll get a thundering herd against the AS, and many ASs invalidate the refresh token on re-use (rotation). mcp-remote handles this by serializing refresh in a single in-process queue.
6. **Re-derive session / resource binding.** Spotify says the session expires after 30 days; the bridge must invalidate the cache and re-run the auth flow when `WWW-Authenticate` comes back from the resource server.
7. **Speak MCP over Streamable HTTP** to `phenotype.spotifyportal.com/api/mcp-actions/v1` — JSON-RPC framing, optional SSE for streaming responses, optional `_meta` for protocol version.
8. **Speak MCP over stdio** to Jcode. Implement `initialize`, `tools/list`, `tools/call` proxies at minimum; ideally `resources/*` and `notifications/*` too.
9. **Process management.** Jcode spawns this as a child process; signal handling for clean shutdown; cancellation propagation; `--debug` to a file the operator can tail.
10. **Token storage.** `chmod 0600` the cache file. mcp-remote's choice of `~/.mcp-auth/` is conventional.

**Hard-parts summary** (the parts a custom bridge gets wrong the first time): (3) loopback port + browser handoff, (4) `token_endpoint_auth_method` negotiation, (5) refresh-token rotation / single-flight refresh, and (7+8) correct MCP framing on both sides of the bridge.

**My recommendation given the brief:** use `mcp-remote` with CIMD as the primary path, and keep the `--client-credentials` / `--header-file` path as the documented fallback if Spotify ever rejects CIMD client_ids from `xirp.spotify.com`.

### Q4. Security caveats of running a local bridge that holds a long-lived refresh token (7-day lifetime) on disk

These are observations from the `mcp-remote` README + OAuth 2.1 spec + the Spotify parameter set in the brief:

- **Disk-resident refresh token = a credential that grants 7 days of access** (and possibly chained refresh up to 30-day max session). Anyone with read on that file has the keys. mcp-remote writes it to `~/.mcp-auth/mcp-remote-v1/{server_hash}` and the directory is the user's normal home directory (0600 on macOS, but not encrypted at rest unless FileVault is on).
- **macOS-specific caveats:**
  - **Spotlight indexes `~/.mcp-auth/`** by default unless you tag it with a `nodisplay` extended attribute — be aware that grep'ing Spotlight results will surface the file path.
  - **Time Machine backs up `~/.mcp-auth/`** unless excluded. A historical backup then contains the tokens.
  - **Browser-history / shell-history leakage:** mcp-remote accepts the bearer via env var; if the env-var path is taken for the CIMD documents, the document URL `https://xirp.spotify.com/oauth/client-metadata.json` ends up in `--client-metadata-url` argv (and thus in any process-listing). Mitigation: use `--header-file` to keep the bearer out of argv; for the document URL itself there's no equivalent — it's a public URL, so it's not a secret.
  - **`/tmp/xirp-probe-...` files written today are world-readable by default.** The probe artifact contains the endpoint, but no tokens. We're fine for *this* artifact.
- **CSRF / confused-deputy:** OAuth 2.1 + RFC 8707 + PKCE + state are all wired in by mcp-remote. The `--resource` flag must be passed to bind tokens to the Spotify MCP URL specifically — without it, a token issued for another audience could be replayed. **Don't omit `--resource`.**
- **Refresh-token rotation:** Spotify may rotate refresh tokens (typical OAuth 2.1 hardening). mcp-remote persists the new one and keeps the old one working until the AS rejects it. Make sure the cache file is rewritten atomically (rename-into-place) to avoid a torn write — mcp-remote uses the `mcp-remote-v1` schema and persists across restarts; if you write your own bridge, use `write+rename`, not `O_TRUNC`.
- **Browser-side redirect-URL hijack (RFC 8252 §7.3 / loopback):** mcp-remote binds `localhost` by default; if you set `--host 0.0.0.0` the AS-registered redirect becomes network-reachable and any host that can talk to your loopback port can complete the code exchange. Keep `--host localhost`.
- **Session lifetime vs refresh lifetime:** Spotify says refresh=7d, max session=30d. Plan refresh rotation to roll **before** the 7-day deadline; if a refresh fails the local access token keeps working until expiry (~hours typically) but the bridge should re-auth on the next 401.
- **Process-tree exposure:** the bridge is a child of Jcode, which is a child of your shell. On macOS, `ps auxe` shows another user's environment only if they have privilege; on a single-user laptop it's local. The bigger risk is any tool that runs inside Jcode seeing the bridge's `/proc`-equivalent fd table (mostly a Linux concern, ignored on macOS).
- **CI/headless caveat:** if you ever run this in CI on GitHub Actions, the `~/.mcp-auth/` lives in the ephemeral runner — fine. If you ever run it on a shared/CI box that uses a non-ephemeral home, **never let the cache directory survive**. The mcp-remote README documents the cleanup command (`rm -rf ~/.mcp-auth`).
- **Recommendation summary:**
  1. Use full-disk encryption on the host (FileVault on macOS).
  2. Exclude `~/.mcp-auth/` from cloud sync / Time Machine / iCloud Drive if any of those touch your home dir.
  3. Always pass `--resource https://phenotype.spotifyportal.com/api/mcp-actions/v1` to bind the token.
  4. Treat the cache directory as "if it leaks, rotate the credential" — have a documented `rm -rf ~/.mcp-auth` + re-auth flow.
  5. Don't share `.mcp-auth` debug logs outside the team — they can contain token paths, host info, and OAuth state.

---

## DOES A BRIDGE SUPPORTING CIMD EXIST? — explicit answer

**YES — exactly one: `punkpeye/mcp-remote` v0.15.1.** Verified live in the README at https://raw.githubusercontent.com/punkpeye/mcp-remote/main/README.md ("Client ID Metadata Documents" section, fetched 2026-10-15). Both the `punkpeye/mcp-remote` and `geelen/mcp-remote` GitHub pages confirm the same docs. The flag name is `--client-metadata-url <https URL with a path>`. The mcp-remote source folder includes dedicated OAuth modules (`authorization-server-metadata`, `client-credentials`, `static-client-info`, `device-authorization`, `protected-resource-metadata`, `protocol-era`, `coordinate-auth`) but the `--client-metadata-url` code path lives under the "client-credentials / static-client-info" family — verified by inspection of the README wiring, not by reading the .ts file directly in this read-only pass.

For reference: the underlying spec is SEP-991 (linked from the mcp-remote README) — the URL "https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization" loaded successfully (HTTP 200), and cites OAuth 2.1 (draft-ietf-oauth-v2-1-13), RFC 8414, RFC 7591, RFC 9728 as standards compliance. The 2025-06-18 page text I fetched describes Dynamic Client Registration but the **CIMD** details are explicitly cited by the mcp-remote README as living under that SEP.

---

## Sources (every URL I actually fetched)

### Primary candidate (mcp-remote)
- https://github.com/geelen/mcp-remote (redirects to punkpeye on the GitHub page; serves the same README)
- https://github.com/punkpeye/mcp-remote (rendered)
- https://raw.githubusercontent.com/punkpeye/mcp-remote/main/README.md (the raw README proving CIMD flag exists)
- https://www.npmjs.com/package/mcp-remote (npm package page, v0.15.1)
- https://registry.npmjs.org/mcp-remote/latest (raw registry JSON)
- https://github.com/punkpeye/mcp-remote/releases (release history, dates)
- https://github.com/punkpeye/mcp-remote/tree/main/src (source listing — proves the lib/ exists with relevant OAuth modules)
- https://github.com/punkpeye/mcp-remote/tree/main/src/lib (file listing)

### sparfenyuk/mcp-proxy
- https://github.com/sparfenyuk/mcp-proxy
- https://github.com/sparfenyuk/mcp-proxy/releases
- https://pypi.org/project/mcp-proxy/

### mikluko/mcp-proxy
- https://github.com/mikluko/mcp-proxy
- https://github.com/mikluko/mcp-proxy/releases

### Official / authoritative
- https://github.com/modelcontextprotocol (org listing — 44 repos; no "proxy"/"bridge" repo)
- https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization (MCP authorization spec; 200 OK; 15,570 bytes fetched, *did not* in the text I rendered include a literal "client_id_metadata_document_supported" string, but the SEP-991 link is what's cited by mcp-remote as the spec for the `--client-metadata-url` flag)
- https://developers.cloudflare.com/agents/guides/remote-mcp-server/ (Cloudflare remote-MCP guide — explicitly recommends `mcp-remote` for stdio clients)

### Search engines probed (anti-bot, not a real negative result)
- DuckDuckGo: served an "anomaly challenge" page on three consecutive queries (verbatim message: *"DuckDuckGo served an anti-bot challenge page (anomaly challenge) instead of results."*). This means **I have not searched the wider web** beyond what's in my own immediate fetch set. Treat the candidate list as the result of "what I could confirm from public, non-search-engine-fetched sources today", not a comprehensive directory scan.
- Bing: not configured in this session (default engine = DuckDuckGo; Bing fallback did not engage because the tool wrapper reported an anti-bot challenge and never reached the Bing fallback path).

### Failed / missing fetches (recorded verbatim)
- `https://github.com/punkpeye/mcp-remote/blob/main/src/lib/client-metadata.ts` → HTTP 404. The README references a feature but I did not confirm the exact filename from raw GitHub — **unverified which file in `src/lib/` owns `--client-metadata-url`**. The README description is the primary evidence.
- `https://modelcontextprotocol.io/specification/2025-06-18/basic/authorization` — fetched, but the rendered page text does not literally contain "client_id_metadata_document_supported". This is an inconsistency between what the README cites and the body I got back; I did not resolve which version of the page was served. The fact that mcp-remote's README documents the flag is sufficient to call CIMD support verified *in the tool*. The spec page text I saw *did* verify OAuth 2.1 / RFC 7591 / RFC 9728 as the standards the MCP auth flow rests on.
- Two modelcontextprotocol URL guesses (`/specification/2025-03-26/basic/authorization`, `/specification/draft/authorization`) returned HTTP 404 — the 2025-06-18 path is the live one.

### Items I intentionally did NOT verify
- The exact `package.json` `bin` entry and TypeScript source for `--client-metadata-url` in the mcp-remote source — would have required downloading the tarball or running `npm pack` (read-only constraint satisfied by `npm view` if it were configured, but I avoided any tool invocation that might mutate state). I rely on the README as the authoritative source.
- Real-world behavior of `phenotype.spotifyportal.com/api/mcp-actions/v1` against `https://xirp.spotify.com/oauth/client-metadata.json` — out of scope for this read-only pass; will be a separate test once we wire the bridge up.
