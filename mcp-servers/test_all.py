#!/usr/bin/env python3
"""
CarpAI MCP Server Verification Script
Tests that all MCP server modules can be imported without errors.
"""

import importlib
import sys
import os
import traceback

MCP_DIR = os.path.dirname(os.path.abspath(__file__))

SERVERS = {
    "github": "github/src/server",
    "jira": "jira/src/server",
    "slack": "slack/src/server",
    "docker": "docker/src/server",
    "postgres": "postgres/src/server",
    "redis": "redis/src/server",
    "kubernetes": "kubernetes/src/server",
    "aws": "aws/src/server",
    "sentry": "sentry/src/server",
    "datadog": "datadog/src/server",
}


def test_server(name: str, module_path: str) -> tuple[bool, str]:
    """Test that a server module can be imported."""
    full_path = os.path.join(MCP_DIR, module_path.replace(".", "/") + ".py")
    if not os.path.exists(full_path):
        return False, f"File not found: {full_path}"

    sys.path.insert(0, os.path.dirname(full_path))
    module_name = os.path.basename(full_path).replace(".py", "")

    try:
        importlib.import_module(module_name)
        # Check for mcp instance
        mod = sys.modules[module_name]
        if hasattr(mod, "mcp"):
            tools = mod.mcp._tool_manager._tools if hasattr(mod.mcp, "_tool_manager") else []
            return True, f"OK ({len(tools)} tools registered)"
        else:
            return True, "OK (no mcp instance)"
    except ImportError as e:
        missing_pkg = str(e).split("'")[1] if "'" in str(e) else str(e)
        return False, f"Missing dependency: {missing_pkg}"
    except Exception as e:
        return False, f"Error: {e}\n{traceback.format_exc()[:200]}"
    finally:
        sys.path.pop(0)
        if module_name in sys.modules:
            del sys.modules[module_name]


def main():
    print("=" * 60)
    print("  CarpAI MCP Server Verification")
    print("=" * 60)
    print()

    passed = 0
    failed = 0

    for name, module_path in SERVERS.items():
        ok, msg = test_server(name, module_path)
        status = "PASS" if ok else "FAIL"
        color = "\033[92m" if ok else "\033[91m"
        reset = "\033[0m"
        print(f"  [{color}{status}{reset}] {name:12s} - {msg}")

        if ok:
            passed += 1
        else:
            failed += 1

    print()
    print("-" * 60)
    print(f"  Total: {passed + failed}  |  Passed: {passed}  |  Failed: {failed}")
    print("-" * 60)
    print()
    print("Note: Some failures may be due to missing dependencies.")
    print("Run 'pip install -r requirements.txt' and server-specific requirements.")
    print()

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
