#!/usr/bin/env python3
"""Verify real Firefox cold startup without touching user profiles or bridge ports.

Run under unshare with private PID/network namespaces; see
../docs/BROWSER_COLD_START_VALIDATION.md. No browser or bridge is mocked.
"""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host-netns", required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--jcode", type=Path)
    parser.add_argument("--bridge-dir", type=Path)
    args = parser.parse_args()
    if sys.platform != "linux" or os.getpid() != 1:
        parser.error("run as PID 1 in a private Linux PID namespace")
    if os.readlink("/proc/self/ns/net") == args.host_netns:
        parser.error("a private network namespace is required")

    source_home = Path.home()
    binary = (args.jcode or source_home / ".jcode/builds/shared-server/jcode").resolve()
    source_bridge = args.bridge_dir or source_home / ".jcode/browser"
    args.output_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="firefox-cold-", dir=args.output_dir))
    home = root / "home"
    bridge = home / ".jcode/browser"
    profile = home / ".mozilla/firefox/acceptance.default"
    runtime = root / "runtime"
    for directory in (bridge, profile / "extensions", runtime):
        directory.mkdir(parents=True, exist_ok=True)
    runtime.chmod(0o700)
    for name in ("browser", "firefox-agent-bridge-host", "browser-agent-bridge.xpi"):
        shutil.copy2(source_bridge / name, bridge / name)
    (bridge / ".setup-complete").write_text("previous successful setup\n")
    (bridge / ".browser").write_text("firefox")
    with zipfile.ZipFile(bridge / "browser-agent-bridge.xpi") as archive:
        manifest = json.loads(archive.read("manifest.json"))
    extension_id = manifest["browser_specific_settings"]["gecko"]["id"]
    shutil.copy2(bridge / "browser-agent-bridge.xpi", profile / "extensions" / (extension_id + ".xpi"))
    (home / ".mozilla/firefox/profiles.ini").write_text(
        "[General]\nStartWithLastProfile=1\nVersion=2\n\n"
        "[Profile0]\nName=acceptance\nIsRelative=1\nPath=acceptance.default\nDefault=1\n"
    )
    (profile / "user.js").write_text('''user_pref("extensions.autoDisableScopes", 0);
user_pref("extensions.enabledScopes", 15);
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.startup.homepage", "about:blank");
user_pref("browser.startup.page", 0);
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("datareporting.policy.dataSubmissionEnabled", false);
user_pref("toolkit.telemetry.reportingpolicy.firstRun", false);
user_pref("browser.aboutwelcome.enabled", false);
''')
    host_dir = home / ".mozilla/native-messaging-hosts"
    host_dir.mkdir(parents=True)
    (host_dir / "firefox_agent_bridge.json").write_text(json.dumps({
        "allowed_extensions": [extension_id, "firefox-agent-bridge@local"],
        "description": "Isolated acceptance native host",
        "name": "firefox_agent_bridge",
        "path": str(bridge / "firefox-agent-bridge-host"),
        "type": "stdio",
    }))
    env = {
        "PATH": "/usr/bin:/bin", "HOME": str(home), "JCODE_HOME": str(home / ".jcode"),
        "XDG_CONFIG_HOME": str(home / ".config"), "XDG_CACHE_HOME": str(home / ".cache"),
        "XDG_RUNTIME_DIR": str(runtime), "LANG": "C.UTF-8", "MOZ_HEADLESS": "1",
        "MOZ_LEGACY_PROFILES": "1", "MOZ_DISABLE_CONTENT_SANDBOX": "1",
        "JCODE_BROWSER": "firefox", "JCODE_BROWSER_AUTO_UPDATE": "0", "JCODE_NO_TELEMETRY": "1",
    }
    subprocess.run(["/usr/bin/ip", "link", "set", "lo", "up"], check=True)
    zombie = subprocess.Popen(["/bin/sh", "-c", "printf firefox > /proc/self/comm; exit 0"], env=env)
    try:
        for _ in range(100):
            stat = Path(f"/proc/{zombie.pid}/stat").read_text()
            if stat.rsplit(")", 1)[1].split()[0] == "Z":
                break
            time.sleep(0.01)
        else:
            raise RuntimeError("failed to establish zombie precondition")
        print(json.dumps({"root": str(root), "binary": str(binary), "zombie_pid": zombie.pid,
                          "zombie_state": "Z", "extension_version": manifest["version"]}), flush=True)
        setup = subprocess.run([str(binary), "browser", "setup", "firefox"], env=env,
                               capture_output=True, text=True, timeout=85)
        status = subprocess.run([str(binary), "browser", "status", "firefox"], env=env,
                                capture_output=True, text=True, timeout=25)
        tabs = subprocess.run([str(bridge / "browser"), "listTabs"], env=env,
                              capture_output=True, text=True, timeout=15)
        for label, result in (("setup", setup), ("status", status), ("tabs", tabs)):
            (root / (label + ".stdout")).write_text(result.stdout)
            (root / (label + ".stderr")).write_text(result.stderr)
            print(result.stdout, flush=True)
            print(result.stderr, file=sys.stderr, flush=True)
        tab_data = json.loads(tabs.stdout) if tabs.returncode == 0 else {}
        passed = (
            setup.returncode == 0
            and "was launched and the browser bridge reconnected" in setup.stdout
            and "extension install prompt" not in setup.stdout
            and "Built-in browser tool is ready" in status.stdout
            and status.returncode == 0
            and tab_data.get("totalTabs") == 1
            and tab_data["windows"][0]["tabs"][0]["url"] == "about:blank"
        )
        evidence = {"passed": passed, "setup_exit": setup.returncode, "status_exit": status.returncode,
                    "tabs_exit": tabs.returncode, "root": str(root), "binary": str(binary)}
        (root / "result.json").write_text(json.dumps(evidence, indent=2))
        print(json.dumps(evidence), flush=True)
        return 0 if passed else 1
    finally:
        zombie.wait()


if __name__ == "__main__":
    sys.exit(main())
