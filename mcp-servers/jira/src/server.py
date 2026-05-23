# MCP Server for Jira Integration
# Provides: Issue tracking, Project management, Sprint operations
# Version: 2.0 (85% feature complete)

from mcp.server import FastMCP
import httpx
import os
import logging
from typing import Optional

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

mcp = FastMCP("jira")

JIRA_URL = os.getenv("JIRA_URL", "https://your-domain.atlassian.net")
JIRA_EMAIL = os.getenv("JIRA_EMAIL", "")
JIRA_TOKEN = os.getenv("JIRA_API_TOKEN", "")

if not JIRA_TOKEN:
    logger.warning("JIRA_API_TOKEN not set. Operations may fail.")

async def get_headers():
    import base64
    auth = base64.b64encode(f"{JIRA_EMAIL}:{JIRA_TOKEN}".encode()).decode()
    return {
        "Authorization": f"Basic {auth}",
        "Content-Type": "application/json"
    }

def handle_jira_error(resp: httpx.Response, operation: str) -> str:
    """Handle Jira API errors"""
    if resp.status_code == 404:
        return f"Error: Resource not found (404)"
    elif resp.status_code == 403:
        return f"Error: Permission denied (403)"
    elif resp.status_code >= 400:
        error_data = resp.json() if resp.content else {}
        message = error_data.get('errorMessages', ['Unknown error'])[0] if 'errorMessages' in error_data else str(error_data)
        return f"Error {resp.status_code}: {message}"
    return None

@mcp.tool()
async def search_issues(jql: str, max_results: int = 50) -> str:
    """Search for issues using JQL"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{JIRA_URL}/rest/api/3/search",
            headers=headers,
            params={"jql": jql, "maxResults": max_results}
        )
        data = resp.json()
        issues = data.get("issues", [])
        result = []
        for issue in issues:
            fields = issue["fields"]
            result.append(f"{issue['key']}: {fields['summary']} ({fields['status']['name']})")
        return "\n".join(result) if result else "No issues found"

@mcp.tool()
async def get_issue(issue_key: str) -> str:
    """Get details of a specific issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{JIRA_URL}/rest/api/3/issue/{issue_key}",
            headers=headers
        )
        issue = resp.json()
        fields = issue["fields"]
        return f"""{issue['key']}: {fields['summary']}
Status: {fields['status']['name']}
Priority: {fields['priority']['name']}
Assignee: {fields.get('assignee', {}).get('displayName', 'Unassigned')}
Reporter: {fields['reporter']['displayName']}
Created: {fields['created']}
Description: {fields.get('description', 'No description')}"""

@mcp.tool()
async def create_issue(project: str, summary: str, issue_type: str = "Task", description: str = "", assignee: str = "") -> str:
    """Create a new Jira issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        data = {
            "fields": {
                "project": {"key": project},
                "summary": summary,
                "issuetype": {"name": issue_type},
                "description": description
            }
        }
        if assignee:
            data["fields"]["assignee"] = {"accountId": assignee}
        resp = await client.post(
            f"{JIRA_URL}/rest/api/3/issue",
            headers=headers,
            json=data
        )
        result = resp.json()
        return f"Issue created: {result['key']}"

@mcp.tool()
async def update_issue(issue_key: str, summary: str = "", description: str = "", status: str = "") -> str:
    """Update an existing issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        data = {"fields": {}}
        if summary:
            data["fields"]["summary"] = summary
        if description:
            data["fields"]["description"] = description
        if status:
            transitions_resp = await client.get(
                f"{JIRA_URL}/rest/api/3/issue/{issue_key}/transitions",
                headers=headers
            )
            transitions = transitions_resp.json()["transitions"]
            transition_id = next((t["id"] for t in transitions if t["name"].lower() == status.lower()), None)
            if transition_id:
                await client.post(
                    f"{JIRA_URL}/rest/api/3/issue/{issue_key}/transitions",
                    headers=headers,
                    json={"transition": {"id": transition_id}}
                )
        if data["fields"]:
            await client.put(
                f"{JIRA_URL}/rest/api/3/issue/{issue_key}",
                headers=headers,
                json=data
            )
        return f"Issue {issue_key} updated"

