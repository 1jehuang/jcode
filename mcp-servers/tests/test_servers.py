"""Tests for MCP server infrastructure — test FastMCP initialization and tool listing."""

import json
import sys
import os
from unittest.mock import AsyncMock, patch

MCP_DIR = os.path.join(os.path.dirname(__file__), "..", "..", "mcp-servers")
sys.path.insert(0, os.path.join(MCP_DIR, "github", "src"))


def test_github_mcp_imports():
    """GitHub MCP: Verify all tool functions exist."""
    from server import mcp
    tool_names = [t.name for t in mcp._tool_manager._tools]
    required = ["list_pull_requests", "get_pull_request", "review_pull_request",
                "list_issues", "create_issue", "get_file_content"]
    for name in required:
        assert name in tool_names, f"Missing tool: {name}"
    assert len(tool_names) >= 6


def test_postgres_mcp_imports():
    """PostgreSQL MCP: Verify tool functions exist."""
    sys.path.insert(0, os.path.join(MCP_DIR, "postgres", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["execute_query", "list_tables", "describe_table",
                    "explain_query", "get_database_info"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
        assert len(tool_names) >= 5
    finally:
        sys.path.pop(0)


def test_redis_mcp_imports():
    """Redis MCP: Verify tool functions exist."""
    sys.path.insert(0, os.path.join(MCP_DIR, "redis", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["get_key", "set_key", "delete_key", "list_keys",
                    "get_ttl", "flush_db", "ping"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
        assert len(tool_names) >= 7
    finally:
        sys.path.pop(0)


def test_docker_mcp_imports():
    """Docker MCP: Verify tool functions exist."""
    sys.path.insert(0, os.path.join(MCP_DIR, "docker", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["list_containers", "start_container", "stop_container",
                    "get_container_logs", "list_images", "get_system_info"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
        assert len(tool_names) >= 8
    finally:
        sys.path.pop(0)


def test_kubernetes_mcp_imports():
    """Kubernetes MCP: Verify tool functions exist (doesn't require actual cluster)."""
    sys.path.insert(0, os.path.join(MCP_DIR, "kubernetes", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["list_pods", "get_pod_logs", "restart_deployment",
                    "get_deployments", "get_services", "get_namespaces",
                    "get_nodes", "get_events", "scale_deployment"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
        assert len(tool_names) >= 9
    finally:
        sys.path.pop(0)


def test_aws_mcp_imports():
    """AWS MCP: Verify tool functions exist (doesn't require actual AWS credentials)."""
    sys.path.insert(0, os.path.join(MCP_DIR, "aws", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["list_ec2_instances", "describe_ec2_instance", "list_s3_buckets",
                    "list_s3_objects", "list_lambda_functions", "get_cloudwatch_metrics"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
        assert len(tool_names) >= 6
    finally:
        sys.path.pop(0)


def test_sentry_mcp_imports():
    """Sentry MCP: Verify tool functions exist."""
    sys.path.insert(0, os.path.join(MCP_DIR, "sentry", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["list_projects", "list_issues", "get_issue_details",
                    "resolve_issue", "get_releases", "get_events"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
    finally:
        sys.path.pop(0)


def test_datadog_mcp_imports():
    """Datadog MCP: Verify tool functions exist."""
    sys.path.insert(0, os.path.join(MCP_DIR, "datadog", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["get_metrics", "list_monitors", "search_logs",
                    "list_dashboards", "mute_monitor", "unmute_monitor"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
    finally:
        sys.path.pop(0)


def test_slack_mcp_imports():
    """Slack MCP: Verify tool functions exist."""
    sys.path.insert(0, os.path.join(MCP_DIR, "slack", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["send_message", "list_channels", "get_channel_history",
                    "upload_file", "create_thread"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
    finally:
        sys.path.pop(0)


def test_jira_mcp_imports():
    """Jira MCP: Verify tool functions exist."""
    sys.path.insert(0, os.path.join(MCP_DIR, "jira", "src"))
    try:
        from server import mcp
        tool_names = [t.name for t in mcp._tool_manager._tools]
        required = ["search_issues", "get_issue", "create_issue",
                    "update_issue", "add_comment", "transition_issue"]
        for name in required:
            assert name in tool_names, f"Missing tool: {name}"
    finally:
        sys.path.pop(0)
