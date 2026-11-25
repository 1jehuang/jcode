#!/usr/bin/env python3
"""Run the actual plugin CLI in a disposable home, with no network or providers."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    binary = parser.parse_args().binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="jcode-plugin-catalog-") as temporary:
        root = Path(temporary)
        home = root / "jcode"
        project = root / "project"
        home.mkdir()
        project.mkdir()
        config = home / "config.toml"
        config.write_text("deliberately malformed = [\n")
        original_config = config.read_bytes()
        env = {
            "PATH": os.environ["PATH"],
            "HOME": str(root / "home"),
            "XDG_CONFIG_HOME": str(root / "config"),
            "JCODE_HOME": str(home),
            "JCODE_TEST_SESSION": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_ALLOW_PROTOCOL": "file",
            "GIT_TERMINAL_PROMPT": "0",
            "LANG": "C.UTF-8",
        }

        def cli(*args, reject=False, scope="project", directory=project):
            result = subprocess.run(
                [str(binary), "--no-update", "--no-selfdev", "--cwd", str(directory),
                 "plugin", *args, "--scope", scope, "--json"],
                cwd=root, env=env, stdin=subprocess.DEVNULL,
                capture_output=True, text=True, timeout=25,
            )
            assert (result.returncode != 0) == reject, (
                args, result.returncode, result.stdout, result.stderr
            )
            value = json.loads(result.stdout)
            assert ("error" in value if isinstance(value, dict) else False) == reject
            return value

        assert cli("list") == []
        assert not (home / "plugin-catalogs").exists(), "listing must be inert"
        market = root / "catalog"
        write_json(market / ".claude-plugin/marketplace.json", {
            "name": "fixture", "plugins": [
                {"name": "main", "source": "./main"},
                {"name": "dependency", "source": "./dependency"},
            ],
        })
        write_json(market / "main/.claude-plugin/plugin.json", {
            "name": "main", "version": "1.0.0", "dependencies": ["dependency"],
        })
        write_json(market / "dependency/.claude-plugin/plugin.json", {
            "name": "dependency", "version": "1.0.0",
        })
        assert cli("marketplace", "add", str(market))["name"] == "fixture"
        installed = cli("install", "main@fixture")
        assert installed["scope"] == "project"
        assert Path(installed["project"]).resolve() == project.resolve()
        assert len(cli("list")) == 2
        other = root / "other-project"
        other.mkdir()
        assert cli("list", directory=other) == []

        registry = home / "plugin-catalogs/registry.json"
        before_rejected_install = registry.read_bytes()
        cli("install", "missing@fixture", reject=True)
        assert registry.read_bytes() == before_rejected_install
        cli("disable", "main@fixture")
        write_json(market / "main/.claude-plugin/plugin.json", {
            "name": "main", "version": "2.0.0", "dependencies": ["dependency"],
        })
        cli("marketplace", "update", "fixture")
        updated = cli("update", "main@fixture")
        assert updated["version"] == "2.0.0" and not updated["enabled"]
        assert updated["root"] != installed["root"]
        assert Path(installed["root"]).exists(), "updates preserve immutable versions"
        cli("enable", "main@fixture")
        assert next(p for p in cli("list") if p["name"] == "main")["enabled"]

        invalid = root / "invalid/.claude-plugin/plugin.json"
        write_json(invalid, {"name": "invalid", "skills": "../../outside"})
        before_rejected_validation = registry.read_bytes()
        cli("validate", str(invalid.parent.parent), reject=True)
        assert registry.read_bytes() == before_rejected_validation
        cli("uninstall", "main@fixture")
        cli("uninstall", "dependency@fixture")
        assert cli("list") == []
        cli("marketplace", "remove", "fixture")
        assert cli("marketplace", "list") == []
        assert config.read_bytes() == original_config
        assert {p.name for p in home.iterdir()} == {"config.toml", "plugin-catalogs"}
        print("PASS: actual CLI lifecycle, rollback, scope, immutable update, inert startup")


if __name__ == "__main__":
    main()
