#!/usr/bin/env python3
"""Opt-in live Jev/browser acceptance on an existing disposable loopback tab.

Owns the HTTP fixture for the entire test run. Never opens a window, changes a
user tab, or prints credentials. Requires a ready browser bridge, an existing
BROWSER_SESSION, and Jcode subscription or Jev BYOK credentials. Calls incur
small inference costs.
"""
import argparse
import fcntl
from http.server import ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
from urllib.parse import urlsplit

from browser_handoff_fixture import FixtureHandler, PAGES


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tab-id", type=int, required=True,
                        help="Existing disposable tab showing a loopback Jcode fixture or about:blank")
    args = parser.parse_args()
    if args.tab_id <= 0 or not os.environ.get("BROWSER_SESSION", "").strip():
        parser.error("A positive --tab-id and an existing BROWSER_SESSION are required")
    repo = Path(__file__).resolve().parents[1]
    env = dict(os.environ, JCODE_BROWSER_HANDOFF_TEST_TAB_ID=str(args.tab_id),
               JCODE_BROWSER_HANDOFF_TEST_BLOCKED_TAB_ID=str(args.tab_id),
               JCODE_BROWSER_HANDOFF_TEST_TRACE="1")
    # Finish compilation before acquiring the tab or starting its fixture. Cargo
    # may wait for another build's lock, which is not a live-test timeout.
    print('JCODE_CHECKPOINT {"message":"Compiling live browser test harness once"}', flush=True)
    build = subprocess.run(
        [str(repo / "scripts/dev_cargo.sh"), "test", "-p", "jcode-app-core", "--lib", "--no-run", "--message-format=json"],
        cwd=repo, env=env, stdout=subprocess.PIPE, text=True, timeout=1800, check=False)
    executables = set()
    for line in build.stdout.splitlines():
        if not line.strip():
            continue
        artifact = json.loads(line)
        if artifact.get("reason") == "compiler-message":
            rendered = artifact.get("message", {}).get("rendered")
            if rendered:
                print(rendered, file=sys.stderr, end="", flush=True)
        target = artifact.get("target", {})
        if (artifact.get("reason") == "compiler-artifact"
                and target.get("name") == "jcode_app_core"
                and "lib" in target.get("kind", [])
                and artifact.get("profile", {}).get("test") is True
                and artifact.get("executable")):
            executables.add(artifact["executable"])
    build.check_returncode()
    if len(executables) != 1:
        parser.error("Expected exactly one jcode-app-core library test executable from Cargo")
    test_executable = executables.pop()
    if not Path(test_executable).is_file():
        parser.error("Cargo's library test executable is missing")
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    lock_fd = os.open(runtime / f"jcode-browser-acceptance-tab-{args.tab_id}.lock",
                      os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        os.close(lock_fd)
        parser.error("Another acceptance runner owns this disposable tab")
    bridge = Path(os.environ.get("JCODE_HOME", str(Path.home() / ".jcode"))) / "browser/browser"

    def call(action, **params):
        result = subprocess.run([str(bridge), action, json.dumps(dict(params, tabId=args.tab_id))],
                                env=env, capture_output=True, text=True, timeout=20, check=True)
        return json.loads(result.stdout)

    def probe():
        return call("evaluate", script="return {url:location.href,title:document.title,ready:document.readyState};", frameId=0)["result"]

    listing = call("listTabs")
    original = next((tab for window in listing.get("windows", [])
                     for tab in window.get("tabs", []) if tab.get("tabId") == args.tab_id), None)
    if original is None:
        parser.error("The designated disposable tab is no longer available")
    url = urlsplit(original["url"])
    if original["url"] != "about:blank" and not (
            url.scheme in ("http", "https") and url.hostname in ("127.0.0.1", "localhost", "::1")
            and original["title"] in ("Jcode isolated browser fixture", "Jev hybrid verified")):
        parser.error("Refusing to replace a non-fixture tab. Prepare a disposable about:blank tab first.")

    server = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    print(f"Owned browser fixture: {origin}, disposable tab {args.tab_id}", flush=True)
    try:
        tests = [
            ("live_jev_decision_smoke", "/"),
            ("live_browser_handoff_completes_local_navigation", "/"),
            ("live_browser_handoff_completes_search_and_nested_navigation", "/task"),
            ("live_browser_handoff_requests_script_and_resumes", "/"),
            ("live_browser_handoff_sensitive_fixture_hands_back_without_actions", "/blocked"),
        ]
        for name, path in tests:
            target = origin + path
            call("navigate", url=target, wait=True)
            # The bridge can acknowledge a click/navigation before a new document
            # commits. Do not let test startup measure a previous fixture page.
            deadline = time.monotonic() + 10
            while True:
                state = probe()
                if state["url"] == target and state["ready"] == "complete":
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError("Local fixture did not finish navigation")
                time.sleep(0.05)
            print(f"JCODE_CHECKPOINT {json.dumps({'message': 'Running ' + name})}", flush=True)
            subprocess.run([test_executable, name, "--ignored", "--nocapture"], cwd=repo, env=env,
                           timeout=300, check=True)
        print('JCODE_CHECKPOINT {"message":"All live browser handoff acceptance cases passed"}', flush=True)
    finally:
        # Leave no dead localhost fixture behind. This is only the explicitly
        # designated disposable test tab, never an arbitrary user browsing tab.
        try:
            if probe()["url"].startswith(origin + "/"):
                call("navigate", url="about:blank", wait=True)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
            os.close(lock_fd)


if __name__ == "__main__":
    main()
