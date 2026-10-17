/**
 * jcode-cimd — serves the Jcode OAuth Client ID Metadata Document (SEP-991 CIMD).
 *
 * Purpose
 * -------
 * Spotify's Portal authorization server accepts an OAuth `client_id` that is an
 * HTTPS URL serving a client-metadata document, provided that URL matches an
 * entry in the Portal "Allowed client ID patterns" list. This Worker serves that
 * document for Jcode so Jcode can authenticate to Portal's MCP endpoint.
 *
 * Status: PREPARED, NOT DEPLOYED.
 * Deployment is a permission change (it admits a new client to authenticate
 * against Portal once the pattern is allow-listed), so it is left for deliberate
 * execution by the owner.
 *
 * Scope
 * -----
 * Bound to ONE exact path on one subdomain. It does not wildcard, does not proxy,
 * and does not touch the kooshapari.com apex, which is a DNS-only CNAME to Vercel
 * and cannot host a Worker route at all.
 *
 * Required order of operations
 * -----------------------------
 *   1. Deploy this Worker and verify:
 *        curl -sS https://auth.kooshapari.com/oauth/client-metadata.json
 *      must return HTTP 200 with content-type application/json.
 *   2. ONLY THEN append the pattern in Portal App Settings -> General -> MCP ->
 *      Allowed client ID patterns.
 *      APPEND, never replace: a custom list supersedes Backstage's implicit
 *      defaults for Claude, VS Code and the built-in CLI.
 *
 * The redirect port
 * -----------------
 * mcp-remote cannot be told which loopback port to use; it derives one from
 * md5(serverUrl + "|" + resource + "|" + clientMetadataUrl):
 *     port = 3335 + int(md5[0:4], 16) % 45816
 * For https://auth.kooshapari.com/oauth/client-metadata.json against
 * https://phenotype.spotifyportal.com/api/mcp-actions/v1 that is 47290.
 * Recompute with scripts/derive_callback_port.py if this URL ever changes.
 * This is the root of the chicken-and-egg: changing the URL changes the port.
 *
 * Note: Portal's documented default for "Allowed redirect URI patterns" permits
 * HTTP loopback redirects on localhost, 127.0.0.1 and [::1] on any port and
 * path when no list is configured, so the exact port is not expected to be a
 * hard gate; CALLBACK_PORT below is the value derived by
 * scripts/derive_callback_port.py for this URL.
 */

const CLIENT_METADATA_URL = "https://auth.kooshapari.com/oauth/client-metadata.json";
const CALLBACK_PORT = 47290;

const DOCUMENT = {
  client_id: CLIENT_METADATA_URL,
  client_name: "Jcode",
  redirect_uris: [`http://127.0.0.1:${CALLBACK_PORT}/oauth/callback`],
  response_types: ["code"],
  grant_types: ["authorization_code", "refresh_token"],
  token_endpoint_auth_method: "none",
  scope: "openid offline_access",
};

export default {
  async fetch(request) {
    const url = new URL(request.url);

    // Serve the document only at its exact path. Anything else falls through as
    // a 404 rather than accidentally serving metadata at unintended URLs.
    if (url.pathname !== "/oauth/client-metadata.json") {
      return new Response("Not Found\n", {
        status: 404,
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }

    return new Response(JSON.stringify(DOCUMENT, null, 2) + "\n", {
      status: 200,
      headers: {
        // Must be application/json. Spotify's own allow-listed document
        // (https://xirp.spotify.com/oauth/client-metadata.json) is served this
        // way, and raw.githubusercontent serves text/plain, which is an untested
        // risk. That is the main reason to use a Worker rather than a raw file.
        "content-type": "application/json; charset=utf-8",
        // Short TTL: the document changes only when client_name, the URL, or the
        // derived port change, and a stale copy would silently misconfigure auth.
        "cache-control": "public, max-age=300, must-revalidate",
        "x-content-type-options": "nosniff",
      },
    });
  },
};