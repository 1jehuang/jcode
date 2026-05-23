#!/usr/bin/env python3
"""
CarpAI MCP Server Launcher
Starts all configured MCP servers as subprocesses.
Usage: python start_all.py [server1 server2 ...]
"""

import subprocess
import sys
import os
import time
import signal

MCP_DIR = os.path.dirname(os.path.abspath(__file__))

SERVERS = {
    "github": {
        "path": "github/src/server.py",
        "port": 8001,
        "env": {"GITHUB_TOKEN": os.getenv("GITHUB_TOKEN", "")},
    },
    "jira": {
        "path": "jira/src/server.py",
        "port": 8002,
        "env": {
            "JIRA_URL": os.getenv("JIRA_URL", ""),
            "JIRA_EMAIL": os.getenv("JIRA_EMAIL", ""),
            "JIRA_API_TOKEN": os.getenv("JIRA_API_TOKEN", ""),
        },
    },
    "slack": {
        "path": "slack/src/server.py",
        "port": 8003,
        "env": {"SLACK_BOT_TOKEN": os.getenv("SLACK_BOT_TOKEN", "")},
    },
    "docker": {
        "path": "docker/src/server.py",
        "port": 8004,
        "env": {},
    },
    "postgres": {
        "path": "postgres/src/server.py",
        "port": 8005,
        "env": {"DATABASE_URL": os.getenv("DATABASE_URL", "")},
    },
    "redis": {
        "path": "redis/src/server.py",
        "port": 8006,
        "env": {"REDIS_URL": os.getenv("REDIS_URL", "")},
    },
    "kubernetes": {
        "path": "kubernetes/src/server.py",
        "port": 8007,
        "env": {"KUBECONFIG": os.getenv("KUBECONFIG", "")},
    },
    "aws": {
        "path": "aws/src/server.py",
        "port": 8008,
        "env": {
            "AWS_ACCESS_KEY_ID": os.getenv("AWS_ACCESS_KEY_ID", ""),
            "AWS_SECRET_ACCESS_KEY": os.getenv("AWS_SECRET_ACCESS_KEY", ""),
            "AWS_REGION": os.getenv("AWS_REGION", "us-east-1"),
        },
    },
    "sentry": {
        "path": "sentry/src/server.py",
        "port": 8009,
        "env": {
            "SENTRY_TOKEN": os.getenv("SENTRY_TOKEN", ""),
            "SENTRY_ORG_SLUG": os.getenv("SENTRY_ORG_SLUG", ""),
        },
    },
    "datadog": {
        "path": "datadog/src/server.py",
        "port": 8010,
        "env": {
            "DATADOG_API_KEY": os.getenv("DATADOG_API_KEY", ""),
            "DATADOG_APP_KEY": os.getenv("DATADOG_APP_KEY", ""),
        },
    },
}

processes = []


def start_server(name: str, config: dict) -> bool:
    server_path = os.path.join(MCP_DIR, config["path"])
    if not os.path.exists(server_path):
        print(f"  [SKIP] {name}: {server_path} not found")
        return False

    env = os.environ.copy()
    for key, value in config["env"].items():
        if value:
            env[key] = value

    print(f"  [START] {name} (port {config.get('port', 'auto')}) ...", end=" ", flush=True)
    try:
        proc = subprocess.Popen(
            [sys.executable, server_path],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        processes.append(proc)
        print(f"PID {proc.pid}")
        return True
    except Exception as e:
        print(f"FAILED: {e}")
        return False


def signal_handler(sig, frame):
    print("\nShutting down MCP servers...")
    for proc in processes:
        proc.terminate()
    for proc in processes:
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    print("All servers stopped.")
    sys.exit(0)


def main():
    signal.signal(signal.SIGINT, signal_handler)
    signal.signal(signal.SIGTERM, signal_handler)

    print("CarpAI MCP Server Launcher")
    print(f"MCP Directory: {MCP_DIR}")
    print()

    active_servers = SERVERS
    if len(sys.argv) > 1:
        requested = set(sys.argv[1:])
        active_servers = {k: v for k, v in SERVERS.items() if k in requested}
        if not active_servers:
            print(f"No matching servers. Available: {', '.join(SERVERS.keys())}")
            sys.exit(1)

    success = 0
    for name, config in active_servers.items():
        if start_server(name, config):
            success += 1

    print(f"\nStarted {success}/{len(active_servers)} servers")
    print("Press Ctrl+C to stop all servers")

    try:
        while processes:
            for proc in processes[:]:
                if proc.poll() is not None:
                    processes.remove(proc)
            time.sleep(1)
    except KeyboardInterrupt:
        signal_handler(None, None)


if __name__ == "__main__":
    main()
