"""
Sentry MCP Server
=================
Provides: Issue management, Project insights, Release tracking, Event inspection
via the Model Context Protocol (MCP).

Environment variables:
  SENTRY_TOKEN       - Required. Sentry Auth Token
  SENTRY_ORG_SLUG    - Optional. Default organization slug
  SENTRY_BASE_URL    - Optional. Defaults to https://sentry.io/api/0/
"""

from mcp.server import FastMCP
import httpx
import os
from typing import Optional

mcp = FastMCP("sentry")

SENTRY_TOKEN = os.getenv("SENTRY_TOKEN", "")
SENTRY_ORG_SLUG = os.getenv("SENTRY_ORG_SLUG", "")
SENTRY_BASE_URL = os.getenv("SENTRY_BASE_URL", "https://sentry.io/api/0/")

if not SENTRY_TOKEN:
    print("Warning: SENTRY_TOKEN environment variable is not set.")


def _get_headers() -> dict:
    token = SENTRY_TOKEN.strip()
    if not token:
        return {"Content-Type": "application/json"}
    if token.startswith("Bearer ") or token.startswith("DSN "):
        return {"Authorization": token, "Content-Type": "application/json"}
    return {"Authorization": f"Bearer {token}", "Content-Type": "application/json"}


async def _api_get(path: str, params: Optional[dict] = None):
    url = SENTRY_BASE_URL.rstrip("/") + "/" + path.lstrip("/")
    headers = _get_headers()
    async with httpx.AsyncClient(timeout=30.0) as client:
        resp = await client.get(url, headers=headers, params=params)
        _raise_for_status(resp, path)
        return resp.json()


async def _api_post(path: str, json_body: Optional[dict] = None):
    url = SENTRY_BASE_URL.rstrip("/") + "/" + path.lstrip("/")
    headers = _get_headers()
    async with httpx.AsyncClient(timeout=30.0) as client:
        resp = await client.post(url, headers=headers, json=json_body)
        _raise_for_status(resp, path)
        return resp.json()


async def _api_put(path: str, json_body: Optional[dict] = None):
    url = SENTRY_BASE_URL.rstrip("/") + "/" + path.lstrip("/")
    headers = _get_headers()
    async with httpx.AsyncClient(timeout=30.0) as client:
        resp = await client.put(url, headers=headers, json=json_body)
        _raise_for_status(resp, path)
        return resp.json()


def _raise_for_status(resp: httpx.Response, path: str):
    if resp.status_code == 401:
        raise PermissionError("Authentication failed (401). Check SENTRY_TOKEN.")
    if resp.status_code == 403:
        raise PermissionError("Access denied (403). Token may lack required scopes.")
    if resp.status_code == 404:
        raise FileNotFoundError(f"Resource not found (404) at {path}.")
    if resp.status_code == 429:
        raise RuntimeError("Rate limited (429). Wait before retrying.")
    resp.raise_for_status()


async def _get_default_org() -> str:
    data = await _api_get("organizations/")
    if isinstance(data, list) and data:
        return data[0]["slug"]
    if SENTRY_ORG_SLUG:
        return SENTRY_ORG_SLUG
    raise RuntimeError("Could not determine org slug. Set SENTRY_ORG_SLUG env var.")


# ---------------------------------------------------------------------------
# 1. list_projects
# ---------------------------------------------------------------------------
@mcp.tool()
async def list_projects() -> str:
    """List all Sentry projects accessible by the auth token."""
    try:
        data = await _api_get("projects/")
        if not data:
            return "No projects found."
        lines = ["Sentry Projects:"]
        for proj in data:
            team = ", ".join(t["slug"] for t in proj.get("teams", []))
            lines.append(
                f"  - {proj['slug']} | Platform: {proj.get('platform', 'N/A')} "
                f"| Team: {team} | ID: {proj['id']}"
            )
        return "\n".join(lines)
    except Exception as e:
        return f"Error listing projects: {e}"


# ---------------------------------------------------------------------------
# 2. list_issues
# ---------------------------------------------------------------------------
@mcp.tool()
async def list_issues(project_slug: str, query: str = "", limit: int = 20) -> str:
    """List recent issues for a Sentry project."""
    try:
        org = SENTRY_ORG_SLUG or await _get_default_org()
        params = {"limit": min(limit, 100)}
        if query:
            params["query"] = query
        path = f"projects/{org}/{project_slug}/issues/"
        data = await _api_get(path, params=params)
        if not data:
            return f"No issues found for project '{project_slug}'."
        lines = [f"Issues for project '{project_slug}':"]
        for issue in data:
            lines.append(
                f"  - [{issue.get('level', 'N/A')}] #{issue['id']}: {issue['title']} "
                f"({issue.get('status', 'N/A')}) | Count: {issue.get('count', 0)} "
                f"| Users: {issue.get('userCount', 0)}"
            )
        return "\n".join(lines)
    except Exception as e:
        return f"Error listing issues: {e}"


