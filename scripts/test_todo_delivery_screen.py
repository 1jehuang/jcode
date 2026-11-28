"""Exercise todo tool -> persisted goals -> daemon snapshot -> pinned terminal colors.

Run with `uv run --with pyte python scripts/test_todo_delivery_screen.py BINARY`.
Uses an isolated socket, home, and terminal. No model request is made.
"""

import json
import os
import re
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import repro_slash_flicker as term


def request(path, command, session=None):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.connect(str(path))
        packet = {
            "type": "debug_command",
            "id": 1,
            "command": command,
            "session_id": session,
        }
        conn.sendall((json.dumps(packet) + "\n").encode())
        result = term._recv(conn, 30)
        if result.get("type") == "error":
            raise RuntimeError(result)
        return result.get("output", "")


def main():
    binary = str(Path(sys.argv[1]).resolve())
    with tempfile.TemporaryDirectory(
        prefix="jd-color-", dir=os.getenv("JCODE_SCRATCH_DIR")
    ) as root:
        home, run, tmp = [Path(root) / name for name in ("home", "run", "tmp")]
        for d in (home, run, tmp):
            d.mkdir(mode=0o700)
        (home / "config.toml").write_text("[display]\npin_todos = true\n")
        env = dict(
            os.environ,
            JCODE_HOME=str(home),
            TMPDIR=str(tmp),
            JCODE_TEST_SESSION="1",
            JCODE_RUNTIME_DIR=str(run),
            JCODE_SOCKET=str(run / "jcode.sock"),
            JCODE_DEBUG_CONTROL="1",
            JCODE_NO_TELEMETRY="1",
            COLORTERM="truecolor",
            JCODE_TEMP_SERVER="1",
            JCODE_SERVER_OWNER_PID=str(os.getpid()),
            JCODE_THEME="dark",
        )
        debug = run / "jcode-debug.sock"
        cmd, resp = run / "client_cmd", run / "client_resp"
        with (run / "log").open("wb") as log:
            server = subprocess.Popen(
                [
                    binary,
                    "serve",
                    "--socket",
                    env["JCODE_SOCKET"],
                    "--debug-socket",
                    "--no-update",
                    "--no-selfdev",
                ],
                env=env,
                stdout=log,
                stderr=log,
                start_new_session=True,
            )
            client = None
            try:
                term.wait_for_socket(debug)
                sid = request(debug, "create_session:" + str(Path.cwd())).strip()
                if sid.startswith("{"):
                    sid = json.loads(sid)["session_id"]
                sid = sid.split()[-1]
                goal = {
                    "group": "outcome",
                    "closed_feedback_loop": "strong",
                    "feedback_loop": "Inspect the terminal cell foreground",
                    "feedback_loop_relevance": "acceptance_aligned",
                    "feedback_loop_coverage": "main_paths",
                    "feedback_loop_traceability": "complete",
                    "verification_state": "checked_end_to_end",
                }
                for i, (state, color) in enumerate(
                    [
                        ("undelivered", "e16969"),
                        ("partially_delivered", "e1b450"),
                        ("delivered", "69cda5"),
                    ]
                ):
                    goal["delivery_state"] = state
                    args = {"intent": "Assess actual outcome", "goals": [goal]}
                    if not i:
                        args["phases"] = [
                            {
                                "phase": 1,
                                "lanes": [
                                    {
                                        "lane": "outcome",
                                        "items": [
                                            {
                                                "name": "Check result",
                                                "status": "completed",
                                                "confidence": "verified",
                                                "priority": "high",
                                            }
                                        ],
                                    }
                                ],
                            }
                        ]
                    result = request(debug, "tool:todo " + json.dumps(args), sid)
                    if state not in result:
                        raise RuntimeError(result)
                    if client is None:
                        client = term.launch(binary, env, sid, cmd, resp)
                        assert term.settle(cmd, resp), "client did not initialize"
                    label = re.compile(
                        r"delivery\s+(" + state.replace("_", "[_ ]") + r")\b",
                        re.IGNORECASE,
                    )
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        with client.lock:
                            rows = list(client.screen.display)
                            match = next(
                                (
                                    (y, m.start(1))
                                    for y, row in enumerate(rows)
                                    if (m := label.search(row))
                                ),
                                None,
                            )
                            if match:
                                assert not any("checked_end_to_end" in row for row in rows)
                                actual = client.screen.buffer[match[0]][match[1]].fg
                                assert actual == color, (state, actual, color)
                                print(f"{state}: #{actual}", flush=True)
                                break
                        if any("esc to skip onboarding" in row.lower() for row in rows):
                            client.send(b"\x1b")
                        time.sleep(0.1)
                    else:
                        raise AssertionError(
                            (state, [row.strip() for row in rows if row.strip()])
                        )
            finally:
                if client:
                    client.shutdown()
                if server.poll() is None:
                    os.killpg(server.pid, signal.SIGTERM)
                    try:
                        server.wait(5)
                    except subprocess.TimeoutExpired:
                        os.killpg(server.pid, signal.SIGKILL)
                        server.wait()


if __name__ == "__main__":
    main()
