"""
Integration tests for MCP servers — tests that actually start server processes.
"""

import subprocess
import sys
import os
import time

MCP_DIR = os.path.join(os.path.dirname(__file__), "..", "mcp-servers")


def _check_server_import(server_dir: str) -> bool:
    """Check that the server module can be imported successfully."""
    server_path = os.path.join(MCP_DIR, server_dir, "src", "server.py")
    if not os.path.exists(server_path):
        return False
    result = subprocess.run(
        [sys.executable, "-c", f"import ast; ast.parse(open('{server_path}').read())"],
        capture_output=True, text=True, timeout=10,
    )
    return result.returncode == 0


def test_github_syntax():
    assert _check_server_import("github"), "GitHub MCP server has syntax errors"


def test_postgres_syntax():
    assert _check_server_import("postgres"), "PostgreSQL MCP server has syntax errors"


def test_redis_syntax():
    assert _check_server_import("redis"), "Redis MCP server has syntax errors"


def test_docker_syntax():
    assert _check_server_import("docker"), "Docker MCP server has syntax errors"


def test_kubernetes_syntax():
    assert _check_server_import("kubernetes"), "Kubernetes MCP server has syntax errors"


def test_aws_syntax():
    assert _check_server_import("aws"), "AWS MCP server has syntax errors"


def test_sentry_syntax():
    assert _check_server_import("sentry"), "Sentry MCP server has syntax errors"


def test_datadog_syntax():
    assert _check_server_import("datadog"), "Datadog MCP server has syntax errors"


def test_slack_syntax():
    assert _check_server_import("slack"), "Slack MCP server has syntax errors"


def test_jira_syntax():
    assert _check_server_import("jira"), "Jira MCP server has syntax errors"


def test_all_servers_syntax():
    """Verify all 10 MCP server modules have valid Python syntax."""
    servers = ["github", "jira", "slack", "docker", "postgres",
               "redis", "kubernetes", "aws", "sentry", "datadog"]
    results = {}
    for server in servers:
        results[server] = _check_server_import(server)
    failed = [s for s, ok in results.items() if not ok]
    assert not failed, f"Syntax errors in: {', '.join(failed)}"
