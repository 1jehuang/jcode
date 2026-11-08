#!/usr/bin/env python3
"""Exercise real cloud CLI parsing against a hermetic, nonexecuting transport.

Usage: python3 scripts/test_cloud_session_cli.py --binary /absolute/path/to/jcode
No cloud account, model call, remote shell, daemon, or detached child is needed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap

SESSION = "session_fixture"

TRANSPORT = r'''
import json
import os
from pathlib import Path
import shlex
import sys

root = Path(os.environ["CLOUD_CLI_FIXTURE"])
if len(sys.argv) != 2:
    raise SystemExit("transport expects exactly one quoted remote script")
# Parse, never execute, the shell source produced by Target.remote_command.
words = shlex.split(sys.argv[1])
if "exec" not in words:
    raise SystemExit("remote script has no exec boundary")
argv = words[words.index("exec") + 1:]
with (root / "transport.jsonl").open("a") as output:
    output.write(json.dumps(argv) + "\n")
if argv[:4] != ["jcode", "--no-update", "--no-selfdev", "cloud"]:
    raise SystemExit("unexpected remote command prefix")
command = argv[4]
if "--session" not in argv or argv[argv.index("--session") + 1] != "session_fixture":
    raise SystemExit("unexpected session")
state = json.loads((root / "state.json").read_text())
if command == "run-detached":
    if argv[7] != "--" or len(argv) != 9:
        raise SystemExit("prompt escaped its single argument")
    (root / "received_prompt.json").write_text(json.dumps(argv[8]))
    print(json.dumps({"session_id": "session_fixture", "state": "running", "pid": 12345}))
elif command == "inspect":
    if state.get("malformed"):
        print("{not-json")
    else:
        print(json.dumps({
            "session_id": state.get("session_id", "session_fixture"),
            "state": state.get("state", "idle"), "messages": [],
            "log": "fixture log\n", "exit_code": None,
        }))
else:
    raise SystemExit("unexpected remote operation: " + command)
'''


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def snapshot(paths):
    """Track protected account, SSH, ownership and migration state, not logs."""
    result = {}
    for base in paths:
        if not base.exists():
            result[str(base)] = None
            continue
        for path in [base, *sorted(base.rglob("*"))] if base.is_dir() else [base]:
            result[str(path)] = (
                "directory" if path.is_dir()
                else hashlib.sha256(path.read_bytes()).hexdigest()
            )
    return result


def exercise(binary, root):
    home = root / "home"
    jcode = home / ".jcode"
    runtime = root / "runtime"
    fake_bin = root / "bin"
    for path in (home, jcode / "session_leases", runtime, fake_bin):
        path.mkdir(parents=True, exist_ok=True)
    runtime.chmod(0o700)
    fixture = root / "fixture"
    fixture.mkdir()
    state_path = fixture / "state.json"
    transport = fake_bin / "cloud-transport"
    transport.write_text("#!" + sys.executable + "\n" + textwrap.dedent(TRANSPORT))
    transport.chmod(0o700)
    check(not any(char.isspace() for char in str(transport)),
          "JCODE_SCRATCH_DIR must have no whitespace (transport CLI splits whitespace)")
    # Unexpected SSH/provisioning helpers fail locally and leave evidence.
    for name in ("ssh", "ssh-keygen", "curl", "wget"):
        guard = fake_bin / name
        guard.write_text(
            "#!" + sys.executable + "\n"
            "import os\nfrom pathlib import Path\n"
            "Path(os.environ['CLOUD_CLI_FIXTURE'], 'unexpected-helper').write_text('called')\n"
            "raise SystemExit(91)\n"
        )
        guard.chmod(0o700)
    # Start from an allowlist, not a credential denylist. In particular do not
    # inherit JCODE_*, provider keys, SSH_AUTH_SOCK, proxies, or cloud credentials.
    env = {key: os.environ[key] for key in ("LANG", "LC_ALL", "TZ") if key in os.environ}
    env.update({
        "PATH": str(fake_bin) + ":/usr/bin:/bin",
        "HOME": str(home), "JCODE_HOME": str(jcode),
        "XDG_RUNTIME_DIR": str(runtime), "TMPDIR": str(root),
        "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_DATA_HOME": str(home / ".local/share"),
        "XDG_CACHE_HOME": str(home / ".cache"),
        "JCODE_CLOUD_TRANSPORT": str(transport),
        "CLOUD_CLI_FIXTURE": str(fixture),
        "NO_COLOR": "1", "TERM": "dumb",
    })
    lease = jcode / "session_leases" / (SESSION + ".json")
    lease.write_text(json.dumps({
        "session_id": SESSION, "away_host": "fixture-host", "epoch": 1,
        "updated_at": "2026-11-06T10:34:29Z",
    }))

    def state(**values):
        state_path.write_text(json.dumps(values))

    def run(*args, expected=0):
        result = subprocess.run(
            [str(binary), "--no-update", "--no-selfdev", "cloud", *args],
            cwd=root, env=env, capture_output=True, text=True, timeout=20,
        )
        check(result.returncode == expected,
              f"cloud {args!r}: expected {expected}, got {result.returncode}\n"
              f"stdout: {result.stdout}\nstderr: {result.stderr}")
        return result

    def rejects(*args, contains=None):
        result = subprocess.run(
            [str(binary), "--no-update", "--no-selfdev", "cloud", *args],
            cwd=root, env=env, capture_output=True, text=True, timeout=20,
        )
        check(result.returncode != 0, f"unexpected success: {args!r}")
        if contains:
            check(contains in result.stderr.lower(),
                  f"expected {contains!r}: {result.stderr}")
        return result

    state(state="idle")
    report = json.loads(run("status", "--session", SESSION, "--json").stdout)
    check(report["session_id"] == SESSION and report["state"] == "idle", "bad idle status")
    check(report["messages"] == [] and report["log"] == "fixture log\n", "lost report fields")
    idle = json.loads(run("wait", "--session", SESSION, "--timeout", "0", "--json").stdout)
    check(idle["state"] == "idle", "idle wait did not complete")
    state(state="running")
    running = json.loads(run("wait", "--session", SESSION, "--timeout", "0", "--json", expected=2).stdout)
    check(running["state"] == "running", "timeout lost running status")
    state(state="failed")
    rejects("wait", "--session", SESSION, "--timeout", "0", "--json", contains="cloud turn failed")
    check(run("logs", "--session", SESSION, "--lines", "7").stdout == "fixture log\n", "bad logs")
    marker = root / "must-not-exist"
    prompt = f"$(touch {marker}); 'single' \"double\" `touch {marker}`\nsecond line\\end"
    receipt = json.loads(run("send", "--session", SESSION, "--json", "--", prompt).stdout)
    check(receipt["session_id"] == SESSION and receipt["state"] == "running", "bad send receipt")
    check(json.loads((fixture / "received_prompt.json").read_text()) == prompt, "prompt changed in quoting boundary")
    check(not marker.exists(), "adversarial prompt executed")
    state(state="idle", session_id="session_other")
    rejects("status", "--session", SESSION, "--json", contains="different session")
    state(state="unexpected")
    rejects("status", "--session", SESSION, "--json", contains="unknown session state")
    state(malformed=True)
    rejects("status", "--session", SESSION, "--json", contains="invalid cloud inspect response")

    missing = json.loads(run("problems", "--json").stdout)
    check(missing["problems"] == [] and missing["log_status"] == "missing", "missing problem log not handled")
    logs = jcode / "logs"
    logs.mkdir(exist_ok=True)
    (logs / "cloud-problems.jsonl").write_text("not-json\n" + json.dumps({
        "ts": "2026-11-06T10:34:29Z", "version": "0.99.714", "command": "/cloud --host secret-value",
        "session_id": SESSION, "exit_code": 255, "stderr": "Bearer secret-value permission denied",
    }) + "\n{partial")
    problems = run("problems", "--limit", "999999", "--json")
    parsed = json.loads(problems.stdout)
    check(parsed["malformed_lines"] == 2 and len(parsed["problems"]) == 1, "bad malformed log handling")
    check("secret-value" not in problems.stdout, "problems leaked a secret")
    calls_before = (fixture / "transport.jsonl").read_bytes()
    protected = [lease, jcode / "cloud", jcode / "auth", jcode / "credentials.json", home / ".ssh"]
    before = snapshot(protected)
    doctor = json.loads(run("doctor", "--json").stdout)
    check(doctor["read_only"] is True and doctor["account"] == "not configured", "doctor found inherited credentials")
    check(doctor["moved_sessions"] == 1, "doctor missed moved lease")
    check(snapshot(protected) == before, "doctor mutated protected cloud/account/lease state")
    check((fixture / "transport.jsonl").read_bytes() == calls_before, "doctor executed transport")
    check(not (fixture / "unexpected-helper").exists(), "doctor invoked an SSH/provisioning helper")
    # Hidden inspection must reject path traversal before creating run state.
    run_state = snapshot([jcode / "cloud_runs", jcode / "session_leases"])
    for traversal in ("../escape", "../../outside", "/tmp/escape", ".", "session/child"):
        rejects("inspect", "--session", traversal)
    check(snapshot([jcode / "cloud_runs", jcode / "session_leases"]) == run_state,
          "invalid inspect ID mutated run/lease state")
    check(not marker.exists(), "fixture shell payload was executed")
    print("PASS: status, wait, logs, send quoting, response validation, problems, read-only doctor, inspect traversal")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.expanduser().resolve()
    if not (binary.is_file() and os.access(binary, os.X_OK)):
        parser.error(f"binary is not executable: {binary}")
    scratch_value = os.environ.get("JCODE_SCRATCH_DIR")
    if not scratch_value:
        parser.error("JCODE_SCRATCH_DIR is required for isolated fixture storage")
    scratch = Path(scratch_value).expanduser().resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="cloud-session-cli-", dir=scratch) as temporary:
        exercise(binary, Path(temporary))


if __name__ == "__main__":
    main()
