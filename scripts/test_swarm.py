#!/usr/bin/env python3
"""
Test swarm coordination features via the debug socket.

Uses debug commands directly (not tool:communicate) to avoid
blocking I/O issues with the main socket.

Tests:
1. Coordinator election (first-created session gets coordinator)
2. Communication (broadcast, DM via debug commands)
3. Invalid DM recipient validation
4. Swarm_id for non-git directories
5. Plan approval workflow
6. Plan rejection workflow
7. Coordinator-only approval enforcement
"""

import socket
import json
import os
import sys
import time

DEBUG_SOCKET = f"/run/user/{os.getuid()}/jcode-debug.sock"
TEST_DIR = "/tmp/swarm-test"


def send_cmd(cmd: str, session_id: str = None, timeout: float = 30) -> tuple:
    """Send a debug command and get response. Returns (ok, output, error)."""
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
        raise RuntimeError(f"Failed to create session: {err or output}")
    return json.loads(output)['session_id']


def destroy_session(session_id: str):
    """Destroy a session."""
    send_cmd(f"destroy_session:{session_id}")


def get_swarm_id(path: str = TEST_DIR) -> str:
    """Get the swarm_id for a directory."""
    ok, output, _ = send_cmd(f"swarm:id:{path}")
    if ok:
        return json.loads(output).get('swarm_id', '')
    return ''


def get_coordinator(swarm_id: str) -> str:
    """Get the coordinator session_id for a swarm."""
    ok, output, _ = send_cmd("swarm:coordinators")
    if ok:
        coords = json.loads(output)
        for c in coords:
            if c['swarm_id'] == swarm_id:
                return c['coordinator_session']
    return ''


def test_coordinator_election():
    """Test that the first-created session becomes coordinator."""
    print("\n" + "=" * 60)
    print("Test: Coordinator Election")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    s2 = create_session()
    swarm_id = get_swarm_id()

    print(f"Session 1 (first): {s1[:20]}...")
    print(f"Session 2 (second): {s2[:20]}...")

    # First-created session should be coordinator
    actual_coordinator = get_coordinator(swarm_id)
    print(f"Actual coordinator: {actual_coordinator[:20]}...")

    success = actual_coordinator == s1
    if success:
        print("✓ First-created session is the coordinator")
    else:
        print("✗ Coordinator is not the first-created session")

    # Also verify via swarm:roles
    ok, output, _ = send_cmd("swarm:roles")
    if ok:
        roles = json.loads(output)
        coord_roles = [r for r in roles if r.get('is_coordinator')]
        if coord_roles:
            print(f"  Role-based coordinator: {coord_roles[0]['session_id'][:20]}...")
        else:
            print("  Warning: No coordinator found via swarm:roles")

    destroy_session(s1)
    destroy_session(s2)
    return success


def test_communication():
    """Test broadcast and DM communication via debug commands."""
    print("\n" + "=" * 60)
    print("Test: Communication (broadcast, DM, members)")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    s2 = create_session()
    swarm_id = get_swarm_id()

    print(f"Session 1: {s1[:20]}...")
    print(f"Session 2: {s2[:20]}...")

    success = True

    # Test broadcast
    ok, output, err = send_cmd(f"swarm:broadcast:{swarm_id} Hello swarm!", s1)
    print(f"Broadcast: ok={ok}")
    if ok:
        data = json.loads(output)
        print(f"  Sent to {data.get('sent_to', 0)} members")
    else:
        print(f"  Error: {err or output}")
        success = False

    # Test DM (notify)
    ok, output, err = send_cmd(f"swarm:notify:{s2} Hello agent!", s1)
    print(f"DM: ok={ok}")
    if ok:
        data = json.loads(output)
        print(f"  Sent to: {data.get('sent_to', '')[:20]}...")
    else:
        print(f"  Error: {err or output}")
        success = False

    # Test list members
    ok, output, err = send_cmd("swarm:members")
    print(f"Members list: ok={ok}")
    if ok:
        members = json.loads(output)
        member_ids = [m['session_id'] for m in members]
        if s1 in member_ids and s2 in member_ids:
            print(f"  Both sessions found in {len(members)} members")
        else:
            print("  ✗ Missing sessions in member list")
            success = False

    destroy_session(s1)
    destroy_session(s2)

    if success:
        print("✓ Communication tests passed")
    else:
        print("✗ Communication tests had failures")
    return success


