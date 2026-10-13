# Remote MCP servers

Jcode connects to local stdio servers, Streamable HTTP servers, and legacy
HTTP+SSE servers. Existing stdio configurations continue to work.

## Configuration

Add remote servers to `~/.jcode/mcp.json` or a project `.mcp.json`:

```json
{
  "mcpServers": {
    "remote": {
      "type": "http",
      "url": "https://mcp.example.com/mcp"
    },
    "remote-with-token": {
      "type": "http",
      "url": "https://mcp.example.com/mcp",
      "headers": {
        "Authorization": "Bearer ${MCP_TOKEN}"
      }
    },
    "legacy": {
      "type": "sse",
      "url": "https://mcp.example.com/sse"
    }
  }
}
```

`http` and `streamable-http` select Streamable HTTP. An entry with a URL and
no command or explicit type also selects Streamable HTTP. `sse` explicitly
selects the legacy transport. HTTP connections can fall back to legacy SSE
when initialization indicates that the endpoint does not support Streamable
HTTP. Unknown transport types and remote entries without a URL are not run.

URLs and headers use the same `${VARIABLE}` and `${VARIABLE:-default}`
expansion as stdio configurations. A configured Authorization header takes
precedence over stored OAuth credentials. Do not put real credentials in
committed configuration files.

Jcode also reads supported external MCP configuration sources. When the same
server name has both a local stdio definition and a remote definition, the
existing local definition is retained. Use a distinct name when you want to
configure the remote server alongside it.

## Commands

```sh
jcode mcp list
jcode mcp test remote
jcode mcp login remote
jcode mcp login remote --no-browser
jcode mcp logout remote
```

`test` connects to the configured server, initializes the protocol, lists its
tools, and shuts down. It exits unsuccessfully when the connection or handshake
fails. It does not invoke an LLM or run the server's tools.

`login` is explicit and interactive. It discovers OAuth metadata, registers a
public client when supported, and completes an authorization-code flow with
PKCE and a loopback callback. `--no-browser` prints the authorization URL rather
than opening the system browser; the browser still needs access to the machine's
loopback callback. If the authorization server does not support dynamic client
registration, configure an Authorization header instead.

Tokens are stored in the Jcode home directory's `mcp-oauth.json` with restrictive
permissions. They are scoped to the remote resource and refreshed without
opening a browser. Background startup and ordinary connection attempts never
start an interactive login. A rejected or missing credential reports the
appropriate explicit login command rather than waiting for human interaction.

The MCP management tool accepts ad-hoc remote connections through `url`,
`transport`, and `headers`, or connects a named configured server without those
fields. `command`, `args`, and `env` remain the local stdio connection fields.

## Startup, connection waits, and prompt caching

Session tool registration does not await remote network traffic. Management,
search, and dispatch tools are registered immediately. Valid cached tool schemas
are advertised before background connections start. Each successful connection
publishes its tools independently, so a slow server cannot hide a fast peer's
tools or block management and calls to that peer.
An empty live catalog also replaces obsolete cached schemas instead of
advertising removed tools on the next startup.

The entire remote handshake has a 30-second deadline, including credential
refresh, initialization, legacy fallback, and tool listing. Individual tool
requests also default to 30 seconds. `timeout_secs` can extend legitimately slow
tool requests, but does not extend the total remote handshake deadline. A first
call to an offline server waits at most 30 seconds for connection. Cancellation
and shutdown remove waiters and stop owned response streams. No ordinary tool
call or startup path waits for an interactive OAuth login.

There is a separate, existing headless behavior: `jcode run` waits up to five
seconds for enabled servers with no valid cached schemas before its first turn.
Warm caches skip this wait. Set `JCODE_RUN_MCP_WAIT_MS=0` to disable it, or set a
smaller millisecond budget. Disabled servers do not consume the budget, and
readiness uses exact server identity rather than normalized tool-name prefixes.

In `auto` and `deferred` MCP modes, connection, disconnection, and tool discovery
keep the eager tool prefix and static system prompt stable. Provider-native
deferred definitions are outside that prefix. KV-cache telemetry now hashes and
counts the eager definitions only, so deferred catalog changes do not falsely
appear as harness-induced cache misses. Explicit `eager` mode with a cold or
changed schema cache can still intentionally rebuild its tool snapshot and cause
a prompt-cache miss. Warm cached schemas avoid that rebuild when unchanged.

### Verification performed on October 13, 2026

- Real TCP tests gate a slow server's initialize response while asserting that
  registration returns, a fast peer becomes discoverable, and management, search,
  and fast tool calls remain usable. Both shared and owned connection paths are
  covered, including a warm-cache restart in explicit eager mode.
- The two-server registration measurements were 357/241 microseconds cold/warm
  for owned connections, and 126/219 microseconds for shared connections
  (`justrust` run `20261013-072941134-560607`). These measure registry work in the
  test, not total application startup or Internet connection latency.
- A later local run under heavy concurrent CPU load measured 7.159/0.565
  milliseconds cold/warm for owned connections and 15.871/0.521 milliseconds
  for shared connections (`20261013-074441115-730779`). Registration still
  returned before the gated network responses. The different machines and load
  conditions are not an implementation speedup or a universal latency bound.
- Actual native Expo OAuth login succeeded in an isolated Jcode home. A native
  initialize/list/shutdown smoke test returned 39 tools in 2.404 seconds. Without
  credentials, the same endpoint returned a login instruction in 0.228 seconds,
  without opening a browser.
- Regression tests compare the real agent's eager schemas, static prompt,
  conversation, cache tracker, and KV monitor before and after HTTP connect,
  disconnect, and reconnect for native and fallback deferred providers. These
  verify the cache-relevant request prefix, not a paid provider's cache-hit bill.
- Passing suites included 113 base MCP tests, 54 app-core MCP tests, 78 broader
  agent tests, and 11 root-library MCP CLI tests. Some suites overlap. CLI tests
  cover disabled servers, cache fingerprint invalidation, and colliding normalized
  aliases. The final local runs followed a verified hosted filesystem failure,
  not an attempt to bypass hosted build accounting.

## Lifecycle and safety

Remote requests preserve JSON-RPC request IDs, negotiated protocol versions,
and server-assigned session IDs. Legacy SSE endpoints must remain on the
configured server's origin. HTTP redirects are not followed with MCP credentials.
Shutdown closes pending requests and stops the connection's owned workers.

Use HTTPS for authenticated remote servers. Loopback HTTP is available for
local development and integration tests. OAuth endpoint discovery and callback
validation enforce secure URLs, resource binding, PKCE, and state validation.
