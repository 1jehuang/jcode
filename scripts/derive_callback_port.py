#!/usr/bin/env python3
"""Compute the loopback callback port mcp-remote will use for a CIMD client.

mcp-remote cannot be told which port to use. It derives one from a hash of the
connection parameters, and that hash includes the client-metadata URL itself:

    hash = md5(serverUrl + "|" + resource + "|" + clientMetadataUrl)
    port = 3335 + int(hash[0:4], 16) % 45816

Only the three populated parts are joined; empty ones are omitted entirely.
Because clientMetadataUrl participates, the redirect_uri that must be published
in the CIMD document cannot be written until that URL is decided.

This reproduces the shipped logic from mcp-remote 0.15.1
(dist/chunk-KJCT2FXL.js: getServerUrlHash, calculateDefaultPort).

Read-only. Performs no network access and changes nothing on disk.

Usage:
    python3 scripts/derive_callback_port.py <cimd-url> [server-url] [resource]

Example:
    python3 scripts/derive_callback_port.py \\
        https://kooshapari.com/oauth/client-metadata.json
"""
import hashlib
import sys

MCP_ENDPOINT = "https://phenotype.spotifyportal.com/api/mcp-actions/v1"
PORT_BASE = 3335
PORT_SPAN = 45816


def derive_port(server_url: str, resource: str, client_metadata_url: str) -> tuple[str, int, int]:
    """Return (hash, derived_port, hash_prefix_int) for the given parameters."""
    parts = [server_url]
    if resource:
        parts.append(resource)
    if client_metadata_url:
        parts.append(client_metadata_url)
    digest = hashlib.md5("|".join(parts).encode()).hexdigest()
    offset = int(digest[0:4], 16)
    return digest, PORT_BASE + offset % PORT_SPAN, offset


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    cimd = argv[1]
    if not cimd.startswith("https://"):
        print(f"refusing non-HTTPS client-metadata URL: {cimd}", file=sys.stderr)
        print("mcp-remote's isClientMetadataUrl requires https:", file=sys.stderr)
        return 1
    if cimd.rstrip("/").count("/") < 3:
        print(f"refusing client-metadata URL without a non-root path: {cimd}", file=sys.stderr)
        print("mcp-remote requires pathname !== \"/\" or it silently ignores the flag",
              file=sys.stderr)
        return 1

    server = argv[2] if len(argv) > 2 else MCP_ENDPOINT
    resource = argv[3] if len(argv) > 3 else server

    digest, port, offset = derive_port(server, resource, cimd)
    print(f"client_metadata_url : {cimd}")
    print(f"server_url          : {server}")
    print(f"resource            : {resource}")
    print(f"md5                 : {digest}")
    print(f"offset (hash[0:4])  : {offset}")
    print(f"derived port        : {port}")
    print()
    print("Publish this exact redirect_uri in the CIMD document:")
    print(f"    http://127.0.0.1:{port}/oauth/callback")
    print()
    print("NOTE: mcp-remote builds the callback as .../oauth/callback on 127.0.0.1,")
    print("while the allow-listed Xirp document declares http://127.0.0.1/callback")
    print("(default port, different path). Confirm with the vendor whether the")
    print("server compares redirect_uri strictly before publishing.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))