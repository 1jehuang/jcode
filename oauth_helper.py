#!/usr/bin/env python3
"""Helper script to complete Claude OAuth flow with proper PKCE."""
import base64
import hashlib
import json
import os
import secrets
import sys
import urllib.parse
import requests

CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"
AUTHORIZE_URL = "https://claude.ai/oauth/authorize"
TOKEN_URL = "https://console.anthropic.com/v1/oauth/token"
REDIRECT_URI = "https://console.anthropic.com/oauth/code/callback"
SCOPES = "org:create_api_key user:profile user:inference"

def generate_pkce():
    """Generate PKCE verifier and challenge."""
    verifier = secrets.token_urlsafe(48)[:64]  # 64 chars
    digest = hashlib.sha256(verifier.encode()).digest()
    challenge = base64.urlsafe_b64encode(digest).rstrip(b'=').decode()
    return verifier, challenge

def generate_state():
    """Generate random state for CSRF protection."""
    return secrets.token_hex(16)

def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--exchange":
        # Exchange mode: read code from stdin or arg
        code = sys.argv[2] if len(sys.argv) > 2 else input("Enter code: ").strip()

        # Load saved state
        with open("/tmp/claude_oauth_state.json") as f:
            saved = json.load(f)

        # Exchange code for tokens
        resp = requests.post(TOKEN_URL, data={
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "code_verifier": saved["verifier"],
            "redirect_uri": REDIRECT_URI,
        })

        if resp.status_code != 200:
            print(f"Error: {resp.text}")
            sys.exit(1)

        tokens = resp.json()
        print(f"\nAccess token: {tokens['access_token'][:50]}...")
        print(f"Refresh token: {tokens['refresh_token'][:50]}...")

        # Save in Claude Code format
        creds_dir = os.path.expanduser("~/.claude")
        os.makedirs(creds_dir, exist_ok=True)

        import time
        expires_at = int(time.time() * 1000) + (tokens["expires_in"] * 1000)

        creds = {
            "claudeAiOauth": {
                "accessToken": tokens["access_token"],
                "refreshToken": tokens["refresh_token"],
                "expiresAt": expires_at,
            }
        }

        with open(os.path.join(creds_dir, ".credentials.json"), "w") as f:
            json.dump(creds, f, indent=2)

        print(f"\nCredentials saved to ~/.claude/.credentials.json")
    else:
        # Generate new auth URL
        verifier, challenge = generate_pkce()
        state = generate_state()

        params = {
            "response_type": "code",
            "client_id": CLIENT_ID,
            "redirect_uri": REDIRECT_URI,
            "scope": SCOPES,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "state": state,
        }
        auth_url = f"{AUTHORIZE_URL}?{urllib.parse.urlencode(params)}"

        with open("/tmp/claude_oauth_state.json", "w") as f:
            json.dump({
                "verifier": verifier,
                "challenge": challenge,
                "state": state,
                "auth_url": auth_url
            }, f)

        print(f"Auth URL: {auth_url}")
        print(f"\nState saved to /tmp/claude_oauth_state.json")
        print(f"Verifier: {verifier}")

if __name__ == "__main__":
    main()
