#!/usr/bin/env python3
"""
Test OAuth usage comparison between Claude Code CLI and jcode direct API.

This script:
1. Shells out to Claude Code CLI with a simple prompt
2. Uses jcode's debug socket to send the same prompt via direct OAuth
3. Compares token usage between the two methods
4. Verifies actual OAuth quota consumption via the usage API
"""

import subprocess
import socket
import json
import time
import sys
import os
import requests

DEBUG_SOCKET = f"/run/user/{os.getuid()}/jcode-debug.sock"
MAIN_SOCKET = f"/run/user/{os.getuid()}/jcode.sock"
TEST_PROMPT = "What is 2+2? Reply with just the number."
CREDENTIALS_PATH = os.path.expanduser("~/.claude/.credentials.json")
USAGE_API_URL = "https://api.anthropic.com/api/oauth/usage"


def get_oauth_usage() -> dict:
    """Fetch current OAuth usage from the API."""
    try:
        with open(CREDENTIALS_PATH) as f:
            creds = json.load(f)
        token = creds['claudeAiOauth']['accessToken']

        response = requests.get(
            USAGE_API_URL,
            headers={
                'Authorization': f'Bearer {token}',
                'anthropic-beta': 'oauth-2025-04-20,claude-code-20250219',
                'Accept': 'application/json',
                'User-Agent': 'claude-cli/1.0.0'
            },
            timeout=10
        )
        if response.status_code == 200:
            return response.json()
    except Exception as e:
        print(f"Warning: Could not fetch OAuth usage: {e}")
    return {}


def run_claude_cli(prompt: str) -> dict:
    """Run Claude Code CLI and capture output/usage."""
    print(f"\n{'='*60}")
    print("Testing Claude Code CLI...")
    print(f"{'='*60}")

    start = time.time()
    try:
        result = subprocess.run(
            ["claude", "-p", prompt, "--output-format", "json"],
            capture_output=True,
            text=True,
            timeout=120
        )
        elapsed = time.time() - start

        print(f"Exit code: {result.returncode}")
        print(f"Time: {elapsed:.2f}s")

        if result.returncode != 0:
            print(f"stderr: {result.stderr}")
            return {"error": result.stderr, "time": elapsed}

        # Parse JSON output
        try:
            output = json.loads(result.stdout)
            response_text = output.get('result', str(output))
            print(f"Response: {response_text[:200]}")

            # Claude CLI outputs detailed usage in the JSON
            usage = output.get("usage", {})
            cost = output.get("total_cost_usd", 0)
            model_usage = output.get("modelUsage", {})

            print(f"\nUsage details:")
            print(f"  Input tokens: {usage.get('input_tokens', 'N/A')}")
            print(f"  Output tokens: {usage.get('output_tokens', 'N/A')}")
            print(f"  Cache read: {usage.get('cache_read_input_tokens', 0)}")
            print(f"  Cache creation: {usage.get('cache_creation_input_tokens', 0)}")
            print(f"  Total cost: ${cost:.6f}")

            return {
                "response": response_text,
                "usage": usage,
                "cost": cost,
                "model_usage": model_usage,
                "time": elapsed,
                "raw": output
            }
        except json.JSONDecodeError:
            print(f"Raw output: {result.stdout[:500]}")
            return {"response": result.stdout, "time": elapsed}

    except subprocess.TimeoutExpired:
        return {"error": "timeout", "time": 120}
    except FileNotFoundError:
        return {"error": "claude CLI not found", "time": 0}


def send_debug_cmd(sock, cmd: str, session_id: str = None, timeout: float = 60) -> tuple:
    """Send a debug command and get response."""
    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id

    sock.send((json.dumps(req) + '\n').encode())
    sock.settimeout(timeout)

    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        data += chunk
        if b'\n' in data:
            break

    resp = json.loads(data.decode().strip())
    return resp.get('ok', False), resp.get('output', ''), resp.get('error', '')


