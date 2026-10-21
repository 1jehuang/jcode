#!/usr/bin/env python3
"""Stub CIMD server for local mcp-remote bridge testing.

Serves a Client ID Metadata Document (SEP-991) at /cimd.json and logs
all requests. Used to verify that mcp-remote ACCEPTS the client metadata
URL and configures the matching callback host before the real Cloudflare
Worker is deployed.

WHAT MCP-REMOTE ACTUALLY DOES WITH THE CIMD (corrected 2026-10-19):
    mcp-remote does NOT download this document. It uses the CIMD URL AS
    THE client_id (SEP-991) and asks the AUTHORIZATION SERVER to fetch it;
    mcp-remote only reads the AS metadata to check that
    `client_id_metadata_document_supported` is true. So this stub never
    sees a /cimd.json request from mcp-remote -- the AS would make it.
    The local test therefore asserts on mcp-remote's own log output
    (CIMD accepted, callback host, callback port) and on the AS-side
    behaviour, which is verified against the real Spotify Portal.

WHY HTTPS (changed 2026-10-19):
    mcp-remote's isClientMetadataUrl() requires `https:` AND a non-root
    pathname. An `http://127.0.0.1:PORT/cimd.json` client metadata URL is
    SILENTLY IGNORED -- mcp-remote logs "Warning: Ignoring invalid client
    metadata URL" and proceeds as if no client metadata document was
    configured. The previous version of this stub served plain HTTP, so
    mcp-remote never even considered the document it was supposed to use.

    This server therefore serves HTTPS using a self-signed certificate for
    127.0.0.1. The caller must point NODE_EXTRA_CA_CERTS at the CA file so
    mcp-remote's undici fetch trusts it (undici honours NODE_EXTRA_CA_CERTS).

The CIMD document uses a redirect_uri targeting a port derived from
a localhost server URL + this stub's own URL, so the port is computed
dynamically at startup.

Usage:
    python3 scripts/stub-cimd-server.py [--tls-dir DIR] [port]

    --tls-dir DIR  directory containing cert.pem and key.pem. When omitted
                   (or the files are absent) the server starts in PLAIN HTTP
                   mode and prints a warning that mcp-remote will ignore
                   the client metadata URL.
    port: TCP port to listen on (default: random available port)

    Output: prints the bound port to stdout (first line), then logs
    requests to stderr. The first line is consumed by the test harness.
    The second stdout line is "scheme=https" or "scheme=http" so the harness
    knows which mode it is in.
"""
import argparse
import hashlib
import http.server
import json
import os
import ssl
import sys
import threading

PORT_BASE = 3335
PORT_SPAN = 45816
MCP_ENDPOINT = "https://phenotype.spotifyportal.com/api/mcp-actions/v1"


def derive_port(server_url: str, resource: str, cimd_url: str) -> int:
    parts = [server_url]
    if resource:
        parts.append(resource)
    if cimd_url:
        parts.append(cimd_url)
    digest = hashlib.md5("|".join(parts).encode()).hexdigest()
    return PORT_BASE + int(digest[0:4], 16) % PORT_SPAN


class CIMDHandler(http.server.BaseHTTPRequestHandler):
    """Serves /cimd.json with the correct SEP-991 fields."""

    # Set at startup by main()
    cimd_document: str = ""
    cimd_content_type: str = "application/json; charset=utf-8"

    def do_GET(self):
        # Strip query string for matching
        path = self.path.split("?")[0]

        if path == "/cimd.json":
            self.send_response(200)
            self.send_header("Content-Type", self.cimd_content_type)
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(self.cimd_document.encode())
            print(f"[stub] 200 /cimd.json ({len(self.cimd_document)} bytes)", file=sys.stderr)
        else:
            self.send_response(404)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"Not Found\n")
            print(f"[stub] 404 {self.path}", file=sys.stderr)

    def log_message(self, format, *args):
        # Suppress default stderr logging; we do our own
        pass


class QuietTCPServer(http.server.HTTPServer):
    allow_reuse_address = True


