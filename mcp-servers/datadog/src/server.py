"""
Datadog MCP Server
==================
Provides: Metrics querying, Monitor management, Log search, Dashboard operations
via the Model Context Protocol (MCP) using FastMCP.

Environment variables:
  DATADOG_API_KEY    - Required. Datadog API key
  DATADOG_APP_KEY    - Required. Datadog Application key
  DATADOG_SITE       - Optional. Datadog site (default: datadoghq.com)
"""

from mcp.server import FastMCP
import httpx
import json
import os
from datetime import datetime, timezone, timedelta
from typing import Optional

mcp = FastMCP("datadog")

DATADOG_API_KEY = os.getenv("DATADOG_API_KEY", "")
DATADOG_APP_KEY = os.getenv("DATADOG_APP_KEY", "")
DATADOG_SITE = os.getenv("DATADOG_SITE", "datadoghq.com")

BASE_V1 = f"https://api.{DATADOG_SITE}/api/v1/"
BASE_V2 = f"https://api.{DATADOG_SITE}/api/v2/"


def _get_headers() -> dict:
    return {
        "DD-API-KEY": DATADOG_API_KEY,
        "DD-APPLICATION-KEY": DATADOG_APP_KEY,
        "Content-Type": "application/json",
    }


def _validate_credentials() -> None:
    if not DATADOG_API_KEY or not DATADOG_APP_KEY:
        raise ValueError(
            "Datadog credentials not configured. "
            "Set DATADOG_API_KEY and DATADOG_APP_KEY environment variables."
        )


async def _request(method: str, url: str, json_body: Optional[dict] = None, params: Optional[dict] = None) -> str:
    _validate_credentials()
    headers = _get_headers()
    async with httpx.AsyncClient(timeout=30.0) as client:
        try:
            resp = await client.request(method, url, headers=headers, json=json_body, params=params)
        except httpx.RequestError as exc:
            return f"Request failed for {url}: {exc}"
        try:
            data = resp.json()
        except Exception:
            data = resp.text
        if resp.is_error:
            detail = ""
            if isinstance(data, dict):
                detail = data.get("errors", data.get("error", json.dumps(data)))
            elif isinstance(data, list):
                detail = "; ".join(str(e) for e in data)
            else:
                detail = str(data)
            return f"Datadog API error [{resp.status_code}] on {url}: {detail}"
        return json.dumps(data, indent=2, ensure_ascii=False, default=str)


# ---------------------------------------------------------------------------
# 1. get_metrics - Datadog API v2 timeseries
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_metrics(query: str, from_hours: int = 1) -> str:
    """Query Datadog metrics using API v2 timeseries endpoint.

    Args:
        query: Metric query string, e.g. "avg:system.cpu.user{*}by{host}"
        from_hours: Look-back window in hours (default 1)
    """
    now_ts = int(datetime.now(timezone.utc).timestamp())
    from_ts = now_ts - (from_hours * 3600)
    body = {
        "data": {
            "attributes": {
                "formulas": [{"formula": "query1"}],
                "from": from_ts,
                "to": now_ts,
                "queries": [{"name": "query1", "data_source": "metrics", "query": query}],
            }
        }
    }
    return await _request("POST", f"{BASE_V2}query/timeseries", json_body=body)


# ---------------------------------------------------------------------------
# 2. list_monitors
# ---------------------------------------------------------------------------
@mcp.tool()
async def list_monitors(tags: str = "", monitor_tags: str = "") -> str:
    """List Datadog monitors with optional filtering by tags."""
    params = {}
    if tags:
        params["tags"] = tags
    if monitor_tags:
        params["monitor_tags"] = monitor_tags
    return await _request("GET", f"{BASE_V1}monitor", params=params)


# ---------------------------------------------------------------------------
# 3. get_monitor
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_monitor(monitor_id: int) -> str:
    """Get detailed information about a specific Datadog monitor."""
    return await _request("GET", f"{BASE_V1}monitor/{monitor_id}")


# ---------------------------------------------------------------------------
# 4. mute_monitor
# ---------------------------------------------------------------------------
@mcp.tool()
async def mute_monitor(monitor_id: int, scope: str = "") -> str:
    """Mute a Datadog monitor, optionally scoping to specific hosts/tags."""
    body: dict = {}
    if scope:
        body["scope"] = [s.strip() for s in scope.split(",") if s.strip()]
    return await _request("POST", f"{BASE_V1}monitor/{monitor_id}/mute", json_body=body)


# ---------------------------------------------------------------------------
# 5. unmute_monitor
# ---------------------------------------------------------------------------
@mcp.tool()
async def unmute_monitor(monitor_id: int) -> str:
    """Unmute a previously muted Datadog monitor."""
    return await _request("POST", f"{BASE_V1}monitor/{monitor_id}/unmute")


# ---------------------------------------------------------------------------
# 6. search_logs
# ---------------------------------------------------------------------------
@mcp.tool()
async def search_logs(query: str, limit: int = 50, from_hours: int = 1) -> str:
    """Search Datadog logs with a query string.

    Args:
        query: Log search query (e.g. "service:myapp status:error")
        limit: Max log events to return (max 1000)
        from_hours: Look-back window in hours (default 1)
    """
    now = datetime.now(timezone.utc)
    since = now - timedelta(hours=from_hours)
    body = {
        "filter": {
            "query": query,
            "from": since.strftime("%Y-%m-%dT%H:%M:%S+00:00"),
            "to": now.strftime("%Y-%m-%dT%H:%M:%S+00:00"),
        },
        "page": {"limit": min(limit, 1000)},
        "sort": "-timestamp",
    }
    return await _request("POST", f"{BASE_V2}logs/events/search", json_body=body)


# ---------------------------------------------------------------------------
# 7. list_dashboards
# ---------------------------------------------------------------------------
@mcp.tool()
async def list_dashboards(filter_shared: bool = False) -> str:
    """List all Datadog dashboards."""
    params = {"filter[shared]": "true" if filter_shared else "false"}
    return await _request("GET", f"{BASE_V1}dashboard", params=params)


# ---------------------------------------------------------------------------
# 8. get_dashboard
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_dashboard(dashboard_id: str) -> str:
    """Get detailed information about a specific Datadog dashboard."""
    return await _request("GET", f"{BASE_V1}dashboard/{dashboard_id}")


if __name__ == "__main__":
    mcp.run()