# ---------------------------------------------------------------------------
# 3. get_issue_details
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_issue_details(issue_id: str) -> str:
    """Get detailed information about a specific Sentry issue."""
    try:
        data = await _api_get(f"issues/{issue_id}/")
        lines = [
            f"Issue #{data['id']}: {data['title']}",
            f"Level: {data.get('level', 'N/A')}",
            f"Status: {data.get('status', 'N/A')}",
            f"Count: {data.get('count', 0)} | Users: {data.get('userCount', 0)}",
            f"First Seen: {data.get('firstSeen', 'N/A')}",
            f"Last Seen: {data.get('lastSeen', 'N/A')}",
            f"Project: {data.get('project', {}).get('slug', 'N/A')}",
            f"Permalink: {data.get('permalink', 'N/A')}",
        ]
        tags = data.get("tags", [])
        if tags:
            lines.append("\nTags:")
            for tag in tags[:20]:
                lines.append(f"  {tag.get('key', '?')}: {tag.get('value', '?')}")
        return "\n".join(lines)
    except Exception as e:
        return f"Error getting issue details: {e}"


# ---------------------------------------------------------------------------
# 4. resolve_issue
# ---------------------------------------------------------------------------
@mcp.tool()
async def resolve_issue(issue_id: str) -> str:
    """Mark a Sentry issue as resolved."""
    try:
        data = await _api_put(f"issues/{issue_id}/", json_body={"status": "resolved"})
        return f"Issue #{issue_id} resolved. Status: {data.get('status', 'N/A')}"
    except Exception as e:
        return f"Error resolving issue: {e}"


# ---------------------------------------------------------------------------
# 5. ignore_issue
# ---------------------------------------------------------------------------
@mcp.tool()
async def ignore_issue(issue_id: str) -> str:
    """Ignore a Sentry issue."""
    try:
        data = await _api_put(f"issues/{issue_id}/", json_body={"status": "ignored"})
        return f"Issue #{issue_id} ignored. Status: {data.get('status', 'N/A')}"
    except Exception as e:
        return f"Error ignoring issue: {e}"


# ---------------------------------------------------------------------------
# 6. get_releases
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_releases(project_slug: str, limit: int = 10) -> str:
    """List recent releases for a Sentry project."""
    try:
        org = SENTRY_ORG_SLUG or await _get_default_org()
        params = {"limit": min(limit, 100)}
        path = f"projects/{org}/{project_slug}/releases/"
        data = await _api_get(path, params=params)
        if not data:
            return f"No releases found for project '{project_slug}'."
        lines = [f"Releases for project '{project_slug}':"]
        for release in data:
            lines.append(
                f"  - {release['version']} | Created: {release.get('dateCreated', 'N/A')} "
                f"| Commits: {len(release.get('commits', []))} "
                f"| Deploy: {len(release.get('deployCount', 0))}"
            )
        return "\n".join(lines)
    except Exception as e:
        return f"Error getting releases: {e}"


# ---------------------------------------------------------------------------
# 7. get_events
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_events(issue_id: str, limit: int = 10) -> str:
    """Get the most recent events for a Sentry issue."""
    try:
        path = f"issues/{issue_id}/events/"
        params = {"limit": min(limit, 100)}
        data = await _api_get(path, params=params)
        if not data:
            return f"No events found for issue #{issue_id}."
        lines = [f"Events for issue #{issue_id}:", ""]
        for event in data:
            event_id = event.get("eventID", "?")
            timestamp = event.get("dateCreated", "N/A")
            message = event.get("message", "No message")
            lines.append(f"  Event {event_id} @ {timestamp}")
            lines.append(f"    Message: {message}")
            # Show top stack trace frame if available
            entries = event.get("entries", [])
            for entry in entries:
                if entry.get("type") == "exception" and entry.get("data", {}).get("values"):
                    exc_val = entry["data"]["values"][0]
                    lines.append(f"    Exception: {exc_val.get('type', '')}: {exc_val.get('value', '')}")
                    frames = (exc_val.get("stacktrace", {}) or {}).get("frames", [])
                    if frames:
                        top = frames[-1]
                        lines.append(f"    At: {top.get('filename', '?')}:{top.get('lineno', '?')} in {top.get('function', '?')}")
                    break
            lines.append("")
        return "\n".join(lines)
    except Exception as e:
        return f"Error getting events: {e}"


# ---------------------------------------------------------------------------
# 8. get_project_stats
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_project_stats(project_slug: str, stat: str = "received", days: int = 7) -> str:
    """Get project statistics (received, rejected, etc.) for the last N days."""
    try:
        org = SENTRY_ORG_SLUG or await _get_default_org()
        params = {"stat": stat, "since": int(__import__("time").time() - days * 86400)}
        path = f"projects/{org}/{project_slug}/stats/"
        data = await _api_get(path, params=params)
        if not data:
            return f"No stats returned for project '{project_slug}'."
        lines = [f"Stats for '{project_slug}' (last {days} days, stat={stat}):"]
        total = 0
        for point in data[-50:]:  # Last 50 data points max
            ts = point[0]
            val = point[1]
            total += val
            dt_str = __import__("datetime").datetime.fromtimestamp(ts).strftime("%Y-%m-%d %H:%M")
            lines.append(f"  {dt_str}: {val}")
        lines.append(f"\nTotal: {total}")
        return "\n".join(lines)
    except Exception as e:
        return f"Error getting project stats: {e}"


if __name__ == "__main__":
    mcp.run()
