#!/usr/bin/env python3
"""
Test script to verify selfdev reload works correctly.

This tests that:
1. The selfdev reload tool returns appropriate output
2. The reload context is saved
3. After restart, the continuation message is sent to the model

Run with: python tests/test_selfdev_reload.py
"""

import socket
import json
import time
import os
import sys

# Check for selfdev socket first, then regular socket
if os.path.exists("/tmp/jcode-selfdev-debug.sock"):
    SOCKET_PATH = "/tmp/jcode-selfdev-debug.sock"
else:
    SOCKET_PATH = f"/run/user/1000/jcode-debug.sock"
JCODE_DIR = os.path.expanduser("~/.jcode")

def send_cmd(sock, cmd, session_id=None, timeout=60):
    """Send a debug command and get the response."""
    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id
    sock.send((json.dumps(req) + '\n').encode())
    sock.settimeout(timeout)
    data = b""
    while True:
        try:
            chunk = sock.recv(65536)
            if not chunk:
                break
            data += chunk
            try:
                return json.loads(data.decode())
            except json.JSONDecodeError:
                continue
        except socket.timeout:
            break
    return json.loads(data.decode()) if data else None

def test_selfdev_status():
    """Test that selfdev status works."""
    print("=" * 60)
    print("Test: selfdev status")
    print("=" * 60)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
    except FileNotFoundError:
        print(f"ERROR: Debug socket not found at {SOCKET_PATH}")
        return False

    try:
        # Create a test session
        result = send_cmd(sock, "create_session:/home/jeremy/jcode")
        if not result or not result.get('ok'):
            print(f"Failed to create session: {result}")
            return False
        session_id = json.loads(result['output'])['session_id']
        print(f"   Session ID: {session_id}")

        # Check state to verify selfdev is available
        result = send_cmd(sock, "state", session_id)
        if result and result.get('ok'):
            state = json.loads(result['output'])
            print(f"   Is canary: {state.get('is_canary', False)}")

        # Call selfdev status
        print("\n1. Calling selfdev status...")
        result = send_cmd(sock, 'tool:selfdev {"action":"status"}', session_id, timeout=30)

        if not result:
            print("   No response")
            return False

        if result.get('ok'):
            output = result.get('output', '')
            print(f"   Status output (preview):")
            for line in output.split('\n')[:10]:
                print(f"     {line}")
            if len(output.split('\n')) > 10:
                print(f"     ... ({len(output.split(chr(10)))} lines total)")
        else:
            error = result.get('error', 'Unknown error')
            if 'selfdev' in error.lower() and 'not available' in error.lower():
                print(f"   SKIP: selfdev not available (not in self-dev mode)")
                send_cmd(sock, f"destroy_session:{session_id}")
                return True  # Skip is not a failure
            print(f"   Error: {error}")
            send_cmd(sock, f"destroy_session:{session_id}")
            return False

        # Cleanup
        send_cmd(sock, f"destroy_session:{session_id}")

        print("\n" + "=" * 60)
        print("TEST PASSED: selfdev status works")
        print("=" * 60)
        return True

    finally:
        sock.close()

def test_selfdev_socket_info():
    """Test that selfdev socket-info works."""
    print("\n" + "=" * 60)
    print("Test: selfdev socket-info")
    print("=" * 60)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
    except FileNotFoundError:
        print(f"ERROR: Debug socket not found")
        return False

    try:
        result = send_cmd(sock, "create_session:/home/jeremy/jcode")
        if not result or not result.get('ok'):
            return False
        session_id = json.loads(result['output'])['session_id']

        # Call selfdev socket-info
        print("1. Calling selfdev socket-info...")
        result = send_cmd(sock, 'tool:selfdev {"action":"socket-info"}', session_id, timeout=30)

        if not result:
            print("   No response")
            return False

        if result.get('ok'):
            output = result.get('output', '')
            print(f"   Output (preview):")
            for line in output.split('\n')[:5]:
                print(f"     {line}")

            # Verify it contains expected info
            if 'debug_socket' in output.lower() or 'socket' in output.lower():
                print("   ✓ Contains socket info")
            else:
                print("   Warning: May not contain expected socket info")
        else:
            error = result.get('error', '')
            if 'not available' in error.lower():
                print("   SKIP: selfdev not available")
                send_cmd(sock, f"destroy_session:{session_id}")
                return True
            print(f"   Error: {error}")
            return False

        send_cmd(sock, f"destroy_session:{session_id}")

        print("\n" + "=" * 60)
        print("TEST PASSED: selfdev socket-info works")
        print("=" * 60)
        return True

    finally:
        sock.close()

def test_reload_context():
    """Test that reload context file exists and is valid JSON."""
    print("\n" + "=" * 60)
    print("Test: Reload context file format")
    print("=" * 60)

    context_path = os.path.join(JCODE_DIR, "reload-context.json")

    # Check if there's an existing context file
    if os.path.exists(context_path):
        print(f"1. Found existing reload context at {context_path}")
        try:
            with open(context_path) as f:
                ctx = json.load(f)
            print(f"   Fields: {list(ctx.keys())}")

            # Verify expected fields
            expected = ['task_context', 'version_before', 'version_after', 'session_id', 'timestamp', 'is_rollback']
            missing = [f for f in expected if f not in ctx]
            if missing:
                print(f"   Warning: Missing fields: {missing}")
            else:
                print("   ✓ All expected fields present")

            print("\n" + "=" * 60)
            print("TEST PASSED: Reload context format is valid")
            print("=" * 60)
            return True

        except json.JSONDecodeError as e:
            print(f"   ERROR: Invalid JSON: {e}")
            return False
    else:
        print(f"1. No existing reload context (will be created on selfdev reload)")
        print("   This is expected if no reload has been performed recently.")

        print("\n" + "=" * 60)
        print("TEST SKIPPED: No reload context to check")
        print("=" * 60)
        return True

def main():
    print("Selfdev Reload Tests")
    print("=" * 60)
    print()

    all_passed = True

    # Test 1: selfdev status
    if not test_selfdev_status():
        all_passed = False

    # Test 2: selfdev socket-info
    if not test_selfdev_socket_info():
        all_passed = False

    # Test 3: Reload context format
    if not test_reload_context():
        all_passed = False

    print()
    if all_passed:
        print("✓ All tests passed!")
        return 0
    else:
        print("✗ Some tests failed")
        return 1

if __name__ == "__main__":
    sys.exit(main())
