#!/usr/bin/env python3
"""Refresh the XJ-03 Spotify Portal token and persist the rotation atomically.

Why this exists (2026-10-19)
---------------------------
The Spotify Portal authorization server ROTATES refresh tokens. Every
successful ``refresh_token`` grant invalidates the refresh token it
consumed and returns a new one. The grant succeeds with HTTP 200 while
the *old* token is already dead.

That makes a naive refresh dangerous. If a refresh is performed with curl
and the response is not written back to ``tokens.json``, the cache is left
holding a spent refresh token. The next launch then fails with::

    {"error":"invalid_grant","error_description":"Invalid refresh token"}

which looks like a broken authorization server but is really a local
cache-consistency bug. This was observed live on 2026-10-19.

This helper performs the refresh and writes the rotated pair back into
``tokens.json`` in the same operation, so the cache never holds a spent
refresh token. It is idempotent: running it repeatedly leaves the cache
in a refreshable state every time.

It never prints token material.

Usage
-----
    python3 scripts/refresh-portal-token.py            # refresh + persist
    python3 scripts/refresh-portal-token.py --check    # report only, no refresh
    python3 scripts/refresh-portal-token.py --refresh-if-expiring 600
                                                       # no-op unless <10 min left

Concurrency
-----------
Refresh tokens are single-use AND rotating: every successful exchange kills the
token it presented. Two concurrent refreshes of the same cache therefore race.
One wins and the other gets ``invalid_grant``, and whichever writer lands last
can leave a spent token on disk. ``mcp-remote`` also refreshes on launch, so the
race is real rather than theoretical.

The read/exchange/write cycle is therefore serialised with an advisory
exclusive lock on ``<token-path>.lock``, and the cache is re-read *inside* the
lock. On platforms without ``fcntl`` the lock is skipped and the atomic write is
the only protection.

Exit codes
----------
    0 = refresh succeeded and the rotated tokens were persisted, or no refresh
        was needed under --refresh-if-expiring
    1 = the refresh failed, or the token endpoint was unreachable
    2 = no usable token cache found, or invalid arguments
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

try:
    import fcntl
except ImportError:  # pragma: no cover - Windows has no fcntl
    fcntl = None  # type: ignore[assignment]

TOKEN_PATH = os.path.expanduser(
    "~/.mcp-auth/mcp-remote-v1/abb316d3ba70bc7220e11618a1b43906_tokens.json"
)
# Public OAuth token endpoint, not a credential.
TOKEN_ENDPOINT = "https://phenotype.spotifyportal.com/api/auth/v1/token"
CLIENT_ID = "https://auth.kooshapari.com/oauth/client-metadata.json"
RESOURCE = "https://phenotype.spotifyportal.com/api/mcp-actions/v1"

# Sentinel status for a transport failure, distinct from any HTTP status code.
NETWORK_ERROR = -1


def post_form(url: str, fields: dict[str, str]) -> tuple[int, dict]:
    """POST an application/x-www-form-urlencoded body and decode the JSON reply.

    Returns ``(status, body)``. A JSON error response comes back with its real
    HTTP status. A transport failure (DNS, TLS, timeout, connection reset) is
    reported as ``(NETWORK_ERROR, {...})`` so callers can tell "the server said
    no" apart from "we never reached the server". No request field is ever
    included in the returned body, so token material cannot leak through it.
    """
    body = urllib.parse.urlencode(fields).encode()
    request = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    try:
        # Scheme is a module constant pointing at a fixed https endpoint.
        with urllib.request.urlopen(request, timeout=30) as response:
            return response.status, json.loads(response.read().decode())
    except urllib.error.HTTPError as exc:
        # HTTPError is a URLError subclass, so it must be handled first.
        raw = exc.read().decode()
        try:
            return exc.code, json.loads(raw)
        except json.JSONDecodeError:
            return exc.code, {"error": "unparseable_response", "raw": raw[:200]}
    except urllib.error.URLError as exc:
        return NETWORK_ERROR, {"error": "network_error", "reason": str(exc.reason)}
    except (TimeoutError, OSError) as exc:
        return NETWORK_ERROR, {"error": "network_error", "reason": type(exc).__name__}


def describe(cached: dict) -> str:
    """Summarize the cached token set without revealing any token material."""
    expires_at = cached.get("expires_at")
    left = "unknown"
    if isinstance(expires_at, (int, float)):
        left = f"{int(expires_at / 1000 - time.time())}s"
    return (
        f"access_token=<{len(cached.get('access_token', ''))} chars> "
        f"refresh_token=<{len(cached.get('refresh_token', ''))} chars> "
        f"expires_in={cached.get('expires_in')} left={left} "
        f"scope={cached.get('requested_scope') or cached.get('scope')}"
    )


def persist_durably(token_path: str, payload: dict) -> None:
    """Atomically and durably replace ``token_path`` with ``payload`` (mode 0600).

    The temporary file is created *in the destination directory* with
    owner-only permissions before any data is written (``NamedTemporaryFile``
    uses 0600), then flushed and fsynced. ``os.replace`` swaps it into place
    atomically, and on POSIX the parent directory is fsynced so the rename
    itself survives a crash. On Windows the rename is used as-is and the
    durability guarantee is limited to what the OS provides for it. The
    temporary file is removed if anything fails before the swap.
    """
    directory = os.path.dirname(os.path.abspath(token_path)) or "."
    tmp_path = ""
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=directory,
        prefix=".tokens-",
        suffix=".tmp",
        delete=False,
    ) as handle:
        tmp_path = handle.name
        if hasattr(os, "fchmod"):
            os.fchmod(handle.fileno(), 0o600)  # already 0600; make it explicit
        json.dump(payload, handle, indent=2)
        handle.flush()
        os.fsync(handle.fileno())

    try:
        os.replace(tmp_path, token_path)
    except OSError:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass
        raise

    if os.name == "posix":
        dir_fd = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)


def remaining_seconds(cached: dict) -> float | None:
    """Seconds until the cached access token expires, or None if unknown."""
    expires_at = cached.get("expires_at")
    if not isinstance(expires_at, (int, float)):
        return None
    return expires_at / 1000.0 - time.time()


def acquire_lock(lock_path: str) -> int | None:
    """Take an exclusive advisory lock; return the fd, or None if unsupported.

    Blocks until the lock is available. The lock file is created 0600 next to
    the cache. It is never unlinked: removing a lock file while another process
    holds it reopens the race the lock exists to close.
    """
    if fcntl is None:
        return None
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
    except OSError:
        os.close(fd)
        raise
    return fd


def release_lock(fd: int | None) -> None:
    """Release a lock taken by :func:`acquire_lock`."""
    if fd is None:
        return
    try:
        fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="report the cached token state without performing a refresh",
    )
    parser.add_argument(
        "--token-path",
        default=TOKEN_PATH,
        help="override the tokens.json path (default: %(default)s)",
    )
    parser.add_argument(
        "--refresh-if-expiring",
        type=int,
        metavar="SECONDS",
        default=None,
        help=(
            "refresh only when the access token expires within SECONDS; a no-op "
            "otherwise. Every refresh consumes a rotation, so prefer this over an "
            "unconditional refresh when calling on a schedule."
        ),
    )
    args = parser.parse_args()
    # Operator-supplied path, by design: this helper manages one cache file
    # that the caller may relocate (e.g. a per-run scratch copy in the tests).
    token_path = os.path.expanduser(args.token_path)

    if args.refresh_if_expiring is not None and args.refresh_if_expiring < 0:
        print("FAIL: --refresh-if-expiring requires a non-negative number of seconds")
        return 2

    if not os.path.exists(token_path):
        print(f"FAIL: no token cache at {token_path}")
        return 2

    with open(token_path, encoding="utf-8") as handle:
        cached = json.load(handle)

    if args.check:
        print(f"cache: {describe(cached)}")
        return 0

    # Cheap skip before locking. A comfortably fresh token needs no work, and
    # skipping avoids burning a rotation (and the race window that comes with it).
    if args.refresh_if_expiring is not None:
        left = remaining_seconds(cached)
        if left is not None and left > args.refresh_if_expiring:
            print(
                f"OK: no refresh needed ({int(left)}s left; "
                f"threshold {args.refresh_if_expiring}s)"
            )
            return 0

    lock_fd = acquire_lock(token_path + ".lock")
    try:
        # Re-read INSIDE the lock. Another process (or mcp-remote itself) may
        # have rotated the pair between the first read and lock acquisition, and
        # refresh tokens are single-use, so acting on a stale read would present
        # a token that is already spent.
        with open(token_path, encoding="utf-8") as handle:
            cached = json.load(handle)

        if args.refresh_if_expiring is not None:
            left = remaining_seconds(cached)
            if left is not None and left > args.refresh_if_expiring:
                print(
                    f"OK: no refresh needed ({int(left)}s left; "
                    f"threshold {args.refresh_if_expiring}s)"
                )
                return 0

        refresh_token = cached.get("refresh_token")
        if not refresh_token:
            print("FAIL: cached token set has no refresh_token")
            return 2

        status, body = post_form(
            TOKEN_ENDPOINT,
            {
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": CLIENT_ID,
                "resource": RESOURCE,
                "scope": cached.get("requested_scope") or cached.get("scope") or "",
            },
        )

        if status == NETWORK_ERROR:
            print(
                f"FAIL: could not reach the token endpoint ({body.get('reason')}). "
                "The token cache was NOT modified; retry once the network is back."
            )
            return 1

        if status != 200 or not body.get("access_token"):
            error = body.get("error")
            if error == "invalid_grant":
                print(
                    f"FAIL: refresh returned HTTP {status} (invalid_grant). The cached "
                    "refresh token was already spent; re-run the consent flow to mint "
                    "a new one."
                )
            else:
                print(
                    f"FAIL: refresh returned HTTP {status} ({error}). The token cache "
                    "was NOT modified; retry later. If this persists, re-run the "
                    "consent flow."
                )
            return 1

        # Persist the rotated pair BEFORE reporting success. The refresh token
        # just consumed is now spent and must not remain in the cache.
        cached["access_token"] = body["access_token"]
        if body.get("refresh_token"):
            cached["refresh_token"] = body["refresh_token"]
        if body.get("id_token"):
            cached["id_token"] = body["id_token"]
        if body.get("expires_in"):
            cached["expires_in"] = body["expires_in"]
            cached["expires_at"] = (
                int(time.time() * 1000) + int(body["expires_in"]) * 1000
            )

        persist_durably(token_path, cached)

        print("OK: refreshed (HTTP 200); rotated refresh token persisted")
        print(f"    cache: {describe(cached)}")
        return 0
    finally:
        release_lock(lock_fd)


if __name__ == "__main__":
    sys.exit(main())