def run_jcode_oauth(prompt: str) -> dict:
    """Run via jcode debug socket using direct OAuth."""
    print(f"\n{'='*60}")
    print("Testing jcode direct OAuth API...")
    print(f"{'='*60}")

    # Check if debug socket exists
    if not os.path.exists(DEBUG_SOCKET):
        return {"error": f"Debug socket not found: {DEBUG_SOCKET}"}

    try:
        # Connect to debug socket
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.connect(DEBUG_SOCKET)

        # Create a test session
        ok, output, err = send_debug_cmd(sock, "create_session:/tmp/oauth-test")
        if not ok:
            return {"error": f"Failed to create session: {err}"}

        session_data = json.loads(output)
        session_id = session_data.get("session_id")
        print(f"Created session: {session_id}")

        # Get initial state to confirm provider
        ok, output, _ = send_debug_cmd(sock, "state", session_id)
        if ok:
            state = json.loads(output)
            print(f"Provider: {state.get('provider', 'unknown')}")
            print(f"Model: {state.get('model', 'unknown')}")

        # Send the test message
        start = time.time()
        ok, output, err = send_debug_cmd(sock, f"message:{prompt}", session_id, timeout=120)
        elapsed = time.time() - start

        print(f"Time: {elapsed:.2f}s")

        if not ok:
            send_debug_cmd(sock, f"destroy_session:{session_id}")
            sock.close()
            return {"error": f"Message failed: {err}", "time": elapsed}

        # The message command returns the text response directly (not JSON)
        print(f"Response: {output[:200]}")

        # Query usage via the "usage" command
        ok, usage_output, _ = send_debug_cmd(sock, "usage", session_id)
        usage = {}
        if ok:
            try:
                usage = json.loads(usage_output)
                print(f"\nUsage details:")
                print(f"  Input tokens: {usage.get('input_tokens', 'N/A')}")
                print(f"  Output tokens: {usage.get('output_tokens', 'N/A')}")
                print(f"  Cache read: {usage.get('cache_read_input_tokens', 0) or 0}")
                print(f"  Cache creation: {usage.get('cache_creation_input_tokens', 0) or 0}")
            except json.JSONDecodeError:
                print(f"Usage: {usage_output}")

        result = {
            "response": output,
            "usage": usage,
            "time": elapsed,
        }

        # Cleanup
        send_debug_cmd(sock, f"destroy_session:{session_id}")
        sock.close()

        return result

    except Exception as e:
        import traceback
        traceback.print_exc()
        return {"error": str(e)}


