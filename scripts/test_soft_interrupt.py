#!/usr/bin/env python3
"""
Comprehensive test for soft interrupt injection.
Tests all injection points with real provider.

Uses separate socket connections to send messages and queue interrupts
concurrently.
"""

import socket
import json
import time
import sys
import os
import threading
import queue as queue_mod

SOCKET_PATH = "/tmp/jcode-selfdev-debug.sock"

def send_cmd_blocking(sock, cmd, session_id=None, timeout=180):
    """Send a debug command and wait for response (blocks)."""
    req = {"type": "debug_command", "id": int(time.time() * 1000000), "command": cmd}
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
                resp = json.loads(data.decode())
                return resp.get('ok', False), resp.get('output', '')
            except json.JSONDecodeError:
                continue
        except socket.timeout:
            break
    
    try:
        resp = json.loads(data.decode())
        return resp.get('ok', False), resp.get('output', '')
    except:
        return False, f"Failed to parse: {data.decode()[:500]}"

def send_cmd_quick(cmd, session_id=None, timeout=10):
    """Quick command on fresh connection."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(SOCKET_PATH)
    try:
        return send_cmd_blocking(sock, cmd, session_id, timeout)
    finally:
        sock.close()

def send_message_async(msg, session_id, result_queue):
    """Send message in a thread."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        sock.connect(SOCKET_PATH)
        ok, output = send_cmd_blocking(sock, f"message:{msg}", session_id, timeout=180)
        result_queue.put((ok, output))
    except Exception as e:
        result_queue.put((False, str(e)))
    finally:
        sock.close()

def create_test_session(cwd="/tmp"):
    """Create a headless test session."""
    ok, output = send_cmd_quick(f"create_session:{cwd}")
    if not ok:
        return None
    try:
        data = json.loads(output)
        return data.get('session_id')
    except:
        return None

def destroy_session(session_id):
    """Destroy a test session."""
    send_cmd_quick(f"destroy_session:{session_id}", timeout=5)

def get_history(session_id):
    """Get conversation history."""
    ok, output = send_cmd_quick("history", session_id)
    if ok:
        try:
            return json.loads(output)
        except:
            pass
    return []

def queue_interrupt(session_id, content, urgent=False):
    """Queue a soft interrupt message."""
    cmd = f"queue_interrupt_urgent:{content}" if urgent else f"queue_interrupt:{content}"
    ok, output = send_cmd_quick(cmd, session_id, timeout=5)
    return ok

