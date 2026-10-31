#!/usr/bin/env python3
"""Fast contracts for the acceptance harness itself, not substitutes for E2E."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
import urllib.request
import xml.etree.ElementTree as ET

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
import test_cloud_acceptance as acceptance
import fake_openai_server as provider


class Reports(unittest.TestCase):
    def test_workflow_installs_nondefault_acceptance_prerequisites(self):
        workflow = (SCRIPTS.parent / ".github/workflows/cloud-acceptance.yml").read_text()
        install_lines = [line for line in workflow.splitlines()
                         if "apt-get install" in line]
        packages = set(" ".join(install_lines).split())
        for package in ("ripgrep", "tmux", "openssh-server", "bubblewrap",
                        "util-linux", "python3"):
            self.assertIn(package, packages, f"runner must install {package}")

    def test_failure_is_present_in_json_and_junit(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            acceptance.write_report(root, [
                {"name": "pass", "seconds": 1.0, "status": "passed", "artifacts": "one"},
                {"name": "fail", "seconds": 2.0, "status": "failed", "artifacts": "two",
                 "error": "outage", "traceback": "details"},
            ], "/binary", "version")
            self.assertEqual(json.loads((root / "report.json").read_text())["results"][1]["status"], "failed")
            xml = ET.parse(root / "junit.xml").getroot()
            self.assertEqual(xml.attrib["tests"], "2")
            self.assertEqual(xml.attrib["failures"], "1")
            self.assertEqual(xml.find("testcase/failure").attrib["message"], "outage")

    def test_missing_prerequisites_fail_instead_of_skip(self):
        with patch.object(acceptance.shutil, "which", return_value=None):
            with self.assertRaisesRegex(RuntimeError, "Missing test prerequisites"):
                acceptance.preflight(["tui"])

    def test_child_failure_propagates_and_preserves_output(self):
        with tempfile.TemporaryDirectory() as temp:
            log = Path(temp) / "child.log"
            with self.assertRaisesRegex(AssertionError, "exited 7"):
                acceptance.child([sys.executable, "-c", "print('evidence'); raise SystemExit(7)"],
                                 os.environ.copy(), log, 5)
            self.assertIn("evidence", log.read_text())

    def test_child_timeout_is_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaisesRegex(AssertionError, "timed out"):
                acceptance.child([sys.executable, "-c", "import time; time.sleep(60)"],
                                 os.environ.copy(), Path(temp) / "child.log", 0.1)


class Provider(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        provider.LOG = str(self.root / "requests.jsonl")
        provider.CONTROL = self.root
        self.server = provider.ThreadingHTTPServer(("127.0.0.1", 0), provider.H)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop)

    def stop(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)

    def request(self, messages, stream=False):
        request = urllib.request.Request(f"http://127.0.0.1:{self.server.server_port}/v1/chat/completions",
                                         json.dumps({"messages": messages, "stream": stream}).encode(),
                                         {"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=5) as response:
            return response.read().decode()

    def test_todo_uses_real_tool_call_wire_shape(self):
        messages = [{"role": "user", "content": "[e2e:todo] [e2e:hold:abc] work"}]
        reply = json.loads(self.request(messages))
        choice = reply["choices"][0]
        self.assertEqual(choice["finish_reason"], "tool_calls")
        call = choice["message"]["tool_calls"][0]
        self.assertEqual(call["function"]["name"], "todo")
        self.assertEqual(json.loads(call["function"]["arguments"])["todos"][0]["status"], "in_progress")
        self.assertFalse((self.root / "abc.held").exists())

    def test_gate_waits_for_release_and_does_not_repeat_tool(self):
        messages = [{"role": "user", "content": "[e2e:todo] [e2e:hold:abc] work"},
                    {"role": "assistant", "content": None, "tool_calls": []},
                    {"role": "tool", "tool_call_id": "call_acceptance_todo", "content": "saved"}]
        replies = []
        thread = threading.Thread(target=lambda: replies.append(self.request(messages, stream=True)))
        thread.start()
        try:
            deadline = time.monotonic() + 5
            while not (self.root / "abc.held").exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue((self.root / "abc.held").exists())
            self.assertTrue(thread.is_alive(), "provider must remain blocked before release")
        finally:
            (self.root / "abc.release").touch()
            thread.join(timeout=5)
        self.assertFalse(thread.is_alive())
        self.assertIn("[DONE]", replies[0])
        self.assertNotIn('"tool_calls"', replies[0])
        self.assertEqual(len(Path(provider.LOG).read_text().splitlines()), 1)

    def test_later_user_prompt_does_not_retrigger_historical_gate(self):
        messages = [{"role": "user", "content": "[e2e:hold:old]"},
                    {"role": "assistant", "content": "done"},
                    {"role": "user", "content": "continue on cloud"}]
        reply = json.loads(self.request(messages))
        self.assertEqual(reply["choices"][0]["finish_reason"], "stop")
        self.assertFalse((self.root / "old.held").exists())


class Isolation(unittest.TestCase):
    def test_cloud_selection_preserves_group_default_and_explicit_round_trip(self):
        from unittest.mock import Mock

        fixture = Mock()
        fixture.records.return_value = ["existing request"]
        acceptance.CloudTUI.go_cloud(fixture)
        fixture.send.assert_called_once_with("/cloud")
        fixture.await_cloud.assert_called_once_with(1)
        fixture.reset_mock()
        acceptance.CloudTUI.go_cloud(fixture, session_only=True)
        fixture.send.assert_called_once_with("/cloud --session-only")
        fixture.await_cloud.assert_called_once_with(1)

    def test_long_artifact_paths_do_not_lengthen_runtime_sockets(self):
        with tempfile.TemporaryDirectory() as temp:
            fixture = acceptance.CloudTUI("/bin/true", Path(temp) / ("long-artifacts-" * 12))
            self.addCleanup(fixture.close)
            self.assertLess(len(os.fsencode(fixture.sock)), 100)
            with socket.socket(socket.AF_UNIX) as sock:
                sock.bind(fixture.sock)

    def test_cleanup_reaps_only_exact_workspace_marker(self):
        with tempfile.TemporaryDirectory() as temp:
            fixture = acceptance.CloudTUI("/bin/true", Path(temp) / "fixture")
            owned = subprocess.Popen(["sleep", "60"], env=dict(os.environ, JCODE_TEST_WORKSPACE=str(fixture.root)))
            other = subprocess.Popen(["sleep", "60"], env=dict(os.environ, JCODE_TEST_WORKSPACE=str(fixture.root) + "-other"))
            try:
                fixture.close()
                owned.wait(timeout=5)
                self.assertIsNone(other.poll(), "cleanup must not kill a neighboring workspace")
            finally:
                for process in (owned, other):
                    if process.poll() is None:
                        process.terminate()
                    process.wait(timeout=5)

    def test_historical_prompt_does_not_count_as_fresh_turn(self):
        with tempfile.TemporaryDirectory() as temp:
            fixture = acceptance.CloudTUI("/bin/true", Path(temp) / "fixture")
            self.addCleanup(fixture.close)
            historical = {"agent": True, "messages": [
                {"role": "user", "content": "acceptance back on laptop"},
                {"role": "user", "content": "new cloud continuation"},
            ]}
            log = fixture.root / "model.jsonl"
            log.write_text(json.dumps(historical) + "\n")
            with patch("cloud_acceptance_tui.wait_for", side_effect=lambda predicate, _: predicate()):
                self.assertIsNone(fixture.wait_request("acceptance back on laptop"))
                with log.open("a") as output:
                    output.write(json.dumps({"agent": True, "messages": [
                        {"role": "user", "content": "acceptance back on laptop"}
                    ]}) + "\n")
                self.assertIsNotNone(fixture.wait_request("acceptance back on laptop"))

    def test_only_agent_requests_count_as_continuation_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            fixture = acceptance.CloudTUI("/bin/true", Path(temp) / "fixture")
            self.addCleanup(fixture.close)
            (fixture.root / "model.jsonl").write_text(
                json.dumps({"agent": False, "messages": [{"content": "old cloud notice"}]}) + "\n" +
                json.dumps({"agent": True, "messages": [{"content": "fresh agent turn"}]}) + "\n" +
                '{"partial":'
            )
            self.assertEqual(fixture.records(), [{"agent": True, "messages": [{"content": "fresh agent turn"}]}])


if __name__ == "__main__":
    unittest.main()
