#!/usr/bin/env python3
"""Private public-CLI acceptance for plugin Git sources and dependency graphs.

No network: Git URL rewriting points at a private local repository, with every
protocol except file disabled. No installed launcher or user configuration is
mutated. Requires an explicit newly built binary and --execute.
"""
import argparse
import json
from pathlib import Path
import subprocess

from agent_profile_instruction_acceptance import Fixture, require
from plugin_history_acceptance import Acceptance, document


def git(args, cwd, env):
    result = subprocess.run(["git", *args], cwd=cwd, env=env,
                            capture_output=True, text=True, timeout=20)
    require(result.returncode == 0, f"fixture git {args}: {result.stderr}")
    return result.stdout.strip()


def configured_identity():
    identity = []
    for field in ("user.name", "user.email"):
        result = subprocess.run(["git", "config", "--get", field],
                                capture_output=True, text=True, timeout=5)
        require(result.returncode == 0 and result.stdout.strip(),
                f"fixture commits require configured {field}; no identity is invented")
        identity.append(result.stdout.strip())
    return identity


def git_sources(a):
    f = a.f
    repo = f.root / "source-repository"
    repo.mkdir()
    name, email = configured_identity()
    git(["init", "-b", "main"], repo, f.env)
    git(["config", "user.name", name], repo, f.env)
    git(["config", "user.email", email], repo, f.env)
    for location, plugin in ((repo, "pin-root"), (repo / "packages/sub", "pin-sub")):
        document(location / ".claude-plugin/plugin.json", {"name": plugin, "version": "1.0.0"})
        (location / "marker.txt").write_text("first revision\n")
    git(["add", "."], repo, f.env)
    git(["commit", "-m", "Create private plugin source fixture"], repo, f.env)
    first = git(["rev-parse", "HEAD"], repo, f.env)
    git(["tag", "fixture-v1"], repo, f.env)
    (repo / "marker.txt").write_text("second revision\n")
    (repo / "packages/sub/marker.txt").write_text("second nested revision\n")
    git(["add", "."], repo, f.env)
    git(["commit", "-m", "Advance private plugin fixture"], repo, f.env)
    second = git(["rev-parse", "HEAD"], repo, f.env)
    require(first != second, "Git fixture did not advance")

    url = "https://fixture.invalid/source.git"
    git_config = f.root / "private-gitconfig"
    git_config.write_text('[url ' + json.dumps(repo.as_uri()) + ']\n'
                          '    insteadOf = ' + url + '\n')
    f.env.update(GIT_CONFIG_GLOBAL=str(git_config), GIT_CONFIG_NOSYSTEM="1",
                 GIT_ALLOW_PROTOCOL="file", GIT_TERMINAL_PROMPT="0")
    market = f.root / "git-market"
    manifest = market / ".claude-plugin/marketplace.json"

    def catalog(root_source, nested_source=None):
        rows = [{"name": "pin-root", "source": root_source}]
        if nested_source is not None:
            rows.append({"name": "pin-sub", "source": nested_source})
        document(manifest, {"name": "git-fixture-market", "plugins": rows})

    catalog({"source": "url", "url": url, "ref": "fixture-v1"})
    a.cli("marketplace", "add", str(market))
    installed = a.cli("install", "pin-root@git-fixture-market")
    require((Path(installed["root"]) / "marker.txt").read_text() == "first revision\n",
            "tag source did not install the pinned revision")
    a.check("public CLI acquires an explicitly pinned Git tag without network")

    catalog({"source": "url", "url": url, "ref": "main", "sha": first})
    a.cli("marketplace", "update", "git-fixture-market")
    updated = a.cli("update", "pin-root@git-fixture-market")
    require((Path(updated["root"]) / "marker.txt").read_text() == "first revision\n",
            "SHA pin did not take precedence over branch ref")
    a.check("public CLI verifies exact SHA and honors SHA precedence over ref")

    catalog({"source": "url", "url": url, "ref": "main"},
            {"source": "git-subdir", "url": url, "path": "packages/sub", "ref": "main"})
    a.cli("marketplace", "update", "git-fixture-market")
    updated = a.cli("update", "pin-root@git-fixture-market")
    require((Path(updated["root"]) / "marker.txt").read_text() == "second revision\n",
            "branch update failed to activate current bytes")
    nested = a.cli("install", "pin-sub@git-fixture-market")
    require((Path(nested["root"]) / "marker.txt").read_text() == "second nested revision\n",
            "git-subdir did not select the declared contained plugin")
    require(not (Path(nested["root"]) / "packages").exists(), "whole repository copied instead of subdirectory")
    a.check("public CLI updates a branch and installs a contained Git subdirectory")

    previous = a.listed()
    catalog({"source": "url", "url": url, "sha": "0" * 40})
    a.cli("marketplace", "update", "git-fixture-market")
    a.cli("update", "pin-root@git-fixture-market", reject=True)
    require(a.listed() == previous, "unavailable pinned commit changed installed registry")
    require((Path(updated["root"]) / "marker.txt").read_text() == "second revision\n",
            "failed pin update damaged previously active plugin")
    a.check("failed pinned-source update preserves the active registry and bytes")


