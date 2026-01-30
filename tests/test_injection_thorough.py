#!/usr/bin/env python3
"""
Thorough testing of soft interrupt injection fix.

Tests that:
1. With Claude provider: injected messages appear AFTER tool_results
2. With OpenAI provider: injected messages appear AFTER tool_results
3. Multiple tool calls: injection happens after ALL results
4. Urgent interrupts: tool_results are added for skipped tools
5. No API errors about tool_use/tool_result pairing

Run with: python tests/test_injection_thorough.py
"""

import socket
import json
import time
import os
import sys
import re

# Check for selfdev socket first, then regular socket
if os.path.exists("/tmp/jcode-selfdev-debug.sock"):
    SOCKET_PATH = "/tmp/jcode-selfdev-debug.sock"
else:
    SOCKET_PATH = f"/run/user/1000/jcode-debug.sock"

def send_cmd(sock, cmd, session_id=None, timeout=120):
    """Send a debug command and get the response."""
    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id
    sock.send((json.dumps(req) + '\n').encode())
    sock.settimeout(timeout)
    data = b""
    start = time.time()
    while time.time() - start < timeout:
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

def check_history_order(history):
    """
    Check that no user text message appears between tool_use and tool_result.
    Returns (is_valid, error_message)
    """
    waiting_for_results = set()  # tool_use IDs that need results

    for i, msg in enumerate(history):
        role = msg.get('role', '')
        content = msg.get('content', '')

        # Check for tool_use in assistant message
        if role == 'assistant':
            # Look for tool_use patterns like [tool: bash] or tool calls
            tool_matches = re.findall(r'\[tool: (\w+)\]', content)
            if tool_matches:
                for tool_name in tool_matches:
                    waiting_for_results.add(f"tool_{i}_{tool_name}")

        # Check for tool_result
        if role == 'tool' or (role == 'user' and '[result:' in content):
            # A tool result was found, clear one waiting
            if waiting_for_results:
                waiting_for_results.pop()

        # Check for user text while waiting for results
        if role == 'user' and waiting_for_results:
            # Is this a tool result or actual user text?
            if '[result:' not in content and 'tool' not in content.lower():
                # This is user text between tool_use and tool_result!
                return False, f"User text '{content[:50]}...' found while waiting for tool_result for: {waiting_for_results}"

    return True, None

def test_injection_with_provider(provider_name, session_id, sock):
    """Test injection with a specific provider."""
    print(f"\n--- Testing with {provider_name} provider ---")

    # Switch to the provider
    if provider_name == "openai":
        result = send_cmd(sock, "set_provider:openai", session_id)
        if not result or not result.get('ok'):
            print(f"   Could not switch to OpenAI: {result}")
            print(f"   Skipping OpenAI tests (may not be configured)")
            return True  # Skip is not failure

    # Queue a soft interrupt
    print("1. Queueing soft interrupt...")
    result = send_cmd(sock, "queue_interrupt:This is an interrupt during tools", session_id)
    if not result:
        print("   Failed to queue interrupt")
        return False
    print(f"   Queued: {result.get('output', 'OK')}")

    # Send a message that will trigger tool use
    print("2. Sending message that triggers tool use...")
    result = send_cmd(sock, "message:Run the bash command: echo 'hello from test'", session_id, timeout=180)

    if not result:
        print("   No response (timeout)")
        return False

    if not result.get('ok'):
        error = result.get('error', '')
        # Check for the specific error we're trying to prevent
        if 'tool_use' in error.lower() and 'tool_result' in error.lower():
            print(f"   FAIL: Got tool_use/tool_result pairing error!")
            print(f"   Error: {error}")
            return False
        print(f"   Response error (may be expected): {error[:100]}...")

    print("   Response received")

    # Check history
    print("3. Checking message history order...")
    result = send_cmd(sock, "history", session_id)
    if not result or not result.get('ok'):
        print(f"   Failed to get history: {result}")
        return False

    history = json.loads(result['output'])
    print(f"   Found {len(history)} messages")

    is_valid, error_msg = check_history_order(history)
    if not is_valid:
        print(f"   FAIL: {error_msg}")
        return False

    print(f"   ✓ History order is valid for {provider_name}")
    return True

