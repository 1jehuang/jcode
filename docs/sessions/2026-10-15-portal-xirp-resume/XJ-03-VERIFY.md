# XJ-03 — LIVE Bridge Verification

**Date**: 2026-10-17T22:46Z
**Bridge version**: mcp-remote@0.15.1 (npm `latest`; verified `dist-tags.latest = 0.15.1`)
**CIMD URL**: https://auth.kooshapari.com/oauth/client-metadata.json
**Portal MCP**: https://phenotype.spotifyportal.com/api/mcp-actions/v1
**Result**: **PARTIAL** — the bridge mechanism passes end-to-end, but the live CIMD is not publicly resolvable, and the shipped bridge config has two defects.

---

## What happened

Ran `mcp-remote@0.15.1` against the live Spotify Portal MCP endpoint, with the
live CIMD passed via `--client-metadata-url`. The bridge performed the full
OAuth 2.1 + CIMD startup sequence: it discovered the protected-resource metadata
(`authorizationServers: ["https://phenotype.spotifyportal.com/api/auth"]`), fetched
the AS metadata (`client_id_metadata_document_supported: true`), derived the
loopback callback port **47290** from the server URL (matching the CIMD's
`redirect_uris`), started the callback server on `http://127.0.0.1:47290`, and
emitted a valid authorization URL. The bridge **does not fetch the CIMD itself** —
under SEP-991, the client_id *is* the CIMD URL and the AS dereferences it. So a
dead CIMD did not stop the bridge; it will stop the **authorization server** when
it validates the client.

---

## Authorize URL produced (the PASS evidence)

```
https://phenotype.spotifyportal.com/api/auth/v1/authorize
  ?response_type=code
  &client_id=https%3A%2F%2Fauth.kooshapari.com%2Foauth%2Fclient-metadata.json
  &code_challenge_method=S256
  &redirect_uri=http%3A%2F%2Flocalhost%3A47290%2Foauth%2Fcallback
  &scope=openid+offline_access&prompt=consent
  &resource=https%3A%2F%2Fphenotype.spotifyportal.com%2Fapi%2Fmcp-actions%2Fv1
```

## Exit code

- Additive script `scripts/test-portal-bridge-live.sh`: **0 (PASS)**
- Inner `mcp-remote` process: **124** (killed by the harness `timeout` after it
  reached the authorize step and blocked waiting for the callback — expected, the
  flow never completes without a human consenting)

## Sample output

```
[9519] Using client metadata document: https://auth.kooshapari.com/oauth/client-metadata.json
[9519] Using callback port derived from the server URL: 47290
[9519] Successfully fetched authorization server metadata { issuer: 'https://phenotype.spotifyportal.com/api/auth' }
[9519] Identifying this client by its metadata document { client_id: 'https://auth.kooshapari.com/oauth/client-metadata.json' }
[9519] OAuth callback server running at http://127.0.0.1:47290
[9519] Please authorize this client by visiting:
https://phenotype.spotifyportal.com/api/auth/v1/authorize?response_type=code&client_id=https%3A%2F%2Fauth...
```

---

## Issues found

1. **BLOCKER — the live CIMD does not resolve.** `auth.kooshapari.com` is
   **NXDOMAIN** from the Cloudflare authoritative nameservers
   (`glen.ns.cloudflare.com`, `grannbo.ns.cloudflare.com`), from `1.1.1.1`, and
   from `8.8.8.8`. `curl https://auth.kooshapari.com/oauth/client-metadata.json`
   → `Could not resolve host`. `https://kooshapari.com/oauth/client-metadata.json`
   → HTTP 404. This **contradicts STATE.md §24 condition 1** ("curl returns valid
   SEP-991 CIMD (356 B)"). Because the Portal AS advertises
   `client_id_metadata_document_supported: true`, it will dereference the
   `client_id` URL during authorization. With the record NXDOMAIN, the real flow
   will fail at the AS with a client-validation error, even though the bridge
   starts correctly.
2. **DEFECT — `scripts/portal-mcp-bridge.json` arg order is wrong.** mcp-remote
   0.15.1 requires the server URL as the **first positional argument**. The
   shipped args put `--transport http-only` before the URL, which makes
   mcp-remote parse `--transport` as the URL:
   `Fatal error: TypeError: Invalid URL ... input: '--transport'`. Merging the
   config verbatim into `~/.jcode/mcp.json` fails immediately.
3. **MISMATCH — redirect_uri host.** The CIMD declares
   `http://127.0.0.1:47290/oauth/callback`; mcp-remote emits
   `http://localhost:47290/oauth/callback`. Both are loopback and the Portal's
   default loopback policy accepts any loopback host/port/path, but if the AS
   ever enforces an exact-string match against the CIMD `redirect_uris`, this
   fails. Passing `--host 127.0.0.1` would align them.
4. **SIDE EFFECT (benign)** — on the first (debug) run the real `open` helper
   launched a browser tab at the authorize URL. No consent was given and the
   process was killed, so no OAuth code was exchanged/consumed. Subsequent runs
   set `BROWSER=/usr/bin/true` to suppress it.

---

## Operator next step

**Since the result is PARTIAL, the CIMD blocker must be fixed before deploy:**

1. **Re-deploy / re-check the Cloudflare Worker + DNS record** for
   `auth.kooshapari.com`. There is currently **no DNS record** for the subdomain
   at all (NXDOMAIN at the authoritative NS). Confirm the record exists and
   resolves publicly; then re-run
   `bash scripts/test-portal-bridge-live.sh` and confirm the CIMD fetches.
2. **Fix the arg order** in `scripts/portal-mcp-bridge.json` (and the eventual
   `~/.jcode/mcp.json` merge): the endpoint URL must come first, then options.
   Correct order:
   ```
   -y mcp-remote@0.15.1
   https://phenotype.spotifyportal.com/api/mcp-actions/v1
   --transport http-only
   --client-metadata-url https://auth.kooshapari.com/oauth/client-metadata.json
   --resource https://phenotype.spotifyportal.com/api/mcp-actions/v1
   ```
3. Optionally add `--host 127.0.0.1` to match the CIMD's redirect host exactly.
4. Once the CIMD resolves, the remaining step is human consent in the browser —
   the bridge is proven ready up to that point.

---

## Reproduction

```bash
# additive script (does not touch master test/config)
MCP_REMOTE_CONFIG_DIR=$(mktemp -d) bash scripts/test-portal-bridge-live.sh
# -> RESULT: PASS — bridge reached the OAuth authorize step   (exit 0)

# CIMD reachability check
dig @glen.ns.cloudflare.com auth.kooshapari.com    # NXDOMAIN
curl -sS https://auth.kooshapari.com/oauth/client-metadata.json  # resolve error
```
