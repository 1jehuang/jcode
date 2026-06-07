#!/usr/bin/env python3
"""Report compile-time isolation risks in the Jcode crate graph.

This is advisory by default. Use --strict-target-state only when a migration phase
has removed the listed temporary violations and we want to prevent regressions.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
WATCHED_CRATES = ["jcode-base", "jcode-app-core", "jcode-tui", "jcode"]


@dataclass
class CrateStats:
    name: str
    manifest_path: str
    src_path: str | None
    rust_files: int
    loc: int
    cfg_test_count: int
    test_attr_count: int
    async_trait_count: int
    derive_count: int
    glob_reexports: list[str]
    normal_workspace_deps: list[str]
    normal_external_deps: list[str]
    dev_workspace_deps: list[str]
    dev_external_deps: list[str]
    build_workspace_deps: list[str]
    build_external_deps: list[str]


def run_metadata() -> dict[str, Any]:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    )
    return json.loads(result.stdout)


def lib_or_root_src(package: dict[str, Any]) -> Path | None:
    manifest = Path(package["manifest_path"])
    for target in package.get("targets", []):
        if "lib" in target.get("kind", []):
            return Path(target["src_path"])
    if package["name"] == "jcode":
        return ROOT / "src"
    src = manifest.parent / "src"
    return src if src.exists() else None


def iter_rust_files(src_path: Path | None) -> list[Path]:
    if src_path is None:
        return []
    if src_path.is_file():
        root = src_path.parent
    else:
        root = src_path
    if not root.exists():
        return []
    return sorted(path for path in root.rglob("*.rs") if path.is_file())


def count_file(path: Path) -> tuple[int, str]:
    text = path.read_text(errors="replace")
    return text.count("\n") + (0 if text.endswith("\n") or not text else 1), text


def collect_stats(package: dict[str, Any], workspace_names: set[str]) -> CrateStats:
    src_path = lib_or_root_src(package)
    rust_files = iter_rust_files(src_path)
    loc = 0
    cfg_test_count = 0
    test_attr_count = 0
    async_trait_count = 0
    derive_count = 0
    glob_reexports: list[str] = []

    for path in rust_files:
        file_loc, text = count_file(path)
        loc += file_loc
        cfg_test_count += len(re.findall(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", text))
        test_attr_count += len(re.findall(r"#\s*\[\s*(?:tokio::)?test(?:\s*\([^\]]*\))?\s*\]", text))
        async_trait_count += len(re.findall(r"#\s*\[\s*(?:async_trait::)?async_trait\s*\]", text))
        derive_count += len(re.findall(r"#\s*\[\s*derive\s*\(", text))
        for line_number, line in enumerate(text.splitlines(), 1):
            stripped = line.strip()
            if stripped.startswith("//"):
                continue
            if re.fullmatch(r"pub\s+use\s+[^;\n]+::\s*\*\s*;", stripped):
                rel = path.relative_to(ROOT)
                glob_reexports.append(f"{rel}:{line_number}: {stripped}")

    workspace_deps_by_kind: dict[str, list[str]] = {"normal": [], "dev": [], "build": []}
    external_deps_by_kind: dict[str, list[str]] = {"normal": [], "dev": [], "build": []}
    for dep in package.get("dependencies", []):
        dep_name = dep["name"]
        dep_kind = dep.get("kind") or "normal"
        if dep_kind not in workspace_deps_by_kind:
            dep_kind = "normal"
        buckets = workspace_deps_by_kind if dep_name in workspace_names else external_deps_by_kind
        buckets[dep_kind].append(dep_name)

    return CrateStats(
        name=package["name"],
        manifest_path=str(Path(package["manifest_path"]).relative_to(ROOT)),
        src_path=str(src_path.relative_to(ROOT)) if src_path and src_path.exists() else None,
        rust_files=len(rust_files),
        loc=loc,
        cfg_test_count=cfg_test_count,
        test_attr_count=test_attr_count,
        async_trait_count=async_trait_count,
        derive_count=derive_count,
        glob_reexports=glob_reexports,
        normal_workspace_deps=sorted(workspace_deps_by_kind["normal"]),
        normal_external_deps=sorted(external_deps_by_kind["normal"]),
        dev_workspace_deps=sorted(workspace_deps_by_kind["dev"]),
        dev_external_deps=sorted(external_deps_by_kind["dev"]),
        build_workspace_deps=sorted(workspace_deps_by_kind["build"]),
        build_external_deps=sorted(external_deps_by_kind["build"]),
    )


def target_state_violations(stats_by_name: dict[str, CrateStats]) -> list[str]:
    violations: list[str] = []

    tui = stats_by_name.get("jcode-tui")
    if tui and "jcode-app-core" in tui.normal_workspace_deps:
        violations.append("target-state: jcode-tui still directly depends on jcode-app-core")

    app_core = stats_by_name.get("jcode-app-core")
    if app_core and "jcode-base" in app_core.normal_workspace_deps:
        violations.append("target-state: jcode-app-core still directly depends on jcode-base")

    base = stats_by_name.get("jcode-base")
    if base:
        for dep in base.normal_workspace_deps:
            if dep in {
                "jcode-azure-auth",
                "jcode-provider-gemini",
                "jcode-provider-openai",
                "jcode-provider-openrouter",
                "jcode-notify-email",
                "jcode-build-support",
            }:
                violations.append(f"target-state: jcode-base still depends on leaf/runtime crate {dep}")
        for dep in base.normal_external_deps:
            if dep.startswith("aws-") or dep in {"aws-types"}:
                violations.append(f"target-state: jcode-base still depends directly on AWS crate {dep}")

    for crate in stats_by_name.values():
        for glob in crate.glob_reexports:
            if crate.name in {"jcode", "jcode-app-core", "jcode-tui"}:
                violations.append(f"target-state: broad glob re-export remains in {glob}")

    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print machine-readable JSON")
    parser.add_argument(
        "--strict-target-state",
        action="store_true",
        help="exit non-zero for target-state violations (advisory by default)",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=12,
        help="number of largest workspace crates to print in text mode",
    )
    args = parser.parse_args()

    metadata = run_metadata()
    package_by_id = {package["id"]: package for package in metadata["packages"]}
    workspace_packages = [package_by_id[package_id] for package_id in metadata["workspace_members"]]
    workspace_names = {package["name"] for package in workspace_packages}

    stats = [collect_stats(package, workspace_names) for package in workspace_packages]
    stats_by_name = {crate.name: crate for crate in stats}
    violations = target_state_violations(stats_by_name)
    largest = sorted(stats, key=lambda crate: crate.loc, reverse=True)

    payload = {
        "watched_crates": {name: asdict(stats_by_name[name]) for name in WATCHED_CRATES if name in stats_by_name},
        "largest_crates": [asdict(crate) for crate in largest[: args.top]],
        "target_state_violations": violations,
    }

    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print("compile isolation static report")
        print("largest workspace crates by Rust LOC:")
        for crate in largest[: args.top]:
            print(
                f"  - {crate.name}: {crate.loc} LOC, {crate.rust_files} files, "
                f"#[test] {crate.test_attr_count}, cfg(test) {crate.cfg_test_count}, "
                f"async_trait {crate.async_trait_count}, derive {crate.derive_count}"
            )
        print("watched crates:")
        for name in WATCHED_CRATES:
            crate = stats_by_name.get(name)
            if not crate:
                continue
            print(
                f"  - {name}: {crate.loc} LOC, "
                f"normal workspace deps={len(crate.normal_workspace_deps)}, "
                f"normal external deps={len(crate.normal_external_deps)}"
            )
            if crate.dev_workspace_deps or crate.dev_external_deps or crate.build_workspace_deps or crate.build_external_deps:
                print(
                    f"    non-normal deps: dev workspace={len(crate.dev_workspace_deps)}, "
                    f"dev external={len(crate.dev_external_deps)}, "
                    f"build workspace={len(crate.build_workspace_deps)}, "
                    f"build external={len(crate.build_external_deps)}"
                )
            if crate.glob_reexports:
                print("    glob re-exports:")
                for glob in crate.glob_reexports[:8]:
                    print(f"      {glob}")
                if len(crate.glob_reexports) > 8:
                    print(f"      ... {len(crate.glob_reexports) - 8} more")
        if violations:
            print("target-state violations/advisories:")
            for violation in violations:
                print(f"  - {violation}")
        else:
            print("target-state violations/advisories: none")

    if args.strict_target_state and violations:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