def test_invalid_dm():
    """Test that DM to non-existent session returns error."""
    print("\n" + "=" * 60)
    print("Test: Invalid DM Recipient")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    print(f"Session: {s1[:20]}...")

    fake_session = "nonexistent_session_12345"
    ok, output, err = send_cmd(f"swarm:notify:{fake_session} Hello?", s1)

    print(f"DM to fake session: ok={ok}")
    combined = (err + output).lower()
    if not ok:
        print(f"  Error (expected): {output[:80]}")

    success = not ok and ("unknown session" in combined or "not in swarm" in combined)

    destroy_session(s1)

    if success:
        print("✓ Invalid DM correctly rejected")
    else:
        print("✗ Invalid DM was not properly rejected")
    return success


def test_swarm_id_non_git():
    """Test that non-git directories get a raw path swarm_id (not .git-based)."""
    print("\n" + "=" * 60)
    print("Test: Non-Git Directory Swarm ID")
    print("=" * 60)

    non_git_dir = "/tmp/non-git-test"
    os.makedirs(non_git_dir, exist_ok=True)

    import shutil
    git_dir = os.path.join(non_git_dir, ".git")
    if os.path.exists(git_dir):
        shutil.rmtree(git_dir)

    try:
        s1 = create_session(non_git_dir)
        print(f"Session: {s1[:20]}...")

        # Check swarm:id — non-git dirs get raw path, is_git_repo=false
        ok, output, _ = send_cmd(f"swarm:id:{non_git_dir}")
        print(f"Swarm ID check: ok={ok}")
        not_git = False
        if ok:
            data = json.loads(output)
            not_git = data.get('is_git_repo') == False
            print(f"  swarm_id: {data.get('swarm_id')}")
            print(f"  is_git_repo: {data.get('is_git_repo')}")
            print(f"  source: {data.get('source')}")

        # Verify the session's swarm_id doesn't contain .git
        ok2, output2, _ = send_cmd(f"swarm:session:{s1}")
        no_git_in_swarm = False
        if ok2:
            sess_data = json.loads(output2)
            swarm_id = sess_data.get('swarm_id') or ''
            no_git_in_swarm = '.git' not in swarm_id
            print(f"  Session swarm_id: {swarm_id}")

        destroy_session(s1)

        success = not_git and no_git_in_swarm
        if success:
            print("✓ Non-git directory correctly identified")
        else:
            print("✗ Non-git directory handling incorrect")
        return success

    except Exception as e:
        print(f"✗ Error: {e}")
        return False


def test_plan_approval():
    """Test plan proposal and approval workflow."""
    print("\n" + "=" * 60)
    print("Test: Plan Approval Workflow")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    s2 = create_session()
    swarm_id = get_swarm_id()
    coordinator = get_coordinator(swarm_id)
    agent = s2 if coordinator == s1 else s1

    print(f"Coordinator: {coordinator[:20]}...")
    print(f"Agent: {agent[:20]}...")

    success = True

    # Get plan item count before approval
    ok, output, _ = send_cmd(f"swarm:plan_version:{swarm_id}")
    items_before = 0
    if ok:
        items_before = json.loads(output).get('item_count', 0)
        print(f"Plan items before: {items_before}")

    # Agent proposes a plan via shared context
    plan_items = [
        {"id": "approval_test_1", "content": "Implement feature X", "status": "pending", "priority": "normal"}
    ]
    plan_json = json.dumps(plan_items)
    proposal_key = f"plan_proposal:{agent}"

    ok, output, err = send_cmd(
        f"swarm:set_context:{agent} {proposal_key} {plan_json}"
    )
    print(f"Plan proposal shared: ok={ok}")
    if not ok:
        print(f"  Error: {err or output}")
        success = False

    # Verify proposal is in context
    ok, output, err = send_cmd(f"swarm:context:{swarm_id}:{proposal_key}")
    print(f"Read proposal: ok={ok}")
    if not ok:
        print(f"  Error: {err or output}")
        success = False

    # Coordinator approves
    ok, output, err = send_cmd(f"swarm:approve_plan:{coordinator} {agent}")
    print(f"Approve plan: ok={ok}")
    if ok:
        data = json.loads(output)
        print(f"  Items added: {data.get('items_added')}")
        print(f"  Plan version: {data.get('plan_version')}")
    else:
        print(f"  Error: {err or output}")
        success = False

    # Verify proposal was removed from context
    ok, _, _ = send_cmd(f"swarm:context:{swarm_id}:{proposal_key}")
    proposal_removed = not ok
    print(f"Proposal removed after approval: {proposal_removed}")
    if not proposal_removed:
        print("  ✗ Proposal still exists after approval")
        success = False

    # Verify plan grew
    ok, output, _ = send_cmd(f"swarm:plan_version:{swarm_id}")
    if ok:
        data = json.loads(output)
        items_after = data.get('item_count', 0)
        print(f"Plan items after: {items_after} (was {items_before})")
        if items_after <= items_before:
            print("  ✗ Plan did not grow after approval")
            success = False

    destroy_session(s1)
    destroy_session(s2)

    if success:
        print("✓ Plan approval workflow completed successfully")
    else:
        print("✗ Plan approval workflow had failures")
    return success