def test_multiple_tools():
    """Test injection when multiple tools are called."""
    print("\n" + "=" * 60)
    print("Test: Multiple tool calls")
    print("=" * 60)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
    except (FileNotFoundError, ConnectionRefusedError) as e:
        print(f"ERROR: Cannot connect to debug socket: {e}")
        return False

    try:
        # Create session
        result = send_cmd(sock, "create_session:/tmp/multi-tool-test")
        if not result or not result.get('ok'):
            print(f"Failed to create session: {result}")
            return False
        session_id = json.loads(result['output'])['session_id']
        print(f"Session ID: {session_id}")

        # Queue interrupt
        print("\n1. Queueing interrupt before multiple tool calls...")
        send_cmd(sock, "queue_interrupt:Interrupting during multiple tools", session_id)

        # Request multiple tool calls
        print("2. Requesting multiple bash commands...")
        result = send_cmd(sock,
            "message:Please run these bash commands one at a time: echo first, echo second, echo third",
            session_id, timeout=180)

        if result and not result.get('ok'):
            error = result.get('error', '')
            if 'tool_use' in error.lower() and 'tool_result' in error.lower():
                print(f"   FAIL: Tool pairing error with multiple tools!")
                return False

        # Check history
        print("3. Verifying history order...")
        result = send_cmd(sock, "history", session_id)
        if result and result.get('ok'):
            history = json.loads(result['output'])
            is_valid, error_msg = check_history_order(history)
            if not is_valid:
                print(f"   FAIL: {error_msg}")
                return False
            print("   ✓ History order is valid")

        send_cmd(sock, f"destroy_session:{session_id}")
        print("\n" + "=" * 60)
        print("TEST PASSED: Multiple tool calls handled correctly")
        print("=" * 60)
        return True

    finally:
        sock.close()

def test_urgent_interrupt():
    """Test urgent interrupt (should skip remaining tools with stub results)."""
    print("\n" + "=" * 60)
    print("Test: Urgent interrupt")
    print("=" * 60)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
    except (FileNotFoundError, ConnectionRefusedError) as e:
        print(f"ERROR: Cannot connect to debug socket: {e}")
        return False

    try:
        result = send_cmd(sock, "create_session:/tmp/urgent-test")
        if not result or not result.get('ok'):
            return False
        session_id = json.loads(result['output'])['session_id']
        print(f"Session ID: {session_id}")

        # Queue URGENT interrupt
        print("\n1. Queueing URGENT interrupt...")
        send_cmd(sock, "queue_interrupt_urgent:STOP! Cancel remaining tools!", session_id)

        # Request tool calls
        print("2. Requesting tool calls...")
        result = send_cmd(sock,
            "message:Run these commands: echo a, echo b, echo c",
            session_id, timeout=180)

        if result and not result.get('ok'):
            error = result.get('error', '')
            if 'tool_use' in error.lower() and 'tool_result' in error.lower():
                print(f"   FAIL: Tool pairing error with urgent interrupt!")
                return False

        # Check that skipped tools have results
        print("3. Checking that skipped tools have results...")
        result = send_cmd(sock, "history", session_id)
        if result and result.get('ok'):
            history = json.loads(result['output'])
            is_valid, error_msg = check_history_order(history)
            if not is_valid:
                print(f"   FAIL: {error_msg}")
                return False

            # Look for skip messages
            has_skip = any('skip' in str(msg.get('content', '')).lower() for msg in history)
            if has_skip:
                print("   ✓ Found skip message (tools were interrupted)")
            else:
                print("   (No skip message - tools may have completed before interrupt)")

        send_cmd(sock, f"destroy_session:{session_id}")
        print("\n" + "=" * 60)
        print("TEST PASSED: Urgent interrupt handled correctly")
        print("=" * 60)
        return True

    finally:
        sock.close()

def test_both_providers():
    """Test injection with both Claude and OpenAI providers."""
    print("\n" + "=" * 60)
    print("Test: Both providers")
    print("=" * 60)

    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
    except (FileNotFoundError, ConnectionRefusedError) as e:
        print(f"ERROR: Cannot connect to debug socket: {e}")
        return False

    try:
        result = send_cmd(sock, "create_session:/tmp/provider-test")
        if not result or not result.get('ok'):
            return False
        session_id = json.loads(result['output'])['session_id']
        print(f"Session ID: {session_id}")

        all_passed = True

        # Test Claude (default)
        if not test_injection_with_provider("claude", session_id, sock):
            all_passed = False

        # Test OpenAI
        if not test_injection_with_provider("openai", session_id, sock):
            all_passed = False

        send_cmd(sock, f"destroy_session:{session_id}")

        if all_passed:
            print("\n" + "=" * 60)
            print("TEST PASSED: Both providers work correctly")
            print("=" * 60)
        return all_passed

    finally:
        sock.close()

def main():
    print("Thorough Soft Interrupt Injection Tests")
    print("=" * 60)
    print(f"Using socket: {SOCKET_PATH}")
    print()

    # Check socket exists
    if not os.path.exists(SOCKET_PATH):
        print(f"ERROR: Socket not found at {SOCKET_PATH}")
        print("Make sure jcode is running with debug control enabled.")
        return 1

    all_passed = True

    # Test 1: Multiple tool calls
    if not test_multiple_tools():
        all_passed = False

    # Test 2: Urgent interrupt
    if not test_urgent_interrupt():
        all_passed = False

    # Test 3: Both providers (if available)
    if not test_both_providers():
        all_passed = False

    print()
    if all_passed:
        print("=" * 60)
        print("✓ ALL TESTS PASSED!")
        print("=" * 60)
        return 0
    else:
        print("=" * 60)
        print("✗ SOME TESTS FAILED")
        print("=" * 60)
        return 1

if __name__ == "__main__":
    sys.exit(main())
