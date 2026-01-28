#!/usr/bin/env python3
"""
Comprehensive memory system test for jcode.

Tests all memory features with both Claude and OpenAI providers via the debug socket.

Usage:
    # With existing server (uses /run/user/1000/jcode-debug.sock)
    ./scripts/test_memory.py

    # Start fresh server for testing
    ./scripts/test_memory.py --fresh

    # Test specific provider only
    ./scripts/test_memory.py --provider claude
"""

import socket
import json
import time
import os
import subprocess
import sys
import re
import argparse

# Colors
GREEN = '\033[92m'
RED = '\033[91m'
YELLOW = '\033[93m'
RESET = '\033[0m'

def log(msg, color=None):
    if color:
        print(f"{color}{msg}{RESET}")
    else:
        print(msg)

def log_pass(msg): log(f"  ✓ {msg}", GREEN)
def log_fail(msg): log(f"  ✗ {msg}", RED)
def log_section(msg): log(f"\n{'='*60}\n{msg}\n{'='*60}", YELLOW)

class DebugSocketClient:
    def __init__(self, socket_path):
        self.socket_path = socket_path
        self.sock = None

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(self.socket_path)
        self.sock.settimeout(120)

    def close(self):
        if self.sock:
            self.sock.close()

    def send(self, cmd, session_id=None):
        req = {"type": "debug_command", "id": 1, "command": cmd}
        if session_id:
            req["session_id"] = session_id
        self.sock.send((json.dumps(req) + '\n').encode())
        data = self.sock.recv(65536).decode()
        resp = json.loads(data)
        return resp.get('ok'), resp.get('output', '')

def run_tests(client, providers):
    results = {"passed": 0, "failed": 0}

    def check(condition, msg):
        if condition:
            log_pass(msg)
            results["passed"] += 1
        else:
            log_fail(msg)
            results["failed"] += 1
        return condition

    for provider in providers:
        log_section(f"Testing with {provider.upper()}")

        # Create session
        ok, output = client.send(f"create_session:/tmp/memory-test-{provider}")
        if not ok:
            log_fail(f"Failed to create session: {output}")
            continue
        session_id = json.loads(output)['session_id']
        check(ok, f"Created session: {session_id}")

        # Switch provider
        ok, output = client.send(f"set_provider:{provider}", session_id)
        check(ok, f"Switched to {provider}")
        if ok:
            state = json.loads(output)
            log(f"       Provider: {state['provider']}, Model: {state['model']}")

        # Test 1: Memory remember with tags
        log("\n  --- Memory Remember ---")
        ok, output = client.send(
            f'tool:memory {{"action":"remember","content":"User prefers {provider} for testing","tags":["test","provider-pref"]}}',
            session_id)
        check(ok and "Remembered" in output, "Remember with tags")

        # Extract memory ID
        result = json.loads(output) if output.startswith('{') else {'output': output}
        match = re.search(r'id: (mem_\d+_\d+)', result.get('output', output))
        mem_id = match.group(1) if match else None
        if mem_id:
            log(f"       Memory ID: {mem_id}")

        # Test 2: Memory list
        log("\n  --- Memory List ---")
        ok, output = client.send('tool:memory {"action":"list"}', session_id)
        check(ok and provider in output, "List shows our memory")

        # Test 3: Memory search
        log("\n  --- Memory Search ---")
        ok, output = client.send(f'tool:memory {{"action":"search","query":"{provider} testing"}}', session_id)
        check(ok, f"Search for '{provider} testing'")

        # Test 4: Memory tag (using correct 'id' field)
        log("\n  --- Memory Tag ---")
        if mem_id:
            ok, output = client.send(
                f'tool:memory {{"action":"tag","id":"{mem_id}","tags":["extra-tag-{provider}"]}}',
                session_id)
            check(ok and "Tagged" in output, f"Added tag to memory")
        else:
            log_fail("No memory ID to tag")
            results["failed"] += 1

        # Test 5: Create second memory and link
        log("\n  --- Memory Link ---")
        ok, output = client.send(
            f'tool:memory {{"action":"remember","content":"Second memory for {provider} link test"}}',
            session_id)
        result = json.loads(output) if output.startswith('{') else {'output': output}
        match2 = re.search(r'id: (mem_\d+_\d+)', result.get('output', output))
        mem_id2 = match2.group(1) if match2 else None

        if mem_id and mem_id2:
            ok, output = client.send(
                f'tool:memory {{"action":"link","from_id":"{mem_id}","to_id":"{mem_id2}","weight":0.75}}',
                session_id)
            check(ok and "Linked" in output, "Linked two memories")
        else:
            log_fail("Missing memory IDs for link")
            results["failed"] += 1

        # Test 6: Memory related (using correct 'id' field)
        log("\n  --- Memory Related (Graph Traversal) ---")
        if mem_id:
            ok, output = client.send(
                f'tool:memory {{"action":"related","id":"{mem_id}","depth":2}}',
                session_id)
            result = json.loads(output) if output.startswith('{') else {'output': output}
            found_related = "Found" in result.get('output', output) or "related" in result.get('output', output).lower()
            check(ok and found_related, "Related memories query via graph")
            if ok:
                log(f"       {result.get('output', output)[:100]}...")
        else:
            log_fail("No memory ID for related query")
            results["failed"] += 1

        # Test 7: Send messages for extraction test
        log("\n  --- Message Exchange ---")
        messages = [
            f"Hello, I'm testing with {provider}",
            "Remember that my favorite editor is vim",
            "What text editors do you know about?",
            "Thanks for the information!"
        ]
        all_ok = True
        for i, msg in enumerate(messages):
            ok, output = client.send(f"message:{msg}", session_id)
            if not ok:
                all_ok = False
                log_fail(f"Message {i+1} failed: {output[:50]}")
        check(all_ok, f"Sent {len(messages)} messages")

        # Test 8: Trigger extraction
        log("\n  --- Trigger Extraction ---")
        ok, output = client.send("trigger_extraction", session_id)
        check(ok, "Trigger extraction")
        if ok:
            result = json.loads(output)
            log(f"       Extracted: {result.get('extracted', 0)} memories from {result.get('message_count', 0)} messages")

        # Cleanup
        client.send(f"destroy_session:{session_id}")
        log_pass(f"Destroyed session")

    return results

