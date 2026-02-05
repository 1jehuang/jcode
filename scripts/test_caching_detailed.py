#!/usr/bin/env python3
"""
Test caching behavior across multiple turns with tool usage.
"""

import socket
import json
import time
import os

DEBUG_SOCKET = f"/run/user/{os.getuid()}/jcode-debug.sock"

def send_cmd(sock, cmd, session_id=None, timeout=120):
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


def main():
    print("=" * 70)
    print("Multi-turn Caching Test")
    print("=" * 70)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(DEBUG_SOCKET)

    # Create a test session
    ok, output, err = send_cmd(sock, "create_session:/tmp/cache-test")
    if not ok:
        print(f"Failed to create session: {err}")
        return

    session_data = json.loads(output)
    session_id = session_data.get("session_id")
    print(f"Session: {session_id}\n")

    # Define a multi-step task
    messages = [
        "List the files in /tmp and tell me how many there are.",
        "Now read the contents of /etc/hostname",
        "What's the current date and time? Use the bash tool to run 'date'.",
        "Summarize what you found in the previous steps."
    ]

    total_input = 0
    total_output = 0
    total_cache_read = 0
    total_cache_creation = 0

    for i, msg in enumerate(messages):
        print(f"\n{'='*70}")
        print(f"Turn {i+1}: {msg[:50]}...")
        print("=" * 70)

        start = time.time()
        ok, response, err = send_cmd(sock, f"message:{msg}", session_id, timeout=180)
        elapsed = time.time() - start

        if not ok:
            print(f"Error: {err}")
            continue

        print(f"Time: {elapsed:.2f}s")
        print(f"Response: {response[:200]}...")

        # Get usage
        ok, usage_output, _ = send_cmd(sock, "usage", session_id)
        if ok:
            try:
                usage = json.loads(usage_output)
                input_tokens = usage.get('input_tokens', 0)
                output_tokens = usage.get('output_tokens', 0)
                cache_read = usage.get('cache_read_input_tokens') or 0
                cache_creation = usage.get('cache_creation_input_tokens') or 0

                total_input += input_tokens
                total_output += output_tokens
                total_cache_read += cache_read
                total_cache_creation += cache_creation

                print(f"\nUsage this turn:")
                print(f"  Input (non-cached): {input_tokens:,}")
                print(f"  Output: {output_tokens:,}")
                print(f"  Cache read: {cache_read:,}")
                print(f"  Cache creation: {cache_creation:,}")

                # Calculate cache efficiency
                total_input_this_turn = input_tokens + cache_read + cache_creation
                if total_input_this_turn > 0:
                    cache_hit_rate = (cache_read / total_input_this_turn) * 100
                    print(f"  Cache hit rate: {cache_hit_rate:.1f}%")

            except json.JSONDecodeError:
                print(f"Usage parse error: {usage_output}")

    # Summary
    print(f"\n{'='*70}")
    print("SUMMARY")
    print("=" * 70)
    print(f"\nTotal across {len(messages)} turns:")
    print(f"  Input (non-cached): {total_input:,}")
    print(f"  Output: {total_output:,}")
    print(f"  Cache read: {total_cache_read:,}")
    print(f"  Cache creation: {total_cache_creation:,}")

    total_all = total_input + total_cache_read + total_cache_creation
    if total_all > 0:
        overall_cache_rate = (total_cache_read / total_all) * 100
        print(f"\nOverall cache hit rate: {overall_cache_rate:.1f}%")

    # Effective tokens (cache reads at 10%)
    effective = total_input + (total_cache_read * 0.1) + total_output
    print(f"Effective tokens (cache @ 10%): {effective:,.0f}")

    # Check for anomalies
    print(f"\n{'='*70}")
    print("ANALYSIS")
    print("=" * 70)

    if total_cache_creation > total_cache_read:
        print("WARNING: More cache creation than reads - caching may not be working well")
        print(f"  Creation: {total_cache_creation:,} vs Read: {total_cache_read:,}")

    avg_non_cached = total_input / len(messages)
    print(f"\nAverage non-cached input per turn: {avg_non_cached:,.0f}")

    if avg_non_cached > 1000:
        print("NOTE: High non-cached input suggests dynamic content or cache misses")

    # Cleanup
    send_cmd(sock, f"destroy_session:{session_id}")
    sock.close()

    print("\nDone.")


if __name__ == "__main__":
    main()