def test_plan_rejection():
    """Test plan rejection workflow."""
    print("\n" + "=" * 60)
    print("Test: Plan Rejection Workflow")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    s2 = create_session()
    swarm_id = get_swarm_id()
    coordinator = get_coordinator(swarm_id)
    agent = s2 if coordinator == s1 else s1

    print(f"Coordinator: {coordinator[:20]}...")
    print(f"Agent: {agent[:20]}...")

    success = True

    # Get plan version before
    ok, output, _ = send_cmd(f"swarm:plan_version:{swarm_id}")
    version_before = 0
    if ok:
        version_before = json.loads(output).get('version', 0)

    # Share a plan proposal
    plan_items = [{"id": "reject_test_1", "content": "Bad idea", "status": "pending", "priority": "normal"}]
    plan_json = json.dumps(plan_items)
    proposal_key = f"plan_proposal:{agent}"

    ok, _, err = send_cmd(f"swarm:set_context:{agent} {proposal_key} {plan_json}")
    print(f"Plan proposal shared: ok={ok}")
    if not ok:
        print(f"  Error: {err}")
        success = False

    # Coordinator rejects the plan
    ok, output, err = send_cmd(
        f"swarm:reject_plan:{coordinator} {agent} Not aligned with goals"
    )
    print(f"Reject plan: ok={ok}")
    if ok:
        data = json.loads(output)
        print(f"  Rejected: {data.get('rejected')}")
    else:
        print(f"  Error: {err or output}")
        success = False

    # Verify proposal was removed
    ok, _, _ = send_cmd(f"swarm:context:{swarm_id}:{proposal_key}")
    proposal_removed = not ok
    print(f"Proposal removed after rejection: {proposal_removed}")
    if not proposal_removed:
        success = False

    # Verify plan version didn't change (rejected plans don't modify the plan)
    ok, output, _ = send_cmd(f"swarm:plan_version:{swarm_id}")
    if ok:
        version_after = json.loads(output).get('version', 0)
        plan_unchanged = version_after == version_before
        print(f"Plan version unchanged: {plan_unchanged} ({version_before} → {version_after})")
        if not plan_unchanged:
            success = False

    destroy_session(s1)
    destroy_session(s2)

    if success:
        print("✓ Plan rejection workflow completed successfully")
    else:
        print("✗ Plan rejection workflow had failures")
    return success


def test_coordinator_only_approval():
    """Test that non-coordinators cannot approve plans."""
    print("\n" + "=" * 60)
    print("Test: Coordinator-Only Approval")
    print("=" * 60)

    os.makedirs(TEST_DIR, exist_ok=True)

    s1 = create_session()
    s2 = create_session()
    swarm_id = get_swarm_id()
    coordinator = get_coordinator(swarm_id)
    non_coordinator = s2 if coordinator == s1 else s1

    print(f"Coordinator: {coordinator[:20]}...")
    print(f"Non-coordinator: {non_coordinator[:20]}...")

    # Try to approve from non-coordinator
    ok, output, err = send_cmd(
        f"swarm:approve_plan:{non_coordinator} {coordinator}"
    )
    print(f"Non-coordinator approve attempt: ok={ok}")
    if not ok:
        print(f"  Error (expected): {output[:80]}")

    combined = (err + output).lower()
    success = not ok and "coordinator" in combined

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
        ("Non-Git Swarm ID", test_swarm_id_non_git),
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
            import traceback
            traceback.print_exc()
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