def main():
    parser = argparse.ArgumentParser(description='Test jcode memory system')
    parser.add_argument('--fresh', action='store_true', help='Start fresh server for testing')
    parser.add_argument('--provider', choices=['claude', 'openai'], help='Test specific provider only')
    parser.add_argument('--socket', help='Custom debug socket path')
    args = parser.parse_args()

    providers = [args.provider] if args.provider else ['claude', 'openai']

    proc = None
    if args.fresh:
        log_section("Starting fresh test server...")
        test_socket = '/tmp/jcode-memory-test.sock'
        debug_socket = test_socket.replace('.sock', '-debug.sock')

        for s in [test_socket, debug_socket]:
            if os.path.exists(s):
                os.remove(s)

        env = os.environ.copy()
        env['JCODE_DEBUG_CONTROL'] = '1'
        env['JCODE_SOCKET'] = test_socket

        proc = subprocess.Popen(
            ['jcode', 'serve'],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
        )

        for i in range(100):
            if os.path.exists(debug_socket):
                break
            time.sleep(0.1)
        else:
            proc.terminate()
            log_fail("Server failed to start")
            sys.exit(1)

        log_pass(f"Server started (PID {proc.pid})")
        socket_path = debug_socket
    else:
        socket_path = args.socket or '/run/user/1000/jcode-debug.sock'
        if not os.path.exists(socket_path):
            log_fail(f"Debug socket not found: {socket_path}")
            log("Use --fresh to start a test server, or ensure jcode is running")
            sys.exit(1)

    client = DebugSocketClient(socket_path)

    try:
        client.connect()
        results = run_tests(client, providers)

        log_section("TEST RESULTS")
        total = results["passed"] + results["failed"]
        log(f"Passed: {results['passed']}/{total}", GREEN if results["failed"] == 0 else YELLOW)
        if results["failed"] > 0:
            log(f"Failed: {results['failed']}/{total}", RED)

        client.close()

    finally:
        if proc:
            proc.terminate()
            proc.wait()
            log("Server stopped")

    sys.exit(0 if results["failed"] == 0 else 1)

if __name__ == '__main__':
    main()