def main() -> int:
    parser = argparse.ArgumentParser(add_help=True)
    parser.add_argument("--tls-dir", default=None)
    parser.add_argument("port", nargs="?", type=int, default=0)
    args = parser.parse_args()

    port = args.port

    # We don't know the stub's own URL until we bind, so use a placeholder
    # server URL for the port derivation. The key point: the redirect_uri
    # in the CIMD doc must match what mcp-remote derives. Since mcp-remote
    # hashes serverUrl + resource + clientMetadataUrl, and for our local test
    # serverUrl = http://localhost:<STUB_PORT>, the derived port depends on
    # the stub port itself. We solve this by computing the derived port for
    # a few candidate stub ports and picking one that's self-consistent, or
    # by using the real CIMD URL's pre-computed port (47290) as the redirect
    # in the stub doc since the test only verifies *fetching*, not *completing*
    # the OAuth dance.
    #
    # For the test, we use a fixed redirect_uri derived from the production
    # CIMD URL. The stub's purpose is to prove mcp-remote *fetches* the doc,
    # not to complete OAuth. The real Cloudflare Worker will serve the
    # production CIMD with the correct port.

    PRODUCTION_CIMD_URL = "https://auth.kooshapari.com/oauth/client-metadata.json"
    PRODUCTION_PORT = derive_port(MCP_ENDPOINT, MCP_ENDPOINT, PRODUCTION_CIMD_URL)

    cimd_doc = {
        "client_id": "PLACEHOLDER_CIMD_URL",
        "client_name": "Jcode (test stub)",
        "redirect_uris": [f"http://127.0.0.1:{PRODUCTION_PORT}/oauth/callback"],
        "response_types": ["code"],
        "grant_types": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_method": "none",
        "scope": "openid offline_access",
    }

    CIMDHandler.cimd_document = json.dumps(cimd_doc, indent=2) + "\n"

    scheme = "http"
    ssl_context = None
    if args.tls_dir:
        cert = os.path.join(args.tls_dir, "cert.pem")
        key = os.path.join(args.tls_dir, "key.pem")
        if os.path.exists(cert) and os.path.exists(key):
            ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            ssl_context.load_cert_chain(certfile=cert, keyfile=key)
            scheme = "https"
        else:
            print(
                f"[stub] WARNING: no cert.pem/key.pem in {args.tls_dir}; "
                "falling back to PLAIN HTTP. mcp-remote will IGNORE an "
                "http:// client metadata URL, so the CIMD fetch assertion "
                "will not be meaningful.",
                file=sys.stderr,
            )

    server = QuietTCPServer(("127.0.0.1", port), CIMDHandler)
    if ssl_context is not None:
        server.socket = ssl_context.wrap_socket(server.socket, server_side=True)
    actual_port = server.server_address[1]
    cimd_url = f"{scheme}://127.0.0.1:{actual_port}/cimd.json"

    # Update the doc's client_id to be the actual stub URL (so mcp-remote
    # can fetch it and the hash is self-consistent)
    cimd_doc["client_id"] = cimd_url
    # Recompute redirect for this actual stub URL
    stub_server_url = f"{scheme}://127.0.0.1:{actual_port}"
    stub_port = derive_port(stub_server_url, stub_server_url, cimd_url)
    cimd_doc["redirect_uris"] = [f"http://127.0.0.1:{stub_port}/oauth/callback"]
    CIMDHandler.cimd_document = json.dumps(cimd_doc, indent=2) + "\n"

    # Print the port on the first line for the test harness
    print(actual_port, flush=True)
    print(f"scheme={scheme}", flush=True)
    print(f"[stub] CIMD URL : {cimd_url}", file=sys.stderr)
    print(f"[stub] Listening: {scheme}://127.0.0.1:{actual_port}", file=sys.stderr)
    print(f"[stub] Redirect : http://127.0.0.1:{stub_port}/oauth/callback", file=sys.stderr)

    # Handle requests until the parent kills us
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