def dependency_graphs(a):
    f = a.f
    market = f.root / "dependency-market"

    def plugin(root, name, dependencies=None):
        manifest = {"name": name, "version": "1.0.0"}
        if dependencies is not None:
            manifest["dependencies"] = dependencies
        document(root / ".claude-plugin/plugin.json", manifest)
        (root / "marker.txt").write_text(name + "\n")

    plugin(market / "root", "graph-root", ["graph-mid"])
    plugin(market / "mid", "graph-mid", [{"name": "graph-leaf"}])
    plugin(market / "leaf", "graph-leaf")
    plugin(market / "catalog-leaf", "graph-catalog-leaf")
    document(market / ".claude-plugin/marketplace.json", {
        "name": "dependency-fixture", "plugins": [
            {"name": "graph-root", "source": "./root", "dependencies": ["graph-catalog-leaf"]},
            {"name": "graph-mid", "source": "./mid"},
            {"name": "graph-leaf", "source": "./leaf", "defaultEnabled": False},
            {"name": "graph-catalog-leaf", "source": "./catalog-leaf", "defaultEnabled": False},
        ],
    })
    a.cli("marketplace", "add", str(market))
    a.cli("install", "graph-root@dependency-fixture", "--scope", "project")
    graph = [p for p in a.listed() if p["name"].startswith("graph-")]
    require({p["name"] for p in graph} == {"graph-root", "graph-mid", "graph-leaf", "graph-catalog-leaf"},
            "catalog/manifest dependency closure incomplete")
    require(all(p["scope"] == "project" and p["enabled"] for p in graph),
            "required default-disabled dependencies were not enabled in root scope")
    sibling = f.root / "dependency-other-project"
    sibling.mkdir()
    require(not any(p["name"].startswith("graph-") for p in a.listed(sibling)),
            "dependency scope leaked into another project")
    a.check("public CLI resolves string/object and catalog/manifest transitive dependencies atomically in project scope")

    a.cli("install", "graph-leaf@dependency-fixture", "--scope", "local")
    before = a.listed()
    a.cli("update", "graph-root@dependency-fixture", "--scope", "project", reject=True)
    require(a.listed() == before, "dependency update bypassed a narrower disabled local scope")
    a.cli("uninstall", "graph-leaf@dependency-fixture", "--scope", "local")
    a.check("narrower disabled local scope masks required project dependencies without silent override")

    a.cli("disable", "graph-leaf@dependency-fixture", "--scope", "project")
    before = a.listed()
    a.cli("update", "graph-root@dependency-fixture", "--scope", "project", reject=True)
    require(a.listed() == before, "dependency update silently overrode a disabled plugin")
    a.check("dependency updates preserve disabled user choices and reject without registry mutation")
    a.cli("enable", "graph-leaf@dependency-fixture", "--scope", "project")

    for dependencies, label in ((["missing-node"], "missing"), (["graph-root"], "cycle")):
        plugin(market / "root", "graph-root", dependencies)
        a.cli("marketplace", "update", "dependency-fixture")
        before = a.listed()
        a.cli("update", "graph-root@dependency-fixture", "--scope", "project", reject=True)
        require(a.listed() == before, label + " dependency failure changed installed records")
    a.check("missing and cyclic dependency updates roll back the whole installed graph")

    first = f.root / "cross-market-root"
    second = f.root / "cross-market-dependency"
    third = f.root / "cross-market-third"
    plugin(first / "root", "cross-root", [{"name": "cross-dep", "marketplace": "cross-second"}])
    plugin(second / "dep", "cross-dep", ["cross-third-leaf@cross-third"])
    plugin(third / "leaf", "cross-third-leaf")

    def cross_catalog(allow):
        document(first / ".claude-plugin/marketplace.json", {
            "name": "cross-first", "allowCrossMarketplaceDependenciesOn": allow,
            "plugins": [{"name": "cross-root", "source": "./root"}],
        })

    cross_catalog(["cross-second"])
    document(second / ".claude-plugin/marketplace.json", {
        "name": "cross-second", "allowCrossMarketplaceDependenciesOn": ["cross-third"],
        "plugins": [{"name": "cross-dep", "source": "./dep"}],
    })
    document(third / ".claude-plugin/marketplace.json", {
        "name": "cross-third", "plugins": [{"name": "cross-third-leaf", "source": "./leaf"}],
    })
    for source in (first, second, third):
        a.cli("marketplace", "add", str(source))
    before = a.listed()
    a.cli("install", "cross-root@cross-first", "--scope", "project", reject=True)
    require(a.listed() == before, "transitive marketplace trust bypassed the root allowlist")
    a.check("dependency trust uses only the root marketplace allowlist, never transitive allowlists")
    cross_catalog(["cross-second", "cross-third"])
    a.cli("marketplace", "update", "cross-first")
    a.cli("install", "cross-root@cross-first", "--scope", "project")
    cross = [p for p in a.listed() if p["name"].startswith("cross-")]
    require({p["name"] for p in cross} == {"cross-root", "cross-dep", "cross-third-leaf"},
            "authorized cross-market dependency closure incomplete")
    require(all(p["scope"] == "project" and p["enabled"] for p in cross),
            "cross-market dependencies did not preserve scope/activation")
    a.check("explicit root allowlist enables the complete cross-market dependency closure")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--execute", action="store_true")
    args = parser.parse_args()
    if not args.execute:
        print("Prepared only; pass --execute with a coordinator-approved built binary.")
        return
    require(args.binary.is_absolute() and args.binary.is_file() and not args.binary.is_symlink(),
            "explicit built binary required, not an installed launcher")
    a = Acceptance(Fixture(args.binary, seconds=180))
    try:
        git_sources(a)
        dependency_graphs(a)
        a.report["status"] = "passed"
    except BaseException as error:
        a.report.update(status="failed", error=repr(error))
        raise
    finally:
        a.report["cleanup"] = a.close()
        cleanup = a.report["cleanup"]
        if cleanup["errors"] or cleanup["tester_errors"] or not cleanup["reaped"] or not cleanup["http_stopped"]:
            a.report["status"] = "failed"
        (a.f.root / "plugin-parity-report.json").write_text(json.dumps(a.report, indent=2) + "\n")
        print("Artifacts: " + str(a.f.root), flush=True)
        require(a.report["status"] == "passed", "expanded acceptance failed; inspect retained report")


if __name__ == "__main__":
    main()
