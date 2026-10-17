# XJ-03 — Portal MCP Bridge Deploy

**Date:** 2026-10-17
**Branch:** `chore/xirp-custom-harness-discovery-20261014`
**Status:** Deployed locally; real deploy blocked on operator + vendor

---

## What's deployed (local)

| Artifact | Path | Purpose |
|----------|------|---------|
| Bridge config snippet | `scripts/portal-mcp-bridge.json` | JSON entry for `~/.jcode/mcp.json` — `portal-bridge` server using `mcp-remote@0.15.1` with CIMD |
| Stub CIMD server | `scripts/stub-cimd-server.py` | Local HTTP server serving a valid SEP-991 CIMD document at `/cimd.json` for testing |
| Local test | `scripts/test-portal-bridge.sh` | Verifies `mcp-remote` fetches the stub CIMD and starts OAuth flow |
| Port derivation | `scripts/derive_callback_port.py` | (Pre-existing) Computes the loopback callback port from CIMD URL |

## What's blocked (real deploy)

The bridge config exists but **cannot be merged into `~/.jcode/mcp.json`** until two conditions are met. These are sequential — (2) depends on (1).

### Block 1: CIMD hosting

The Cloudflare Worker at `jcode-cimd.worker.js` must be deployed to `auth.kooshapari.com`.

- **Source:** `docs/sessions/2026-10-15-portal-xirp-resume/jcode-cimd.worker.js`
- **Route:** `auth.kooshapari.com/oauth/client-metadata.json`
- **Required response:** HTTP 200, `content-type: application/json`, body is the CIMD JSON document
- **The CIMD document fields are:**
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
- **Redirect port 47290** is derived from `md5(serverUrl + "|" + resource + "|" + clientMetadataUrl)`. Reproduce with:
  ```bash
  python3 scripts/derive_callback_port.py https://auth.kooshapari.com/oauth/client-metadata.json
  ```
- **Verification after deploy:**
  ```bash
  curl -sS -I https://auth.kooshapari.com/oauth/client-metadata.json
  # Must show: HTTP 200, content-type: application/json
  ```

### Block 2: Spotify Portal allow-list

The CIMD URL pattern must be added to Portal's "Allowed client ID patterns" list.

- **Where:** Spotify Portal App Settings → General → MCP → Allowed client ID patterns
- **Pattern to add:** `https://auth.kooshapari.com/*`
- **Important:** APPEND — do not replace. The existing patterns (`https://claude.ai/*`, `https://chatgpt.com/oauth/codex/*/client.json`, `https://backstage.spotify.com/chirp.json`, `https://xirp.spotify.com/oauth/client-metadata.json`) must be preserved.
- **Verification after adding:** Portal's `/oauth2/authorize` endpoint should return HTTP 302 (not 400/403) when probed with `client_id=https://auth.kooshapari.com/oauth/client-metadata.json`.

## Unblock checklist (for the operator)

- [ ] Deploy the Cloudflare Worker (`jcode-cimd.worker.js`) to `auth.kooshapari.com`
- [ ] Verify `curl -sS -I https://auth.kooshapari.com/oauth/client-metadata.json` returns `200` with `application/json`
- [ ] In Portal App Settings, append `https://auth.kooshapari.com/*` to "Allowed client ID patterns"
- [ ] Verify the authorize probe returns 302 with the new client_id
- [ ] Merge `scripts/portal-mcp-bridge.json` into `~/.jcode/mcp.json` under `servers.portal-bridge`
- [ ] Replace `PLACEHOLDER_CIMD_URL` in the args with `https://auth.kooshapari.com/oauth/client-metadata.json`
- [ ] Run `scripts/test-portal-bridge.sh` to verify the local bridge test still passes
- [ ] Test the real bridge: restart jcode, the `portal-bridge` server should appear and open a browser for OAuth consent

## Bridge config (for reference)

The `portal-bridge` entry for `~/.jcode/mcp.json`:

```json
{
  "portal-bridge": {
    "command": "npx",
    "args": [
      "-y", "mcp-remote@0.15.1",
      "--transport", "http-only",
      "https://phenotype.spotifyportal.com/api/mcp-actions/v1",
      "--client-metadata-url", "https://auth.kooshapari.com/oauth/client-metadata.json",
      "--resource", "https://phenotype.spotifyportal.com/api/mcp-actions/v1"
    ],
    "description": "Spotify Portal MCP via mcp-remote bridge with CIMD (SEP-991).",
    "timeout_secs": 120
  }
}
```

## Pre-existing portal entry

The existing `phenotype-portal` entry (type: `http`, native Jcode HTTP MCP) remains in `~/.jcode/mcp.json`. The `portal-bridge` entry is an alternative that uses `mcp-remote` for OAuth 2.1 + CIMD support. Once the bridge works, the native `http` entry can be removed (it cannot do OAuth on its own).

## Port derivation

mcp-remote derives the loopback callback port from a hash of the connection parameters:

```
hash = md5(serverUrl + "|" + resource + "|" + clientMetadataUrl)
port = 3335 + int(hash[0:4], 16) % 45816
```

For the production CIMD URL `https://auth.kooshapari.com/oauth/client-metadata.json`:
- Server URL: `https://phenotype.spotifyportal.com/api/mcp-actions/v1`
- Resource: `https://phenotype.spotifyportal.com/api/mcp-actions/v1`
- **Derived port: 47290**
- **Redirect URI: `http://127.0.0.1:47290/oauth/callback`**

If the CIMD URL changes, re-derive: `python3 scripts/derive_callback_port.py <new-url>`

## Open issue: redirect_uri path mismatch

mcp-remote builds its callback as `http://127.0.0.1:<port>/oauth/callback`, but the allow-listed Xirp document declares `http://127.0.0.1/callback` (default port, different path). If the Portal authorization server compares `redirect_uri` strictly, the derived port must be registered in the CIMD document. The Xirp form would not match. This should be confirmed with the vendor before the real deploy.

## References

- Bridge evaluation: `docs/sessions/2026-10-15-portal-xirp-resume/evidence/mcp-bridge-research.md`
- CIMD template: `docs/sessions/2026-10-15-portal-xirp-resume/portal-cimd.client-metadata.template.json`
- Worker source: `docs/sessions/2026-10-15-portal-xirp-resume/jcode-cimd.worker.js`
- Allow-list evidence: `STATE.md` §15
- Portal sign-in history: `STATE.md` §13