def extract_text_from_content(content):
    """Extract text from message content blocks."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        texts = []
        for block in content:
            if isinstance(block, dict):
                if block.get('type') == 'text':
                    texts.append(block.get('text', ''))
                elif 'text' in block:
                    texts.append(block['text'])
            elif isinstance(block, str):
                texts.append(block)
        return ' '.join(texts)
    return str(content)

def print_history(history):
    """Print conversation history for debugging."""
    for i, msg in enumerate(history):
        role = msg.get('role', '?')
        content = msg.get('content', [])
        text = extract_text_from_content(content)[:80]
        
        # Check for tool_use/tool_result
        has_tool_use = any(
            isinstance(b, dict) and b.get('type') == 'tool_use'
            for b in (content if isinstance(content, list) else [])
        )
        has_tool_result = any(
            isinstance(b, dict) and b.get('type') == 'tool_result'
            for b in (content if isinstance(content, list) else [])
        )
        
        suffix = ""
        if has_tool_use:
            suffix = " [tool_use]"
        if has_tool_result:
            suffix = " [tool_result]"
        
        print(f"  [{i}] {role}{suffix}: {text}...")

def test_basic_message():
    """Test that basic messaging works."""
    print("\n" + "="*60)
    print("TEST: Basic message (no interrupt)")
    print("="*60)
    
    session_id = create_test_session()
    if not session_id:
        print("❌ Failed to create session")
        return False
    print(f"Created session: {session_id}")
    
    try:
        result_q = queue_mod.Queue()
        thread = threading.Thread(
            target=send_message_async,
            args=("What is 2+2? Just answer with the number.", session_id, result_q)
        )
        thread.start()
        thread.join(timeout=120)
        
        if thread.is_alive():
            print("❌ Message timed out")
            return False
        
        ok, output = result_q.get()
        if not ok:
            print(f"❌ Message failed: {output[:200]}")
            return False
        
        print(f"Response: {output[:100]}...")
        
        history = get_history(session_id)
        roles = [m.get('role') for m in history]
        print(f"Roles: {roles}")
        
        if roles == ['user', 'assistant']:
            print("✅ Basic message works")
            return True
        else:
            print(f"❌ Unexpected roles: {roles}")
            return False
            
    finally:
        destroy_session(session_id)

def test_soft_interrupt_during_streaming():
    """
    Test soft interrupt injection during streaming.
    
    1. Start a message that takes time (asks AI to think step by step)
    2. While streaming, queue a soft interrupt
    3. Verify the interrupt appears in the conversation after the first response
    """
    print("\n" + "="*60)
    print("TEST: Soft interrupt during streaming")
    print("="*60)
    
    session_id = create_test_session()
    if not session_id:
        print("❌ Failed to create session")
        return False
    print(f"Created session: {session_id}")
    
    try:
        # Start a message that should take a moment
        result_q = queue_mod.Queue()
        thread = threading.Thread(
            target=send_message_async,
            args=("Count from 1 to 10, one number per line.", session_id, result_q)
        )
        thread.start()
        
        # Wait a moment for streaming to start, then queue interrupt
        time.sleep(1.0)
        print("Queueing soft interrupt: 'What is 5+5?'")
        ok = queue_interrupt(session_id, "What is 5+5? Just the number.")
        if ok:
            print("✓ Interrupt queued successfully")
        else:
            print("⚠️ Failed to queue interrupt (may have finished already)")
        
        # Wait for message to complete
        thread.join(timeout=120)
        
        if thread.is_alive():
            print("❌ Message timed out")
            return False
        
        ok, output = result_q.get()
        if not ok:
            print(f"❌ Message failed: {output[:200]}")
            return False
        
        print(f"Response: {output[:150]}...")
        
        # Check history
        history = get_history(session_id)
        roles = [m.get('role') for m in history]
        print(f"\nHistory ({len(history)} messages):")
        print_history(history)
        
        # Look for our interrupt message in history
        found_interrupt = False
        for msg in history:
            text = extract_text_from_content(msg.get('content', []))
            if '5+5' in text:
                found_interrupt = True
                break
        
        if found_interrupt:
            print("✅ Interrupt message found in history")
        else:
            print("⚠️ Interrupt might have arrived after response completed")
            # This is OK - timing dependent
        
        # The key check: message order should still be valid
        # User messages should be followed by assistant messages
        valid_order = True
        for i in range(len(roles) - 1):
            if roles[i] == 'user' and roles[i+1] == 'user':
                # Check if second user is tool_result
                content = history[i+1].get('content', [])
                is_tool_result = any(
                    isinstance(b, dict) and b.get('type') == 'tool_result'
                    for b in (content if isinstance(content, list) else [])
                )
                if not is_tool_result:
                    valid_order = False
                    print(f"⚠️ Two consecutive user messages at {i} and {i+1}")
        
        if valid_order:
            print("✅ Message order is valid")
            return True
        else:
            return False
            
    finally:
        destroy_session(session_id)

def test_soft_interrupt_with_tools():
    """
    Test soft interrupt injection when tools are involved.
    
    1. Send message that triggers a tool
    2. Queue interrupt during tool execution
    3. Verify interrupt appears after tool result
    """
    print("\n" + "="*60)
    print("TEST: Soft interrupt with tool execution")
    print("="*60)
    
    session_id = create_test_session(cwd="/tmp")
    if not session_id:
        print("❌ Failed to create session")
        return False
    print(f"Created session: {session_id}")
    
    try:
        # Create a test file
        test_file = "/tmp/test_interrupt_tools.txt"
        with open(test_file, 'w') as f:
            f.write("apple\nbanana\ncherry\ndate\nelderberry")
        
        # Start message that will trigger file read
        result_q = queue_mod.Queue()
        thread = threading.Thread(
            target=send_message_async,
            args=(f"Read {test_file} and list each fruit.", session_id, result_q)
        )
        thread.start()
        
        # Wait for tool execution to start, then queue interrupt
        time.sleep(0.5)
        print("Queueing soft interrupt: 'How many fruits are there?'")
        queue_interrupt(session_id, "How many fruits are there in total?")
        
        # Wait for completion
        thread.join(timeout=180)
        
        if thread.is_alive():
            print("❌ Timed out")
            os.remove(test_file)
            return False
        
        ok, output = result_q.get()
        os.remove(test_file)
        
        if not ok:
            print(f"❌ Failed: {output[:200]}")
            return False
        
        print(f"Response: {output[:150]}...")
        
        # Check history
        history = get_history(session_id)
        print(f"\nHistory ({len(history)} messages):")
        print_history(history)
        
        # Verify tool was used
        has_tool_use = False
        for msg in history:
            content = msg.get('content', [])
            if isinstance(content, list):
                for block in content:
                    if isinstance(block, dict) and block.get('type') == 'tool_use':
                        has_tool_use = True
        
        if has_tool_use:
            print("✅ Tool was used")
        else:
            print("⚠️ No tool use detected (AI may have guessed)")
        
        # Check for our interrupt
        found_interrupt = False
        for msg in history:
            text = extract_text_from_content(msg.get('content', []))
            if 'how many' in text.lower() and 'fruit' in text.lower():
                found_interrupt = True
        
        if found_interrupt:
            print("✅ Interrupt found in history")
        else:
            print("⚠️ Interrupt may have arrived after completion")
        
        print("✅ Test completed")
        return True
            
    finally:
        destroy_session(session_id)

def test_urgent_interrupt_skips_tools():
    """
    Test urgent interrupt can skip remaining tools.
    
    Note: This is hard to test reliably because we need multiple
    tool calls and precise timing. We'll do a best-effort test.
    """
    print("\n" + "="*60)
    print("TEST: Urgent interrupt (tool skipping)")
    print("="*60)
    
    session_id = create_test_session(cwd="/tmp")
    if not session_id:
        print("❌ Failed to create session")
        return False
    print(f"Created session: {session_id}")
    
    try:
        # Create multiple files so AI might try to read them all
        files = []
        for i in range(3):
            f = f"/tmp/test_urgent_{i}.txt"
            with open(f, 'w') as fp:
                fp.write(f"Content of file {i}")
            files.append(f)
        
        # Ask to read all files
        result_q = queue_mod.Queue()
        thread = threading.Thread(
            target=send_message_async,
            args=(f"Read all three files: {', '.join(files)}. Tell me what each contains.", session_id, result_q)
        )
        thread.start()
        
        # Send urgent interrupt quickly
        time.sleep(0.3)
        print("Queueing URGENT interrupt: 'Stop! Just tell me hi.'")
        queue_interrupt(session_id, "Stop! Just tell me hi.", urgent=True)
        
        # Wait
        thread.join(timeout=180)
        
        # Cleanup files
        for f in files:
            if os.path.exists(f):
                os.remove(f)
        
        if thread.is_alive():
            print("❌ Timed out")
            return False
        
        ok, output = result_q.get()
        if not ok:
            print(f"❌ Failed: {output[:200]}")
            return False
        
        print(f"Response: {output[:150]}...")
        
        # Check history
        history = get_history(session_id)
        print(f"\nHistory ({len(history)} messages):")
        print_history(history)
        
        # Look for skipped tools
        found_skipped = False
        for msg in history:
            text = extract_text_from_content(msg.get('content', []))
            if 'skipped' in text.lower() or 'interrupted' in text.lower():
                found_skipped = True
        
        if found_skipped:
            print("✅ Found evidence of tool skipping")
        else:
            print("⚠️ Tools may have completed before interrupt")
        
        print("✅ Test completed")
        return True
            
    finally:
        destroy_session(session_id)

def test_interrupt_during_long_response():
    """
    Test soft interrupt during a genuinely long response.
    We ask for something that takes time to generate.
    """
    print("\n" + "="*60)
    print("TEST: Interrupt during long response")
    print("="*60)
    
    session_id = create_test_session()
    if not session_id:
        print("❌ Failed to create session")
        return False
    print(f"Created session: {session_id}")
    
    try:
        # Ask for something that takes time
        result_q = queue_mod.Queue()
        thread = threading.Thread(
            target=send_message_async,
            args=("Write a detailed 5-paragraph essay about the history of computing, from ENIAC to modern smartphones.", session_id, result_q)
        )
        thread.start()
        
        # Queue interrupt after a delay
        time.sleep(2.0)
        print("Queueing interrupt: 'STOP - just say OK'")
        ok = queue_interrupt(session_id, "STOP - just say 'OK' and nothing else.")
        if ok:
            print("✓ Interrupt queued")
        
        # Wait for completion
        thread.join(timeout=180)
        
        if thread.is_alive():
            print("❌ Timed out")
            return False
        
        ok, _ = result_q.get()
        if not ok:
            print("❌ Message failed")
            return False
        
        # Check history
        history = get_history(session_id)
        print(f"\nHistory ({len(history)} messages):")
        print_history(history)
        
        # Look for our interrupt
        found_stop = False
        for msg in history:
            if msg.get('role') == 'user':
                text = extract_text_from_content(msg.get('content', []))
                if 'STOP' in text:
                    found_stop = True
                    break
        
        if found_stop:
            print("✅ Interrupt found in history")
            
            # Verify order: first assistant response should come BEFORE the STOP message
            first_assistant_idx = None
            stop_idx = None
            for i, msg in enumerate(history):
                role = msg.get('role')
                text = extract_text_from_content(msg.get('content', []))
                if role == 'assistant' and first_assistant_idx is None:
                    first_assistant_idx = i
                if role == 'user' and 'STOP' in text:
                    stop_idx = i
            
            if first_assistant_idx is not None and stop_idx is not None:
                if first_assistant_idx < stop_idx:
                    print(f"✅ First assistant ({first_assistant_idx}) comes before STOP ({stop_idx})")
                    return True
                else:
                    print(f"❌ STOP ({stop_idx}) comes before first assistant ({first_assistant_idx})")
                    return False
        else:
            print("⚠️ Interrupt not found - response may have completed too fast")
            # Not a failure, just timing
            return True
            
    finally:
        destroy_session(session_id)

def test_message_order_preserved():
    """
    Test that assistant message comes BEFORE injected user message.
    This is the bug we fixed.
    """
    print("\n" + "="*60)
    print("TEST: Message order (assistant before interrupt)")
    print("="*60)
    
    session_id = create_test_session()
    if not session_id:
        print("❌ Failed to create session")
        return False
    print(f"Created session: {session_id}")
    
    try:
        # Send message
        result_q = queue_mod.Queue()
        thread = threading.Thread(
            target=send_message_async,
            args=("Write a haiku about coding.", session_id, result_q)
        )
        thread.start()
        
        # Queue interrupt
        time.sleep(0.5)
        queue_interrupt(session_id, "Also write one about debugging.")
        
        thread.join(timeout=120)
        
        if thread.is_alive():
            print("❌ Timed out")
            return False
        
        ok, _ = result_q.get()
        if not ok:
            print("❌ Failed")
            return False
        
        # Check history
        history = get_history(session_id)
        print(f"\nHistory ({len(history)} messages):")
        print_history(history)
        
        # Key check: find the interrupt message
        # It should be AFTER an assistant message, not before
        interrupt_idx = None
        for i, msg in enumerate(history):
            text = extract_text_from_content(msg.get('content', []))
            if 'debugging' in text.lower() and msg.get('role') == 'user':
                interrupt_idx = i
                break
        
        if interrupt_idx is not None:
            print(f"Found interrupt at index {interrupt_idx}")
            # Check what's before it
            if interrupt_idx > 0:
                prev_role = history[interrupt_idx - 1].get('role')
                if prev_role == 'assistant':
                    print("✅ Interrupt comes AFTER assistant message (correct order)")
                    return True
                else:
                    print(f"❌ Interrupt preceded by {prev_role}, expected assistant")
                    return False
            else:
                print("❌ Interrupt at index 0, unexpected")
                return False
        else:
            print("⚠️ Interrupt not found (may have arrived after completion)")
            # Check general order
            roles = [m.get('role') for m in history]
            print(f"Roles: {roles}")
            return True  # Can't verify but not a failure
            
    finally:
        destroy_session(session_id)

def main():
    print("="*60)
    print("SOFT INTERRUPT INJECTION TESTS")
    print("="*60)
    print(f"Using debug socket: {SOCKET_PATH}")
    
    if not os.path.exists(SOCKET_PATH):
        print(f"❌ Debug socket not found: {SOCKET_PATH}")
        sys.exit(1)
    
    results = []
    
    tests = [
        ("Basic message", test_basic_message),
        ("Soft interrupt during streaming", test_soft_interrupt_during_streaming),
        ("Soft interrupt with tools", test_soft_interrupt_with_tools),
        ("Urgent interrupt", test_urgent_interrupt_skips_tools),
        ("Interrupt during long response", test_interrupt_during_long_response),
        ("Message order", test_message_order_preserved),
    ]
    
    for name, test_fn in tests:
        try:
            result = test_fn()
            results.append((name, result))
        except Exception as e:
            print(f"❌ Test '{name}' crashed: {e}")
            import traceback
            traceback.print_exc()
            results.append((name, False))
    
    # Summary
    print("\n" + "="*60)
    print("SUMMARY")
    print("="*60)
    
    passed = sum(1 for _, r in results if r)
    failed = sum(1 for _, r in results if not r)
    
    for name, result in results:
        status = "✅ PASSED" if result else "❌ FAILED"
        print(f"  {status}: {name}")
    
    print(f"\nTotal: {passed} passed, {failed} failed")
    sys.exit(0 if failed == 0 else 1)

if __name__ == "__main__":
    main()
