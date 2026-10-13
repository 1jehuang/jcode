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

## Lifecycle and safety

Remote requests preserve JSON-RPC request IDs, negotiated protocol versions,
and server-assigned session IDs. Legacy SSE endpoints must remain on the
configured server's origin. HTTP redirects are not followed with MCP credentials.
Shutdown closes pending requests and stops the connection's owned workers.

Use HTTPS for authenticated remote servers. Loopback HTTP is available for
local development and integration tests. OAuth endpoint discovery and callback
validation enforce secure URLs, resource binding, PKCE, and state validation.
