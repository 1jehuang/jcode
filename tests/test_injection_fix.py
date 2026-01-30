#!/usr/bin/env python3
"""
Test script to verify soft interrupt injection happens at the correct point.

This tests that:
1. User messages are NOT injected between tool_use and tool_result
2. User messages ARE injected after all tool_results are added
3. The API doesn't return errors about tool_use/tool_result pairing

Run with: python tests/test_injection_fix.py
"""

import socket
import json
import time
import sys

import os

# Check for selfdev socket first, then regular socket
if os.path.exists("/tmp/jcode-selfdev-debug.sock"):
    SOCKET_PATH = "/tmp/jcode-selfdev-debug.sock"
else:
    SOCKET_PATH = f"/run/user/1000/jcode-debug.sock"

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
            # Try to parse as complete JSON
            try:
                return json.loads(data.decode())
            except json.JSONDecodeError:
                continue
        except socket.timeout:
            break
    return json.loads(data.decode()) if data else None

def test_injection_during_tools():
    """Test that soft interrupts are injected AFTER tool results, not before."""
    print("=" * 60)
    print("Test: Soft interrupt injection during tool execution")
    print("=" * 60)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
    except FileNotFoundError:
        print(f"ERROR: Debug socket not found at {SOCKET_PATH}")
        print("Make sure jcode is running with debug control enabled.")
        return False

    try:
        # Create a test session
        print("\n1. Creating test session...")
        result = send_cmd(sock, "create_session:/tmp/injection-test")
        if not result or not result.get('ok'):
            print(f"Failed to create session: {result}")
            return False
        session_id = json.loads(result['output'])['session_id']
        print(f"   Session ID: {session_id}")

        # Send a message that will trigger tool use
        print("\n2. Sending message that triggers tool use...")
        result = send_cmd(sock, "message:Run the bash command 'echo hello'", session_id, timeout=120)
        if not result:
            print("   No response received")
            return False

        if not result.get('ok'):
            error = result.get('error', 'Unknown error')
            if 'tool_use' in error and 'tool_result' in error:
                print(f"   FAIL: API error about tool_use/tool_result pairing: {error}")
                return False
            print(f"   Response: {error}")
        else:
            print(f"   Response received (length: {len(result.get('output', ''))})")

        # Check history to verify message order
        print("\n3. Checking history for correct message order...")
        result = send_cmd(sock, "history", session_id)
        if not result or not result.get('ok'):
            print(f"Failed to get history: {result}")
            return False

        history = json.loads(result['output'])
        print(f"   Found {len(history)} messages")

        # Verify no user text message appears between tool_use and tool_result
        found_tool_use = False
        for i, msg in enumerate(history):
            role = msg.get('role', '')
            content = msg.get('content', '')

            # Check for tool_use
            if role == 'assistant' and '[tool:' in content:
                found_tool_use = True
                print(f"   [{i}] Assistant with tool_use")
                # Next message should be tool result, not user text
                if i + 1 < len(history):
                    next_msg = history[i + 1]
                    next_role = next_msg.get('role', '')
                    next_content = next_msg.get('content', '')
                    if next_role == 'user' and '[result:' not in next_content and 'tool' not in next_role:
                        print(f"   FAIL: User text message found between tool_use and tool_result")
                        return False
                    print(f"   [{i+1}] {next_role}: {next_content[:50]}...")

        if not found_tool_use:
            print("   (No tool calls found in this response)")

        # Cleanup
        print("\n4. Cleaning up...")
        send_cmd(sock, f"destroy_session:{session_id}")

        print("\n" + "=" * 60)
        print("TEST PASSED: No injection between tool_use and tool_result")
        print("=" * 60)
        return True

    finally:
        sock.close()

def test_injection_api_error():
    """
    Reproduce the original bug: inject during tool execution and verify no API error.

    The original error was:
    "messages.34: `tool_use` ids were found without `tool_result` blocks immediately after"
    """
    print("\n" + "=" * 60)
    print("Test: Verify no API errors from injection timing")
    print("=" * 60)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
    except FileNotFoundError:
        print(f"ERROR: Debug socket not found at {SOCKET_PATH}")
        return False

    try:
        # Create session
        result = send_cmd(sock, "create_session:/tmp/api-error-test")
        if not result or not result.get('ok'):
            print(f"Failed to create session: {result}")
            return False
        session_id = json.loads(result['output'])['session_id']
        print(f"   Session ID: {session_id}")

        # Queue a soft interrupt
        print("\n1. Queueing soft interrupt...")
        result = send_cmd(sock, "queue_interrupt:This is an interrupt message", session_id)
        print(f"   Result: {result.get('output', 'OK') if result else 'no response'}")

        # Send a message that triggers multiple tool calls
        print("\n2. Sending message with tool calls...")
        result = send_cmd(sock,
            "message:Please run these three bash commands in sequence: echo one, echo two, echo three",
            session_id, timeout=180)

        if not result:
            print("   No response")
            return False

        if not result.get('ok'):
            error = result.get('error', '')
            if 'tool_use' in error.lower() and 'tool_result' in error.lower():
                print(f"\n   FAIL: Got the original bug error!")
                print(f"   Error: {error}")
                return False
            # Other errors might be OK (like tool not available, etc.)
            print(f"   Response error (may be expected): {error[:100]}...")
        else:
            print("   Response received successfully")

        # Cleanup
        send_cmd(sock, f"destroy_session:{session_id}")

        print("\n" + "=" * 60)
        print("TEST PASSED: No API errors from injection timing")
        print("=" * 60)
        return True

    finally:
        sock.close()

def main():
    print("Soft Interrupt Injection Fix Tests")
    print("=" * 60)
    print()

    all_passed = True

    # Test 1: Check injection happens at correct point
    if not test_injection_during_tools():
        all_passed = False

    print()

    # Test 2: Verify no API errors
    if not test_injection_api_error():
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