@mcp.tool()
async def add_comment(issue_key: str, comment: str) -> str:
    """Add a comment to an issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{JIRA_URL}/rest/api/3/issue/{issue_key}/comment",
            headers=headers,
            json={"body": comment}
        )
        return f"Comment added to {issue_key}"

@mcp.tool()
async def get_project_issues(project: str, status: str = "Open") -> str:
    """Get all issues for a project"""
    jql = f"project = {project} AND status = \"{status}\" ORDER BY created DESC"
    return await search_issues(jql)

@mcp.tool()
async def delete_issue(issue_key: str, confirm: bool = False) -> str:
    """Delete an issue (requires admin permissions). Requires confirm=True."""
    if not confirm:
        return f"WARNING: This will permanently delete {issue_key}. Set confirm=true to proceed."
    
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.delete(
            f"{JIRA_URL}/rest/api/3/issue/{issue_key}",
            headers=headers
        )
        
        error = handle_jira_error(resp, "delete_issue")
        if error:
            return error
        
        return f"Issue {issue_key} deleted"

@mcp.tool()
async def get_transitions(issue_key: str) -> str:
    """Get available status transitions for an issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{JIRA_URL}/rest/api/3/issue/{issue_key}/transitions",
            headers=headers
        )
        
        error = handle_jira_error(resp, "get_transitions")
        if error:
            return error
        
        data = resp.json()
        transitions = data.get('transitions', [])
        
        if not transitions:
            return "No transitions available"
        
        result = [f"Available transitions for {issue_key}:"]
        for t in transitions:
            result.append(f"  - {t['id']}: {t['name']}")
        
        return "\n".join(result)

@mcp.tool()
async def link_issues(inward_key: str, outward_key: str, link_type: str = "Blocks") -> str:
    """Create a link between two issues
    
    Args:
        inward_key: Source issue key (e.g., 'PROJ-123')
        outward_key: Target issue key (e.g., 'PROJ-456')
        link_type: Link type (Blocks, Relates to, Duplicates, etc.)
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{JIRA_URL}/rest/api/3/issueLink",
            headers=headers,
            json={
                "type": {"name": link_type},
                "inwardIssue": {"key": inward_key},
                "outwardIssue": {"key": outward_key}
            }
        )
        
        error = handle_jira_error(resp, "link_issues")
        if error:
            return error
        
        return f"Linked {inward_key} {link_type.lower()} {outward_key}"

@mcp.tool()
async def get_worklogs(issue_key: str) -> str:
    """Get worklog entries for an issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{JIRA_URL}/rest/api/3/issue/{issue_key}/worklog",
            headers=headers
        )
        
        error = handle_jira_error(resp, "get_worklogs")
        if error:
            return error
        
        data = resp.json()
        worklogs = data.get('worklogs', [])
        
        if not worklogs:
            return f"No worklogs for {issue_key}"
        
        result = [f"Worklogs for {issue_key}:"]
        for w in worklogs:
            author = w.get('author', {}).get('displayName', 'Unknown')
            time_spent = w.get('timeSpent', 'N/A')
            comment = w.get('comment', 'No comment')
            started = w.get('started', 'N/A')[:10]
            result.append(f"  - {author}: {time_spent} on {started} - {comment}")
        
        return "\n".join(result)

@mcp.tool()
async def add_worklog(issue_key: str, time_spent: str, comment: str = "") -> str:
    """Add a worklog entry to an issue
    
    Args:
        issue_key: Issue key (e.g., 'PROJ-123')
        time_spent: Time spent (e.g., '2h 30m', '1d')
        comment: Optional comment
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        data = {"timeSpent": time_spent}
        if comment:
            data["comment"] = comment
        
        resp = await client.post(
            f"{JIRA_URL}/rest/api/3/issue/{issue_key}/worklog",
            headers=headers,
            json=data
        )
        
        error = handle_jira_error(resp, "add_worklog")
        if error:
            return error
        
        return f"Worklog added to {issue_key}: {time_spent}"

@mcp.tool()
async def get_sprints(board_id: int, state: str = "active") -> str:
    """Get sprints for a board
    
    Args:
        board_id: Board ID
        state: Sprint state (active, future, closed)
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{JIRA_URL}/rest/agile/1.0/board/{board_id}/sprint",
            headers=headers,
            params={"state": state}
        )
        
        error = handle_jira_error(resp, "get_sprints")
        if error:
            return error
        
        data = resp.json()
        sprints = data.get('sprints', [])
        
        if not sprints:
            return f"No {state} sprints found for board {board_id}"
        
        result = [f"{state.capitalize()} sprints for board {board_id}:"]
        for s in sprints:
            result.append(f"  - {s['id']}: {s['name']} ({s['state']})")
            if 'startDate' in s:
                result.append(f"    {s['startDate'][:10]} to {s['endDate'][:10]}")
        
        return "\n".join(result)

if __name__ == "__main__":
    logger.info("Starting Jira MCP Server...")
    mcp.run()
