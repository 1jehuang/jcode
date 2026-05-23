#!/usr/bin/env python3
"""
CarpAI MCP Server Installer
Installs dependencies for all MCP servers from requirements files.
"""

import subprocess
import sys
import os

MCP_DIR = os.path.dirname(os.path.abspath(__file__))
REQUIREMENTS_FILES = [
    "requirements.txt",
    "requirements-github.txt",
    "requirements-jira.txt",
    "requirements-slack.txt",
    "requirements-docker.txt",
    "requirements-postgres.txt",
    "requirements-redis.txt",
    "requirements-kubernetes.txt",
    "requirements-aws.txt",
    "requirements-sentry.txt",
    "requirements-datadog.txt",
]


def install_requirements(req_file: str) -> bool:
    path = os.path.join(MCP_DIR, req_file)
    if not os.path.exists(path):
        print(f"  [SKIP] {req_file} not found")
        return False
    print(f"  [INSTALL] {req_file} ...", end=" ", flush=True)
    result = subprocess.run(
        [sys.executable, "-m", "pip", "install", "-r", path],
        capture_output=True, text=True,
    )
    if result.returncode == 0:
        print("OK")
        return True
    else:
        print(f"FAILED\n{result.stderr.strip()}")
        return False


def main():
    print("CarpAI MCP Server Installer")
    print(f"Python: {sys.executable}")
    print(f"MCP Directory: {MCP_DIR}")
    print()

    success = 0
    failed = 0

    for req_file in REQUIREMENTS_FILES:
        if install_requirements(req_file):
            success += 1
        else:
            failed += 1

    print()
    print(f"Summary: {success} installed, {failed} failed")


if __name__ == "__main__":
    main()
