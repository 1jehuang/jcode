#!/usr/bin/env python3
"""Isolated public-binary plugin lifecycle, profile, and TUI history acceptance.

Prepare only by default. Run AFTER the coordinator grants the runtime slot:
  python3 scripts/tests/plugin_history_acceptance.py --binary /abs/jcode --execute

Uses the sibling profile acceptance Fixture: private HOME/JCODE_HOME/runtime,
Unix sockets, loopback fake provider, no credentials or shared daemon. Every
mutation is inside the retained scratch artifact directory. No build, symlink
publication, network repository, or production configuration writes occur.
"""
import argparse
from contextlib import suppress
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time

from agent_profile_instruction_acceptance import Fixture, Peer, require

PLUGIN = "acceptance-plugin"
MARKET = "acceptance-market"
PROFILE = PLUGIN + ":auditor"
MARKER = "PLUGIN_PROFILE_SENTINEL_902714"
FIRST = "HISTORY_FIRST_SENTINEL_914726"
SECOND = "HISTORY_SECOND_SENTINEL_628413"


def document(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def marketplace(root, name=MARKET, source="./bundle", version="1.0.0", plugin_name=PLUGIN):
    document(root / ".claude-plugin/marketplace.json", {
        "name": name, "plugins": [{"name": plugin_name, "source": source}],
    })
    bundle = root / "bundle"
    document(bundle / ".claude-plugin/plugin.json", {
        "name": plugin_name, "version": version, "description": "Offline acceptance fixture",
    })
    agents = bundle / "agents"
    agents.mkdir(parents=True, exist_ok=True)
    (agents / "auditor.md").write_text(
        "---\nname: Auditor\ndescription: Offline fixture\n---\n"
        + MARKER + "_" + version + "\nReview the fixture only.\n")
    skills = bundle / "skills/probe"
    skills.mkdir(parents=True, exist_ok=True)
    (skills / "SKILL.md").write_text(
        "---\nname: probe\ndescription: Acceptance plugin probe\n---\n"
        "PLUGIN_SKILL_BODY_SENTINEL_813742\nArguments: $ARGUMENTS\n")
    commands = bundle / "commands"
    commands.mkdir(exist_ok=True)
    (commands / "echo.md").write_text("PLUGIN_COMMAND_BODY_SENTINEL_253619\n$ARGUMENTS\n")
    return bundle


class Acceptance:
    def __init__(self, fixture):
        self.f = fixture
        self.testers = []
        self.channels = {}
        self.report = {"status": "running", "checks": [], "skipped": [],
                       "root": str(fixture.root), "binary": str(fixture.binary),
                       "binary_sha256": hashlib.sha256(fixture.binary.read_bytes()).hexdigest()}
        self.calls = (fixture.root / "cli.jsonl").open("w")

    def check(self, label):
        self.report["checks"].append(label)
        print("PASS " + label, flush=True)

    def cli(self, *args, cwd=None, reject=False, plugin=True):
        command = [str(self.f.binary), "--no-update", "--no-selfdev"]
        if plugin:
            command += ["plugin", "--json"]
        command += list(args)
        result = subprocess.run(command, env=self.f.env, cwd=cwd or self.f.cwd,
                                stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=25)
        self.calls.write(json.dumps({"command": command, "cwd": str(cwd or self.f.cwd),
                                    "returncode": result.returncode, "stdout": result.stdout,
                                    "stderr": result.stderr}) + "\n")
        self.calls.flush()
        require((result.returncode != 0) == reject,
                f"unexpected CLI result {command}: {result.returncode}\n{result.stdout}\n{result.stderr}")
        if reject:
            return result
        return json.loads(result.stdout) if plugin else result.stdout.strip()

    def debug(self, command):
        return self.cli("debug", "--socket", str(self.f.endpoint), command, plugin=False)

    def tester(self, tester_id, command, structured=False):
        if command == "frame":
            # ClientDebugRequest forces a fresh draw before screen-json. The
            # legacy file polling path enables capture without requesting draw.
            # This daemon is private and has exactly one owned TUI attachment.
            result = self.debug("client:screen-json")
            self.calls.write(json.dumps({"tester": tester_id, "command": command, "response": result}) + "\n")
            self.calls.flush()
            return json.loads(result) if structured else result
        # Match memory_probe.sh's file channel, clearing stale responses first.
        # The server's convenience tester command does not clear startup replies.
        cmd_path, response_path = self.channels[tester_id]
        response_path.unlink(missing_ok=True)
        cmd_path.write_text("screen-json" if command == "frame" else command)
        deadline = min(self.f.deadline, time.monotonic() + 12)
        previous = None
        while time.monotonic() < deadline:
            try:
                result = response_path.read_text()
            except FileNotFoundError:
                result = ""
            if result and result == previous:
                response_path.unlink(missing_ok=True)
                self.calls.write(json.dumps({"tester": tester_id, "command": command, "response": result}) + "\n")
                self.calls.flush()
                return json.loads(result) if structured else result
            previous = result
            time.sleep(0.05)
        raise TimeoutError(f"private tester response: {command}")

    def wait(self, predicate, label, seconds=20):
        deadline = min(self.f.deadline, time.monotonic() + seconds)
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(0.1)
        raise TimeoutError(label)

    def listed(self, cwd=None):
        value = self.cli("list", cwd=cwd)
        require(isinstance(value, list), "plugin list must emit a JSON array")
        return value

    def provider_turn(self, peer, label):
        before = len(self.f.captures)
        peer.request("message", content="acceptance:" + label)
        require(len(self.f.captures) == before + 1, "expected one loopback provider request")
        return json.dumps(self.f.captures[-1])

    def run_cli(self):
        f = self.f
        market = f.root / "market"
        bundle = marketplace(market)
        sibling = f.root / "other-project"
        sibling.mkdir()
        require(self.listed() == [], "private registry must start empty")
        require(self.cli("validate", str(bundle))["name"] == PLUGIN, "manifest validation failed")
        require(self.cli("marketplace", "add", str(market))["name"] == MARKET, "marketplace add failed")
        require(len(self.cli("marketplace", "list")) == 1, "marketplace list failed")
        installed = self.cli("install", PLUGIN + "@" + MARKET)
        require(installed["enabled"] and installed["scope"] == "user", "install scope/enablement mismatch")
        require(Path(installed["root"]).is_relative_to(f.jhome), "install escaped private JCODE_HOME")
        self.cli("install", PLUGIN + "@" + MARKET, reject=True)
        require(len(self.listed()) == 1, "duplicate install changed registry")
        self.check("public CLI validates, registers, installs, and rejects duplicate installs offline")

        for scope in ("project", "local"):
            self.cli("install", PLUGIN + "@" + MARKET, "--scope", scope)
        require({p["scope"] for p in self.listed()} == {"user", "project", "local"}, "scoped installs missing")
        require([p["scope"] for p in self.listed(sibling)] == ["user"], "project/local install leaked to sibling cwd")
        for scope in ("local", "project"):
            self.cli("uninstall", PLUGIN + "@" + MARKET, "--scope", scope)
        require(len(self.listed()) == 1, "scope-specific uninstall removed user install")
        self.check("project and local CLI scopes stay in their project; removal preserves the user install")

        peer = Peer(f)
        sid = peer.subscribe()
        require(PROFILE in peer.agent()["available"], "enabled plugin agent missing from live runtime")
        result = peer.request("runtime_command", "runtime_command_result", command="agent", args=[PROFILE])
        require(result["data"]["current"] == PROFILE, "qualified plugin profile not selected")
        require("applied" in result["message"].lower() and "next turn" in result["message"].lower(),
                "selection did not positively confirm instruction application")
        require("frontmatter" not in result["message"].lower(), "informational YAML metadata caused false warning")
        require(self.provider_turn(peer, "plugin-v1").count(MARKER + "_1.0.0") == 1,
                "plugin instructions absent or duplicated in actual provider request")
        self.check("public profile notice distinguishes ignored settings from applied Markdown")
        self.check("plugin agent selection activates its Markdown exactly once in actual provider payload")
        profile_path = f.cwd / ".jcode/agents/metadata.md"
        profile_path.write_text("---\nname: Metadata\nmodel: inherit\ntools: [Read]\n---\nMETADATA_BODY_SENTINEL_469813\n")
        result = peer.request("runtime_command", "runtime_command_result", command="agent", args=["metadata"])
        require("Ignored YAML frontmatter fields: model, tools" in result["message"], "ignored YAML keys were not identified")
        require("still applies" in result["message"] and "METADATA_BODY_SENTINEL_469813" in self.provider_turn(peer, "yaml-body"),
                "unsupported YAML metadata prevented body application")
        before_profile = f.saved(sid)["agent_profile"]
        profile_path.write_text("---\nname: [unterminated\n---\nnot-applied\n")
        peer.agent("reload", reject=True)
        require(f.saved(sid)["agent_profile"] == before_profile, "invalid YAML reload replaced selected snapshot")
        self.check("unsupported YAML keys warn without rejecting body; malformed reload is atomic")
        peer.agent("off")
        self.cli("disable", PLUGIN + "@" + MARKET)
        require(PROFILE not in peer.agent()["available"], "disable left plugin profile discoverable")
        peer.agent(PROFILE, reject=True)
        require(peer.agent()["current"] is None, "failed disabled selection changed active profile")
        self.cli("enable", PLUGIN + "@" + MARKET)
        require(PROFILE in peer.agent()["available"], "enable did not invalidate profile discovery")
        self.check("CLI disable/enable affects an already-running daemon without restarting it")

        # Refresh catalog separately, then prove invalid plugin update retains the old install.
        (bundle / ".claude-plugin/plugin.json").write_text("{invalid-json\n")
        self.cli("marketplace", "update", MARKET)
        self.cli("update", PLUGIN + "@" + MARKET, reject=True)
        require(self.listed()[0]["root"] == installed["root"], "failed update replaced working install")
        require(Path(installed["root"]).is_dir(), "failed update removed working files")
        peer.agent(PROFILE)
        require(MARKER + "_1.0.0" in self.provider_turn(peer, "failed-update"), "failed update broke active runtime")
        peer.agent("off")
        marketplace(market, version="2.0.0")
        self.cli("marketplace", "update", MARKET)
        replacement = self.cli("update", PLUGIN + "@" + MARKET)
        require(replacement["version"] == "2.0.0" and replacement["root"] != installed["root"], "update did not publish replacement")
        peer.agent(PROFILE)
        payload = self.provider_turn(peer, "plugin-v2")
        require(MARKER + "_2.0.0" in payload and MARKER + "_1.0.0" not in payload, "stale plugin profile after update")
        peer.agent("off")
        require(f.saved(sid).get("agent_profile") is None, "profile off did not persist")
        self.check("invalid update preserves the working install; successful update refreshes live profile bytes")

        for index, kind in enumerate(("relative", "absolute", "url", "github")):
            name = f"escape-market-{index}"
            hostile_plugin = f"escape-plugin-{index}"
            # A real, valid outside plugin makes success a containment defect,
            # rather than letting a missing path/manifest accidentally reject it.
            outside = marketplace(f.root / f"outside-{index}", name=f"outside-market-{index}",
                                  plugin_name=hostile_plugin)
            source = {
                "relative": "../outside/bundle",
                "absolute": str(outside),
                "url": {"source": "url", "url": str(outside)},
                "github": {"source": "github", "repo": str(outside)},
            }[kind]
            hostile = f.root / name
            marketplace(hostile, name=name, source=source, plugin_name=hostile_plugin)
            self.cli("marketplace", "add", str(hostile))
            before = self.listed()
            self.cli("install", hostile_plugin + "@" + name, reject=True)
            require(self.listed() == before, "rejected escaping source changed registry")
            self.cli("marketplace", "remove", name)
        self.check("relative, absolute and disguised remote local sources reject without registry mutation")

        # A URL-shaped Unix path must never replace a real remote acquisition.
        # Stub git, so this check cannot contact any network even after the fix.
        shaped_name = "url-shaped-plugin"
        shaped_url = "https://example.invalid/repo.git"
        shaped_root = f.cwd / "https:/example.invalid/repo.git"
        document(shaped_root / ".claude-plugin/plugin.json", {"name": shaped_name})
        shaped_market = f.root / "url-shaped-market"
        marketplace(shaped_market, name="url-shaped-market", plugin_name=shaped_name,
                    source={"source": "url", "url": shaped_url})
        self.cli("marketplace", "add", str(shaped_market))
        stub_bin = f.root / "stub-bin"
        stub_bin.mkdir()
        called = f.root / "git-clone-attempted"
        git = stub_bin / "git"
        git.write_text("#!/bin/sh\nprintf called > " + shlex.quote(str(called)) + "\nexit 17\n")
        git.chmod(0o700)
        old_path = f.env.get("PATH", "")
        try:
            f.env["PATH"] = str(stub_bin) + os.pathsep + old_path
            before = self.listed()
            self.cli("install", shaped_name + "@url-shaped-market", reject=True)
            require(called.is_file(), "remote URL was accepted as a local URL-shaped directory")
            require(self.listed() == before, "failed remote acquisition changed registry")
        finally:
            f.env["PATH"] = old_path
        self.cli("marketplace", "remove", "url-shaped-market")
        self.check("remote acquisition cannot fall back to a URL-shaped workspace directory; git stub prevents network")

        # Leave the main plugin installed for the optional TUI discovery check.
        return peer

    def run_tui(self):
        f = self.f
        # The toggle is private and debug_control_allowed() re-reads it per command.
        (f.jhome / "debug_control").touch(mode=0o600)
        wrapper = f.root / "client"
        args = [str(f.binary), "--no-update", "--no-selfdev", "--socket", str(f.endpoint),
                "--provider-profile", "acceptance", "--model", "acceptance-no-inference"]
        wrapper.write_text("#!/bin/sh\nexec " + shlex.join(args) + ' "$@"\n')
        wrapper.chmod(0o700)
        spawned = json.loads(self.debug("tester:spawn " + json.dumps({
            "binary": str(wrapper), "cwd": str(f.cwd), "cols": 120, "rows": 40})))
        tid = spawned["id"]
        self.testers.append(spawned)
        info = next(row for row in json.loads(self.debug("tester:list")) if row["id"] == tid)
        self.testers[-1].update(info)
        paths = tuple(Path(info[key]) for key in ("debug_cmd_path", "debug_response_path"))
        require(all(path.parent.resolve() == (f.root / "tmp").resolve() for path in paths),
                "tester channels escaped private TMPDIR")
        self.channels[tid] = paths
        self.wait(lambda: self.tester(tid, "state", True).get("server_version"), "tester startup")
        self.tester(tid, "enable")
        self.tester(tid, "frame", True)
        self.plugin_output_start = Path(info["stdout_path"]).stat().st_size
        self.tester(tid, "message:/plugin")
        frame = self.wait(lambda: self.frame_with(tid, PLUGIN), "plugin dialog frame")
        (f.root / "plugin-frame.json").write_text(json.dumps(frame, indent=2) + "\n")
        self.tester(tid, "keys:esc")
        self.check("actual isolated TUI /plugin frame lists the registered local fixture")
        for invocation, body in (("probe", "PLUGIN_SKILL_BODY_SENTINEL_813742"),
                                 ("echo", "PLUGIN_COMMAND_BODY_SENTINEL_253619")):
            before = len(f.captures)
            self.tester(tid, "message:/" + PLUGIN + ":" + invocation + " PLUGIN_ARGUMENT_SENTINEL_176429")
            self.wait(lambda: len(f.captures) > before and not self.tester(tid, "state", True)["processing"],
                      "actual plugin slash invocation")
            payload = json.dumps(f.captures[-1])
            require(body in payload and "PLUGIN_ARGUMENT_SENTINEL_176429" in payload,
                    "plugin slash invocation did not activate its actual body and arguments")
        self.check("actual TUI slash invocation expands plugin skills and legacy commands into provider payloads")

        for prompt in (FIRST, SECOND):
            before = len(f.captures)
            self.tester(tid, "message:" + prompt)
            self.wait(lambda: len(f.captures) > before and not self.tester(tid, "state", True)["processing"], "fixture prompt completion")
        (f.root / "pre-rewind-history.json").write_text(
            json.dumps(self.tester(tid, "history", True), indent=2) + "\n")
        journal_dir = f.jhome / "prompt-history"
        (f.root / "pre-rewind-journals.json").write_text(json.dumps(
            {path.name: path.read_text() for path in journal_dir.glob("*.jsonl")}, indent=2) + "\n")
        self.tester(tid, "message:/rewind 1")
        self.wait(lambda: not any(m["role"] == "user" and SECOND in m["content"]
                                 for m in self.tester(tid, "history", True)), "rewind must remove second prompt from actual transcript")
        self.tester(tid, "set_input:")
        self.tester(tid, "keys:up")
        recalled = self.tester(tid, "state", True)["input"]
        require(recalled == SECOND, f"rewind changed newest recall order: expected {SECOND!r}, got {recalled!r}")
        journal_contents = {path.name: path.read_text() for path in journal_dir.glob("*.jsonl")}
        (f.root / "post-rewind-journals.json").write_text(json.dumps(journal_contents, indent=2) + "\n")
        matching_journals = [[json.loads(line) for line in text.splitlines()]
                             for text in journal_contents.values() if SECOND in text]
        require(len(matching_journals) == 1 and matching_journals[0][-1] == SECOND,
                "rewind preserved in-memory recall but changed persisted newest-prompt order")
        self.tester(tid, "keys:down")
        require(self.tester(tid, "state", True)["input"] == "", "Down did not restore empty draft")
        self.tester(tid, "keys:ctrl+r")
        self.tester(tid, "keys:" + ",".join(SECOND))
        require(self.tester(tid, "state", True)["input"] == SECOND, "Ctrl+R cannot find rewound prompt")
        search = self.wait(lambda: self.tester(tid, "frame", True), "history search frame")
        (f.root / "history-search-frame.json").write_text(json.dumps(search, indent=2) + "\n")
        self.tester(tid, "keys:esc")
        self.check("real rewind removes transcript entry but preserves Up/Down and Ctrl+R recall")

        before_sessions = set(f.jhome.joinpath("sessions").glob("*.json"))
        self.tester(tid, "message:/clear")
        self.wait(lambda: set(f.jhome.joinpath("sessions").glob("*.json")) - before_sessions,
                  "new session persistence")
        self.wait(lambda: not any(m["role"] == "user" and FIRST in m["content"]
                                 for m in self.tester(tid, "history", True)), "new session transcript")
        self.tester(tid, "set_input:")
        self.tester(tid, "keys:up")
        require(self.tester(tid, "state", True)["input"] == "", "new session leaked previous prompt history")
        self.check("new session does not expose previous session's prompt journal")

    def frame_with(self, tid, marker):
        # Visual JSON captures the base transcript, not modal dialog overlays.
        # Verify actual PTY rendering instead of inventing a dialog JSON field.
        frame = self.tester(tid, "frame", True)
        info = next(row for row in self.testers if row["id"] == tid)
        raw = Path(info["stdout_path"]).read_bytes()[self.plugin_output_start:]
        text = raw.decode("utf-8", errors="replace")
        required = (marker + "@" + MARKET, "Plugins", "Discover", "Installed", "Marketplaces", "Errors")
        if all(part in text for part in required):
            (self.f.root / "plugin-frame.ansi").write_bytes(raw)
            return {"visual_frame": frame, "terminal_output": text,
                    "evidence": "actual PTY render; visual JSON does not include dialog overlays"}
        return None

    def finish_lifecycle(self, peer):
        self.cli("uninstall", PLUGIN + "@" + MARKET)
        require(self.listed() == [], "uninstall left plugin in installed list")
        require(PROFILE not in peer.agent()["available"], "uninstall left cached plugin profile")
        peer.agent(PROFILE, reject=True)
        self.cli("marketplace", "remove", MARKET)
        require(self.cli("marketplace", "list") == [], "marketplace remove left registry entry")
        self.check("uninstall removes live runtime discovery; marketplace removal stays scoped")

    def close(self):
        errors = []
        for tester in self.testers:
            try:
                self.debug(f"tester:{tester['id']}:stop")
            except BaseException as error:
                errors.append(repr(error))
                with suppress(ProcessLookupError):
                    os.kill(tester["pid"], signal.SIGTERM)
        self.calls.close()
        cleanup = self.f.close()
        cleanup["tester_errors"] = errors
        return cleanup


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--skip-tui", action="store_true", help="CLI/profile acceptance only; reports the TUI gap")
    parser.add_argument("--diagnostic-hold-seconds", type=int, default=0,
                        help="On failure, retain the private live tester briefly for inspection, max 120 seconds")
    args = parser.parse_args()
    if not args.execute:
        print("Prepared only. Runtime requires coordinator slot and --execute.")
        return
    require(args.binary.is_absolute() and args.binary.is_file() and not args.binary.is_symlink(),
            "explicit built binary required, not the installed launcher")
    signal.signal(signal.SIGALRM, lambda *_: (_ for _ in ()).throw(TimeoutError("acceptance global deadline")))
    signal.alarm(330)
    acceptance = Acceptance(Fixture(args.binary, seconds=300))
    try:
        peer = acceptance.run_cli()
        if args.skip_tui:
            acceptance.report["skipped"].append("actual TUI plugin frame and history rewind/session isolation")
        else:
            acceptance.run_tui()
        acceptance.finish_lifecycle(peer)
        acceptance.report["status"] = "passed"
    except BaseException as error:
        acceptance.report.update(status="failed", error=repr(error))
        for tester in acceptance.testers:
            for command in ("state", "history", "frame"):
                with suppress(BaseException):
                    raw = acceptance.tester(tester["id"], command)
                    (acceptance.f.root / ("failure-" + command + ".txt")).write_text(raw)
        if args.diagnostic_hold_seconds:
            print("Diagnostic private fixture: " + str(acceptance.f.root), flush=True)
            time.sleep(max(0, min(args.diagnostic_hold_seconds, 120)))
        raise
    finally:
        signal.alarm(0)
        acceptance.report["cleanup"] = acceptance.close()
        cleanup = acceptance.report["cleanup"]
        if cleanup["errors"] or cleanup["tester_errors"] or not cleanup["reaped"] or not cleanup["http_stopped"]:
            acceptance.report["status"] = "failed"
        (acceptance.f.root / "plugin-history-report.json").write_text(json.dumps(acceptance.report, indent=2) + "\n")
        print("Artifacts: " + str(acceptance.f.root), flush=True)
        require(acceptance.report["status"] == "passed", "acceptance failed; inspect report and wire/CLI artifacts")


if __name__ == "__main__":
    main()
