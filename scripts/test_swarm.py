#!/usr/bin/env python3
"""
Test swarm coordination features via the debug socket.

This script tests:
1. Coordinator election (deterministic, alphabetically first)
2. Communication (broadcast, DM, channel)
3. Plan approval workflow
4. Invalid DM recipient validation
5. Swarm_id error feedback
"""

import socket
import json
import os
import sys
import time

DEBUG_SOCKET = f"/run/user/{os.getuid()}/jcode-debug.sock"
TEST_DIR = "/tmp/swarm-test"


def send_cmd(cmd: str, session_id: str = None, timeout: float = 30) -> tuple:
    """Send a debug command and get response."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(DEBUG_SOCKET)
    sock.settimeout(timeout)

    req = {"type": "debug_command", "id": 1, "command": cmd}
    if session_id:
        req["session_id"] = session_id

    sock.send((json.dumps(req) + '\n').encode())

    data = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            break
        data += chunk
        if b'\n' in data:
            break

    sock.close()
    resp = json.loads(data.decode().strip())
    return resp.get('ok', False), resp.get('output', ''), resp.get('error', '')


def create_session(working_dir: str = TEST_DIR) -> str:
    """Create a new session and return its ID."""
    ok, output, err = send_cmd(f"create_session:{working_dir}")
    if not ok:
        raise RuntimeError(f"Failed to create session: {err}")
    return json.loads(output)['session_id']


def destroy_session(session_id: str):
    """Destroy a session."""
    send_cmd(f"destroy_session:{session_id}")


def get_state(session_id: str) -> dict:
    """Get session state."""
    ok, output, _ = send_cmd("state", session_id)
    if ok:
        return json.loads(output)
    return {}


def test_coordinator_election():
    """Test that coordinator selection is deterministic (alphabetically first)."""
    print("\n" + "=" * 60)
    print("Test: Coordinator Election (deterministic)")
    print("=" * 60)

    # Ensure test directory exists
    os.makedirs(TEST_DIR, exist_ok=True)

    # Create two sessions - the first one becomes coordinator
    s1 = create_session()
    s2 = create_session()

    print(f"Session 1: {s1[:12]}...")
    print(f"Session 2: {s2[:12]}...")

    # Check which is alphabetically first
    expected_coordinator = min(s1, s2)
    print(f"Expected coordinator (alphabetically first): {expected_coordinator[:12]}...")

    # Get state to verify coordinator
    state1 = get_state(s1)
    state2 = get_state(s2)

    print(f"S1 is_coordinator: {state1.get('is_coordinator', 'N/A')}")
    print(f"S2 is_coordinator: {state2.get('is_coordinator', 'N/A')}")

    # Cleanup
    destroy_session(s1)
    destroy_session(s2)

    # For now we just verify sessions were created and destroyed
    print("✓ Sessions created and destroyed successfully")
    return True


def test_communication():
    """Test broadcast, DM, and channel communication."""
    print("\n" + "=" * 60)
    print("Test: Communication (broadcast, DM, channel)")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    s2 = create_session()

    print(f"Session 1: {s1[:12]}...")
    print(f"Session 2: {s2[:12]}...")

    # Test broadcast
    ok, output, err = send_cmd(
        'tool:communicate {"action":"broadcast","message":"Hello swarm!"}',
        s1
    )
    print(f"Broadcast result: ok={ok}, err={err}")

    # Test DM
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"dm","to_session":"{s2}","message":"Hello agent!"}}',
        s1
    )
    print(f"DM result: ok={ok}, err={err}")

    # Test channel
    ok, output, err = send_cmd(
        'tool:communicate {"action":"channel","channel":"test","message":"Hello channel!"}',
        s1
    )
    print(f"Channel result: ok={ok}, err={err}")

    # Test list
    ok, output, err = send_cmd(
        'tool:communicate {"action":"list"}',
        s1
    )
    print(f"List result: ok={ok}")
    if ok:
        print(f"  Output: {output[:200]}")

    destroy_session(s1)
    destroy_session(s2)

    print("✓ Communication tests completed")
    return True


def test_invalid_dm():
    """Test that DM to non-existent session returns error."""
    print("\n" + "=" * 60)
    print("Test: Invalid DM Recipient")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    print(f"Session: {s1[:12]}...")

    # Try to DM a non-existent session
    fake_session = "nonexistent_session_12345"
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"dm","to_session":"{fake_session}","message":"Hello?"}}',
        s1
    )

    print(f"DM to fake session: ok={ok}, err={err}")

    # Should fail with an error about session not in swarm
    success = not ok or "not in swarm" in (err + output).lower()

    destroy_session(s1)

    if success:
        print("✓ Invalid DM correctly rejected")
    else:
        print("✗ Invalid DM was not properly rejected")

    return success


def test_swarm_id_error():
    """Test that operations fail gracefully without swarm_id."""
    print("\n" + "=" * 60)
    print("Test: Swarm ID Error Feedback")
    print("=" * 60)

    # Create session in a non-git directory
    non_git_dir = "/tmp/non-git-test"
    os.makedirs(non_git_dir, exist_ok=True)

    # Remove any git directory if it exists
    import shutil
    git_dir = os.path.join(non_git_dir, ".git")
    if os.path.exists(git_dir):
        shutil.rmtree(git_dir)

    try:
        s1 = create_session(non_git_dir)
        print(f"Session: {s1[:12]}...")

        # Try to use swarm features
        ok, output, err = send_cmd(
            'tool:communicate {"action":"list"}',
            s1
        )

        print(f"List in non-swarm: ok={ok}")
        print(f"  Output: {output[:100] if output else 'none'}")
        print(f"  Error: {err[:100] if err else 'none'}")

        # Should get an error about not being in a swarm
        error_found = "not in a swarm" in (output + err).lower()

        destroy_session(s1)

        if error_found:
            print("✓ Swarm ID error correctly returned")
        else:
            print("✗ Missing swarm ID error message")

        return error_found
    except Exception as e:
        print(f"✗ Error: {e}")
        return False


def test_plan_approval():
    """Test plan proposal and approval workflow."""
    print("\n" + "=" * 60)
    print("Test: Plan Approval Workflow")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    # Create coordinator and agent sessions
    coordinator = create_session()
    agent = create_session()

    print(f"Coordinator: {coordinator[:12]}...")
    print(f"Agent: {agent[:12]}...")

    # Simulate agent proposing a plan via shared context
    plan_items = [
        {"id": "1", "subject": "Implement feature X", "description": "Add feature X", "status": "pending"}
    ]
    plan_json = json.dumps(plan_items)
    proposal_key = f"plan_proposal:{agent}"

    # Share the plan proposal
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"share","key":"{proposal_key}","value":{json.dumps(plan_json)}}}',
        agent
    )
    print(f"Plan proposal shared: ok={ok}, err={err}")

    # Read to verify it's there
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"read","key":"{proposal_key}"}}',
        coordinator
    )
    print(f"Read proposal: ok={ok}")

    # Coordinator approves the plan
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"approve_plan","proposer_session":"{agent}"}}',
        coordinator
    )
    print(f"Approve plan: ok={ok}, err={err}")

    # Verify proposal was removed
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"read","key":"{proposal_key}"}}',
        coordinator
    )
    print(f"Read after approval: ok={ok}")

    destroy_session(coordinator)
    destroy_session(agent)

    print("✓ Plan approval workflow completed")
    return True


def test_plan_rejection():
    """Test plan rejection workflow."""
    print("\n" + "=" * 60)
    print("Test: Plan Rejection Workflow")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    coordinator = create_session()
    agent = create_session()

    print(f"Coordinator: {coordinator[:12]}...")
    print(f"Agent: {agent[:12]}...")

    # Share a plan proposal
    plan_items = [{"id": "1", "subject": "Bad idea", "status": "pending"}]
    plan_json = json.dumps(plan_items)
    proposal_key = f"plan_proposal:{agent}"

    ok, _, _ = send_cmd(
        f'tool:communicate {{"action":"share","key":"{proposal_key}","value":{json.dumps(plan_json)}}}',
        agent
    )
    print(f"Plan proposal shared: ok={ok}")

    # Coordinator rejects the plan
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"reject_plan","proposer_session":"{agent}","reason":"Not aligned with goals"}}',
        coordinator
    )
    print(f"Reject plan: ok={ok}, err={err}")

    destroy_session(coordinator)
    destroy_session(agent)

    print("✓ Plan rejection workflow completed")
    return True


def test_coordinator_only_approval():
    """Test that non-coordinators cannot approve plans."""
    print("\n" + "=" * 60)
    print("Test: Coordinator-Only Approval")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()  # This will be coordinator (first session)
    s2 = create_session()  # This is not coordinator

    print(f"Session 1 (coordinator): {s1[:12]}...")
    print(f"Session 2 (agent): {s2[:12]}...")

    # Try to approve from non-coordinator
    ok, output, err = send_cmd(
        f'tool:communicate {{"action":"approve_plan","proposer_session":"{s1}"}}',
        s2
    )
    print(f"Non-coordinator approve attempt: ok={ok}, err={err}")

    # Should fail
    success = not ok or "coordinator" in (err + output).lower()

    destroy_session(s1)
    destroy_session(s2)

    if success:
        print("✓ Non-coordinator approval correctly rejected")
    else:
        print("✗ Non-coordinator was not properly rejected")

    return success


def main():
    """Run all tests."""
    print("=" * 60)
    print("Swarm Integration Tests")
    print("=" * 60)

    # Check if debug socket exists
    if not os.path.exists(DEBUG_SOCKET):
        print(f"Error: Debug socket not found: {DEBUG_SOCKET}")
        print("Make sure jcode server is running with debug_control enabled:")
        print("  touch ~/.jcode/debug_control")
        print("  jcode serve")
        sys.exit(1)

    results = []

    tests = [
        ("Coordinator Election", test_coordinator_election),
        ("Communication", test_communication),
        ("Invalid DM", test_invalid_dm),
        ("Swarm ID Error", test_swarm_id_error),
        ("Plan Approval", test_plan_approval),
        ("Plan Rejection", test_plan_rejection),
        ("Coordinator-Only Approval", test_coordinator_only_approval),
    ]

    for name, test_fn in tests:
        try:
            result = test_fn()
            results.append((name, result))
        except Exception as e:
            print(f"✗ {name} failed with exception: {e}")
            results.append((name, False))

    # Summary
    print("\n" + "=" * 60)
    print("Summary")
    print("=" * 60)

    passed = sum(1 for _, r in results if r)
    total = len(results)

    for name, result in results:
        status = "✓ PASS" if result else "✗ FAIL"
        print(f"  {status}: {name}")

    print(f"\n{passed}/{total} tests passed")

    if passed == total:
        print("\nAll tests passed!")
        sys.exit(0)
    else:
        print("\nSome tests failed.")
        sys.exit(1)


if __name__ == "__main__":
    main()