def main():
    print("OAuth Usage Comparison Test")
    print("="*60)
    print(f"Test prompt: {TEST_PROMPT}")

    # Check OAuth quota BEFORE tests
    print(f"\n{'='*60}")
    print("Checking OAuth quota before tests...")
    print(f"{'='*60}")
    usage_before = get_oauth_usage()
    five_hour_before = usage_before.get('five_hour', {}).get('utilization', 0)
    print(f"5-hour utilization: {five_hour_before:.2f}%")

    # Test Claude CLI
    cli_result = run_claude_cli(TEST_PROMPT)

    # Check quota after CLI test
    time.sleep(1)  # Wait for API to update
    usage_after_cli = get_oauth_usage()
    five_hour_after_cli = usage_after_cli.get('five_hour', {}).get('utilization', 0)
    cli_quota_delta = five_hour_after_cli - five_hour_before
    print(f"\nQuota after Claude CLI: {five_hour_after_cli:.2f}% (delta: +{cli_quota_delta:.4f}%)")

    # Test jcode OAuth
    jcode_result = run_jcode_oauth(TEST_PROMPT)

    # Check quota after jcode test
    time.sleep(1)  # Wait for API to update
    usage_after_jcode = get_oauth_usage()
    five_hour_after_jcode = usage_after_jcode.get('five_hour', {}).get('utilization', 0)
    jcode_quota_delta = five_hour_after_jcode - five_hour_after_cli
    print(f"\nQuota after jcode: {five_hour_after_jcode:.2f}% (delta: +{jcode_quota_delta:.4f}%)")

    # Summary
    print(f"\n{'='*60}")
    print("SUMMARY")
    print(f"{'='*60}")

    print("\nClaude Code CLI:")
    if "error" in cli_result:
        print(f"  Error: {cli_result['error']}")
    else:
        print(f"  Time: {cli_result.get('time', 'N/A'):.2f}s")
        usage = cli_result.get('usage', {})
        cost = cli_result.get('cost', 0)
        if usage:
            print(f"  Input tokens: {usage.get('input_tokens', 'N/A')}")
            print(f"  Output tokens: {usage.get('output_tokens', 'N/A')}")
            print(f"  Cache read: {usage.get('cache_read_input_tokens', 0)}")
            print(f"  Cache creation: {usage.get('cache_creation_input_tokens', 0)}")
            print(f"  Cost: ${cost:.6f}")

    print("\njcode Direct OAuth:")
    if "error" in jcode_result:
        print(f"  Error: {jcode_result['error']}")
    else:
        print(f"  Time: {jcode_result.get('time', 'N/A'):.2f}s")
        usage = jcode_result.get('usage', {})
        if usage:
            print(f"  Input tokens: {usage.get('input_tokens', 'N/A')}")
            print(f"  Output tokens: {usage.get('output_tokens', 'N/A')}")
            print(f"  Cache read: {usage.get('cache_read_input_tokens', 0) or 0}")
            print(f"  Cache creation: {usage.get('cache_creation_input_tokens', 0) or 0}")

    # Key insight
    print(f"\n{'='*60}")
    print("INSIGHT")
    print(f"{'='*60}")

    # Calculate totals for comparison
    cli_usage = cli_result.get('usage', {})
    jcode_usage = jcode_result.get('usage', {})

    cli_total = (cli_usage.get('input_tokens', 0) or 0) + \
                (cli_usage.get('cache_creation_input_tokens', 0) or 0) + \
                (cli_usage.get('cache_read_input_tokens', 0) or 0) + \
                (cli_usage.get('output_tokens', 0) or 0)

    jcode_total = (jcode_usage.get('input_tokens', 0) or 0) + \
                  (jcode_usage.get('cache_creation_input_tokens', 0) or 0) + \
                  (jcode_usage.get('cache_read_input_tokens', 0) or 0) + \
                  (jcode_usage.get('output_tokens', 0) or 0)

    cli_time = cli_result.get('time', 0)
    jcode_time = jcode_result.get('time', 0)
    speedup = cli_time / jcode_time if jcode_time > 0 else 0
    token_savings = 100 * (1 - jcode_total / cli_total) if cli_total > 0 else 0

    print(f"""
Both methods use the same OAuth token from ~/.claude/.credentials.json.

PERFORMANCE COMPARISON:
                    Claude CLI      jcode
  Response time:    {cli_time:.2f}s           {jcode_time:.2f}s ({speedup:.1f}x faster)
  Total tokens:     {cli_total:,}         {jcode_total:,} ({token_savings:.0f}% fewer)
  Estimated cost:   ${cli_result.get('cost', 0):.4f}         (not calculated)

ACTUAL QUOTA CONSUMPTION (from OAuth API):
  Before tests:     {five_hour_before:.2f}%
  After Claude CLI: {five_hour_after_cli:.2f}%  (+{cli_quota_delta:.4f}%)
  After jcode:      {five_hour_after_jcode:.2f}%  (+{jcode_quota_delta:.4f}%)

NOTES:
- The quota API shows percentage of a large 5-hour window (likely millions of tokens)
- Single requests (~10k-30k tokens) are too small to show meaningful % changes
- Both methods use the same OAuth token and count against the same quota
- Cache reads (cache_read tokens) count at reduced rates for billing
- For quota impact, what matters is: output tokens + non-cached input tokens
""")


if __name__ == "__main__":
    main()
