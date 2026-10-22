#!/usr/bin/env python3
"""One gate for isolated cloud handoff acceptance. See docs/cloud-acceptance.md."""
import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback
import xml.etree.ElementTree as ET

from cloud_acceptance_tui import CloudTUI, SCENARIOS

ROOT = Path(__file__).resolve().parent.parent


def preflight(layers):
    required = {"bash", "git", "python3"}
    if "cli" in layers or "agent" in layers:
        required.add("bwrap")
    if "tui" in layers:
        required.update(("ssh", "sshd", "ssh-keygen", "tmux", "unshare", "mount"))
    missing = sorted(name for name in required if not shutil.which(name))
    if missing:
        raise RuntimeError("Missing test prerequisites: " + ", ".join(missing))
    if os.geteuid() == 0:
        raise RuntimeError("Run cloud acceptance as an ordinary user, not root")
    probes = []
    if "tui" in layers:
        probes.append(["unshare", "--user", "--map-root-user", "--mount", "true"])
    if "cli" in layers or "agent" in layers:
        probes.append(["bwrap", "--ro-bind", "/", "/", "--unshare-pid", "--proc", "/proc", "true"])
    for probe in probes:
        result = subprocess.run(probe, text=True, capture_output=True, timeout=10)
        if result.returncode:
            raise RuntimeError(f"Isolation unavailable ({' '.join(probe)}): {result.stderr.strip()}")


def child(command, env, logfile, timeout):
    with logfile.open("w") as log:
        process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            raise AssertionError(f"Stage timed out after {timeout}s. See {logfile}")
    if code:
        raise AssertionError(f"Stage exited {code}. See {logfile}")


def write_report(path, results, binary, version):
    report = {"binary": binary, "version": version, "results": results,
              "limitations": ["No hosted VM provisioning/suspend/resume or production OAuth service is exercised.",
                              "The provider is deterministic; SSH, TUI, daemons and transfer code are real."]}
    (path / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    suite = ET.Element("testsuite", name="cloud-acceptance", tests=str(len(results)),
                       failures=str(sum(r["status"] != "passed" for r in results)))
    for result in results:
        case = ET.SubElement(suite, "testcase", name=result["name"], time=str(result["seconds"]))
        if result["status"] != "passed":
            ET.SubElement(case, "failure", message=result["error"]).text = result.get("traceback", "")
        ET.SubElement(case, "system-out").text = result["artifacts"]
    ET.ElementTree(suite).write(path / "junit.xml", encoding="utf-8", xml_declaration=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=os.environ.get("JCODE_BIN"), required=not os.environ.get("JCODE_BIN"))
    parser.add_argument("--layers", nargs="+", choices=("cli", "agent", "credentials", "tui"),
                        default=["cli", "agent", "credentials", "tui"])
    parser.add_argument("--scenario", choices=tuple(SCENARIOS), help="Run one TUI scenario (with --layers tui)")
    parser.add_argument("--repeat", type=int, default=1, help="Repeat each selected stage to expose state leakage")
    parser.add_argument("--artifacts", type=Path, help="New directory for retained logs, homes and reports")
    parser.add_argument("--timeout", type=int, default=240, help="Timeout per CLI/agent/credential stage")
    args = parser.parse_args()
    if args.repeat < 1 or args.timeout < 1:
        parser.error("--repeat and --timeout must be positive")
    binary = str(Path(args.binary).resolve(strict=True))
    if args.artifacts:
        artifacts = args.artifacts.resolve()
        artifacts.mkdir(parents=True, exist_ok=False)
    else:
        parent = os.environ.get("JCODE_SCRATCH_DIR", tempfile.gettempdir())
        artifacts = Path(tempfile.mkdtemp(prefix="cloud-acceptance-", dir=parent))
    os.chmod(artifacts, 0o700)
    results = []
    version = subprocess.run([binary, "--no-update", "--version"], capture_output=True,
                             text=True, timeout=10).stdout.strip()
    print(f"Testing {version}: {binary}\nArtifacts: {artifacts}", flush=True)
    try:
        preflight(args.layers)
    except Exception as exc:
        results.append({"name": "preflight", "status": "failed", "seconds": 0,
                        "error": str(exc), "artifacts": str(artifacts)})
        write_report(artifacts, results, binary, version)
        print(f"FAIL preflight: {exc}", file=sys.stderr)
        return 1
    for iteration in range(1, args.repeat + 1):
        stages = [(layer, None) for layer in args.layers if layer != "tui"]
        if "tui" in args.layers:
            stages += [("tui", name) for name in ([args.scenario] if args.scenario else SCENARIOS)]
        for layer, scenario in stages:
            name = f"{layer}.{scenario or 'suite'}.{iteration}"
            directory = artifacts / name
            started = time.monotonic()
            record = {"name": name, "artifacts": str(directory), "status": "passed"}
            fixture = None
            print(f"RUN {name}", flush=True)
            try:
                if layer == "tui":
                    fixture = CloudTUI(binary, directory)
                    fixture.start()
                    SCENARIOS[scenario](fixture)
                else:
                    directory.mkdir()
                    # Do not pass caller session/account/transport state to a test.
                    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                           "HOME": str(directory), "LANG": "C.UTF-8", "JCODE_BIN": binary,
                           "JCODE_SCRATCH_DIR": str(directory), "TMPDIR": str(directory),
                           "JCODE_NO_AUTO_UPDATE": "1", "JCODE_NO_TELEMETRY": "1"}
                    script = {"cli": "scripts/test_cloud_move_e2e.sh",
                              "agent": "scripts/test_cloud_move_agent_e2e.sh",
                              "credentials": "tests/test_cloud_credentials.py"}[layer]
                    command = ([sys.executable, script, binary] if layer == "credentials"
                               else ["bash", script])
                    child(command, env, directory / "output.log", args.timeout)
            except Exception as exc:
                record.update(status="failed", error=str(exc), traceback=traceback.format_exc())
                print(f"FAIL {name}: {exc}", flush=True)
            finally:
                if fixture:
                    try:
                        fixture.close()
                    except Exception as exc:
                        cleanup_error = f"Cleanup failed: {exc}"
                        record.update(status="failed",
                                      error=(record["error"] + "\n" + cleanup_error
                                             if "error" in record else cleanup_error),
                                      traceback=record.get("traceback", "") + traceback.format_exc())
                record["seconds"] = round(time.monotonic() - started, 3)
                results.append(record)
                write_report(artifacts, results, binary, version)
            if record["status"] == "passed":
                print(f"PASS {name} ({record['seconds']}s)", flush=True)
    failed = sum(result["status"] != "passed" for result in results)
    print(f"\n{len(results) - failed} passed, {failed} failed. Report: {artifacts / 'report.json'}", flush=True)
    return int(failed != 0)


if __name__ == "__main__":
    sys.exit(main())
