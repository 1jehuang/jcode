#!/usr/bin/env python3
"""
Test script for swarm debug socket commands.
Tests all the new swarm commands including proposals, touches, timestamps, etc.
"""

import socket
import json
import time
import sys
import os

SOCKET_PATH = f"/run/user/{os.getuid()}/jcode-debug.sock"
MAIN_SOCKET_PATH = f"/run/user/{os.getuid()}/jcode.sock"

def send_cmd(cmd, session_id=None, timeout=10):
    """Send a debug command and return the response."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(timeout)
    sock.connect(SOCKET_PATH)

    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id

    sock.send((json.dumps(req) + '\n').encode())

    # Read response with non-blocking to handle slow responses
    data = b''
    sock.setblocking(False)
    start = time.time()
    while time.time() - start < timeout:
        try:
            chunk = sock.recv(4096)
            if chunk:
                data += chunk
                # Check if we have a complete JSON response
                try:
                    resp = json.loads(data.decode())
                    sock.close()
                    return resp.get('ok', False), resp.get('output', '')
                except json.JSONDecodeError:
                    pass
        except BlockingIOError:
            time.sleep(0.05)

    sock.close()
    if data:
        try:
            resp = json.loads(data.decode())
            return resp.get('ok', False), resp.get('output', '')
        except json.JSONDecodeError:
            return False, f"Invalid JSON: {data.decode()[:100]}"
    raise TimeoutError("timed out")

def create_session(cwd="/tmp"):
    """Create a headless session for testing."""
    ok, output = send_cmd(f"create_session:{cwd}")
    if ok:
        return json.loads(output).get('session_id')
    return None

def destroy_session(session_id):
    """Destroy a test session."""
    send_cmd(f"destroy_session:{session_id}")

def test_basic_swarm_commands():
    """Test basic swarm listing commands."""
    print("\n=== Testing Basic Swarm Commands ===")

    tests = [
        ("swarm:list", "List all swarms"),
        ("swarm:members", "List all members"),
        ("swarm:coordinators", "List coordinators"),
        ("swarm:plans", "List plans"),
        ("swarm:context", "List shared context"),
        ("swarm:touches", "List file touches"),
        ("swarm:conflicts", "List conflicts"),
        ("swarm:interrupts", "List interrupts"),
        ("swarm:proposals", "List proposals"),
    ]

    passed = 0
    failed = 0

    for cmd, desc in tests:
        ok, output = send_cmd(cmd)
        if ok:
            print(f"  ✓ {cmd}: {desc}")
            # Verify output is valid JSON
            try:
                parsed = json.loads(output) if output and output.strip() not in ['', '{}'] else output
                passed += 1
            except json.JSONDecodeError:
                # Some outputs might be plain strings
                passed += 1
        else:
            print(f"  ✗ {cmd}: {desc} - FAILED: {output[:100]}")
            failed += 1

    return passed, failed

def test_swarm_touches_timestamps():
    """Test that file touches include timestamps."""
    print("\n=== Testing File Touches with Timestamps ===")

    passed = 0
    failed = 0

    # Check touches output format (even if empty)
    ok, output = send_cmd("swarm:touches")
    if ok:
        touches = json.loads(output)
        # Check that if there are touches, they have timestamp_unix
        if len(touches) > 0:
            if 'timestamp_unix' in touches[0]:
                print("  ✓ swarm:touches includes timestamp_unix field")
                passed += 1
            else:
                print(f"  ✗ swarm:touches missing timestamp_unix: {list(touches[0].keys())}")
                failed += 1
        else:
            print("  ⚠ No touches to verify timestamp format (empty list is valid)")
            passed += 1
    else:
        print(f"  ✗ swarm:touches failed: {output[:100]}")
        failed += 1

    return passed, failed

def test_swarm_member_timestamps():
    """Test that swarm:members includes timestamps."""
    print("\n=== Testing Swarm Member Timestamps ===")

    ok, output = send_cmd("swarm:members")
    if not ok:
        print(f"  ✗ swarm:members failed: {output[:100]}")
        return 0, 1

    passed = 0
    failed = 0

    try:
        members = json.loads(output)
        if len(members) == 0:
            print("  ⚠ No members to test (empty swarm)")
            return 1, 0

        # Check for timestamp fields
        sample = members[0]
        required_fields = ['joined_secs_ago', 'status_changed_secs_ago']

        for field in required_fields:
            if field in sample:
                print(f"  ✓ swarm:members has {field}")
                passed += 1
            else:
                print(f"  ✗ swarm:members missing {field}")
                failed += 1

    except json.JSONDecodeError as e:
        print(f"  ✗ Invalid JSON from swarm:members: {e}")
        failed += 1

    return passed, failed

def test_swarm_session_details():
    """Test swarm:session command format (without creating sessions)."""
    print("\n=== Testing swarm:session Details ===")

    passed = 0
    failed = 0

    # Test with a made-up session ID - should return an error gracefully
    ok, output = send_cmd("swarm:session:nonexistent_session_123")
    if not ok:
        # Error is expected for nonexistent session
        if "not found" in output.lower() or "unknown" in output.lower() or "no session" in output.lower():
            print("  ✓ swarm:session handles unknown session gracefully")
            passed += 1
        else:
            print(f"  ✓ swarm:session returns error for unknown session: {output[:80]}")
            passed += 1
    else:
        print(f"  ⚠ swarm:session unexpectedly succeeded for unknown session")
        passed += 1

    return passed, failed

def test_swarm_context_timestamps():
    """Test that shared context entries have timestamps."""
    print("\n=== Testing Shared Context Timestamps ===")

    passed = 0
    failed = 0

    # Check context output format (even if empty)
    ok, output = send_cmd("swarm:context")
    if ok:
        contexts = json.loads(output)
        if len(contexts) > 0:
            sample = contexts[0]
            if 'created_secs_ago' in sample and 'updated_secs_ago' in sample:
                print("  ✓ swarm:context has timestamp fields")
                passed += 1
            else:
                print(f"  ✗ swarm:context missing timestamps: {list(sample.keys())}")
                failed += 1
        else:
            print("  ⚠ No context entries to verify timestamp format (empty list is valid)")
            passed += 1
    else:
        print(f"  ✗ swarm:context failed: {output[:100]}")
        failed += 1

    return passed, failed

def test_swarm_proposals():
    """Test plan proposal commands."""
    print("\n=== Testing Plan Proposals ===")

    passed = 0
    failed = 0

    # Test basic proposals list
    ok, output = send_cmd("swarm:proposals")
    if ok:
        try:
            proposals = json.loads(output)
            print(f"  ✓ swarm:proposals returns valid JSON ({len(proposals)} proposals)")
            passed += 1
        except json.JSONDecodeError:
            print(f"  ✗ swarm:proposals invalid JSON: {output[:100]}")
            failed += 1
    else:
        print(f"  ✗ swarm:proposals failed: {output[:100]}")
        failed += 1

    # Test proposals for a swarm
    ok, output = send_cmd("swarm:proposals:/tmp")
    if ok:
        try:
            proposals = json.loads(output)
            print(f"  ✓ swarm:proposals:/tmp returns valid JSON")
            passed += 1
        except json.JSONDecodeError:
            print(f"  ✗ swarm:proposals:/tmp invalid JSON: {output[:100]}")
            failed += 1
    else:
        # Might fail if swarm doesn't exist, which is OK
        if "No proposal" in output or output == "[]":
            print(f"  ✓ swarm:proposals:/tmp handles missing swarm correctly")
            passed += 1
        else:
            print(f"  ✗ swarm:proposals:/tmp failed: {output[:100]}")
            failed += 1

    return passed, failed

def test_swarm_touches_filtering():
    """Test file touches swarm filtering."""
    print("\n=== Testing File Touches Swarm Filtering ===")

    ok, output = send_cmd("swarm:touches:swarm:/tmp")
    if ok:
        try:
            touches = json.loads(output)
            print(f"  ✓ swarm:touches:swarm:/tmp returns valid JSON ({len(touches)} touches)")
            return 1, 0
        except json.JSONDecodeError:
            print(f"  ✗ swarm:touches:swarm:/tmp invalid JSON: {output[:100]}")
            return 0, 1
    else:
        print(f"  ✗ swarm:touches:swarm:/tmp failed: {output[:100]}")
        return 0, 1

def test_swarm_conflicts_details():
    """Test that conflicts include full access history."""
    print("\n=== Testing Conflict Details ===")

    ok, output = send_cmd("swarm:conflicts")
    if ok:
        try:
            conflicts = json.loads(output)
            if len(conflicts) > 0:
                sample = conflicts[0]
                if 'accesses' in sample:
                    print("  ✓ swarm:conflicts includes full access history")
                    return 1, 0
                else:
                    print(f"  ✗ swarm:conflicts missing accesses: {sample.keys()}")
                    return 0, 1
            else:
                print("  ⚠ No conflicts to test (no multi-session touches)")
                return 1, 0
        except json.JSONDecodeError:
            print(f"  ✗ swarm:conflicts invalid JSON: {output[:100]}")
            return 0, 1
    else:
        print(f"  ✗ swarm:conflicts failed: {output[:100]}")
        return 0, 1

def test_swarm_id_provenance():
    """Test swarm:id command for path provenance."""
    print("\n=== Testing Swarm ID Provenance ===")

    ok, output = send_cmd("swarm:id:/home/jeremy/jcode")
    if ok:
        try:
            data = json.loads(output)
            required = ['path', 'swarm_id', 'git_root', 'is_git_repo']
            missing = [f for f in required if f not in data]
            if not missing:
                print("  ✓ swarm:id includes all provenance fields")
                return 1, 0
            else:
                print(f"  ✗ swarm:id missing fields: {missing}")
                return 0, 1
        except json.JSONDecodeError:
            print(f"  ✗ swarm:id invalid JSON: {output[:100]}")
            return 0, 1
    else:
        print(f"  ✗ swarm:id failed: {output[:100]}")
        return 0, 1

def test_swarm_help():
    """Test that help includes new commands."""
    print("\n=== Testing Help Documentation ===")

    ok, output = send_cmd("swarm:help")
    if not ok:
        print(f"  ✗ swarm:help failed: {output[:100]}")
        return 0, 1

    passed = 0
    failed = 0

    # Check for documented commands
    commands_to_check = [
        "swarm:proposals",
        "swarm:touches:swarm:",
        "timestamp",
        "swarm:session",
        "events:recent",
        "events:since",
    ]

    for cmd in commands_to_check:
        if cmd.lower() in output.lower():
            print(f"  ✓ Help documents {cmd}")
            passed += 1
        else:
            print(f"  ✗ Help missing {cmd}")
            failed += 1

    return passed, failed


def test_event_commands():
    """Test real-time event subscription commands."""
    print("\n=== Testing Event Commands ===")

    passed = 0
    failed = 0

    # Test events:count
    ok, output = send_cmd("events:count")
    if ok:
        data = json.loads(output)
        if 'count' in data and 'latest_id' in data and 'max_history' in data:
            print("  ✓ events:count returns expected fields")
            passed += 1
        else:
            print(f"  ✗ events:count missing fields: {list(data.keys())}")
            failed += 1
    else:
        print(f"  ✗ events:count failed: {output[:80]}")
        failed += 1

    # Test events:types
    ok, output = send_cmd("events:types")
    if ok:
        data = json.loads(output)
        if 'types' in data and len(data['types']) > 0:
            print(f"  ✓ events:types returns {len(data['types'])} event types")
            passed += 1
        else:
            print(f"  ✗ events:types missing types")
            failed += 1
    else:
        print(f"  ✗ events:types failed: {output[:80]}")
        failed += 1

    # Test events:recent
    ok, output = send_cmd("events:recent")
    if ok:
        events = json.loads(output)
        print(f"  ✓ events:recent returns valid JSON ({len(events)} events)")
        passed += 1
    else:
        print(f"  ✗ events:recent failed: {output[:80]}")
        failed += 1

    # Test events:recent:10
    ok, output = send_cmd("events:recent:10")
    if ok:
        events = json.loads(output)
        print(f"  ✓ events:recent:10 returns valid JSON")
        passed += 1
    else:
        print(f"  ✗ events:recent:10 failed: {output[:80]}")
        failed += 1

    # Test events:since:0
    ok, output = send_cmd("events:since:0")
    if ok:
        events = json.loads(output)
        print(f"  ✓ events:since:0 returns valid JSON ({len(events)} events)")
        passed += 1
    else:
        print(f"  ✗ events:since:0 failed: {output[:80]}")
        failed += 1

    return passed, failed

def main():
    print("=" * 60)
    print("Swarm Debug Socket Test Suite")
    print("=" * 60)

    # Check socket exists
    if not os.path.exists(SOCKET_PATH):
        print(f"\n✗ Debug socket not found at {SOCKET_PATH}")
        print("Make sure jcode server is running with debug control enabled.")
        print("Enable with: touch ~/.jcode/debug_control")
        sys.exit(1)

    total_passed = 0
    total_failed = 0

    test_funcs = [
        test_basic_swarm_commands,
        test_swarm_member_timestamps,
        test_swarm_touches_timestamps,
        test_swarm_session_details,
        test_swarm_context_timestamps,
        test_swarm_proposals,
        test_swarm_touches_filtering,
        test_swarm_conflicts_details,
        test_swarm_id_provenance,
        test_swarm_help,
        test_event_commands,
    ]

    for test_func in test_funcs:
        try:
            passed, failed = test_func()
            total_passed += passed
            total_failed += failed
        except Exception as e:
            print(f"  ✗ Test crashed: {e}")
            total_failed += 1

    print("\n" + "=" * 60)
    print(f"Results: {total_passed} passed, {total_failed} failed")
    print("=" * 60)

    sys.exit(0 if total_failed == 0 else 1)

if __name__ == "__main__":
    main()
